//! ソケットを使わない要求の振り分け、OS での表示と回答の追記。

use super::http::{self, Body, Request, Response};
use super::{State, listing, markdown, paths};
use crate::questions::{self, AnswerError, AnswerRequest};
use crate::shell;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

const INDEX: &str = include_str!("assets/index.html");
const APP_JS: &str = include_str!("assets/app.js");
const APP_CSS: &str = include_str!("assets/app.css");
const APP_CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; frame-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// OS の既定のアプリで開く拡張子の正本。画面にも /api/config で渡す。
const OPEN_EXTENSIONS: &[&str] = &[
    "md", "txt", "html", "htm", "pdf", "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "csv", "tsv", "xlsx", "docx",
    "pptx", "json", "yaml", "yml", "toml", "log",
];

/// macOS の open がアプリ等として扱うディレクトリ。全 OS で同じ規則を使う。
const BUNDLE_EXTENSIONS: &[&str] = &[
    "app",
    "bundle",
    "framework",
    "plugin",
    "kext",
    "prefpane",
    "appex",
    "xpc",
    "workflow",
    "action",
    "saver",
    "qlgenerator",
    "mdimporter",
    "pkg",
    "mpkg",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SpecialFolder {
    Bundle(&'static str),
    Structure,
    Clsid,
}

/// 名前の判定とフォルダの選択は純粋な処理にし、構造の照会だけを差し込む。
fn safe_open_folder_with(
    room: &Path,
    folder: &Path,
    has_package_structure: impl Fn(&Path) -> bool,
) -> Result<(PathBuf, Option<SpecialFolder>), u16> {
    let kind = |path: &Path| {
        if let Some(extension) = path.extension().and_then(|value| value.to_str()) {
            if let Some(bundle) = BUNDLE_EXTENSIONS.iter().copied().find(|e| extension.eq_ignore_ascii_case(e)) {
                return Some(SpecialFolder::Bundle(bundle));
            }
            if extension.starts_with('{') && extension.ends_with('}') {
                return Some(SpecialFolder::Clsid);
            }
        }
        has_package_structure(path).then_some(SpecialFolder::Structure)
    };
    let relative = folder.strip_prefix(room).map_err(|_| 404u16)?;
    if kind(room).is_some() {
        return Err(422);
    }
    let mut target = room.to_path_buf();
    for component in relative.components() {
        target.push(component.as_os_str());
        if let Some(special) = kind(&target) {
            target.pop();
            return Ok((target, Some(special)));
        }
    }
    Ok((target, None))
}

/// 全 OS で Contents の構造も調べ、照会に失敗した場合も安全側で回避する。
fn safe_open_folder(room: &Path, folder: &Path) -> Result<(PathBuf, Option<SpecialFolder>), u16> {
    safe_open_folder_with(room, folder, |path| {
        ["Contents/Info.plist", "Contents/MacOS"].iter().any(|marker| path.join(marker).try_exists().unwrap_or(true))
    })
}

#[cfg(windows)]
fn unchanged_absolute_path(path: &std::ffi::OsStr) -> bool {
    std::path::absolute(path).is_ok_and(|absolute| absolute.as_os_str() == path)
}

#[derive(Default)]
struct Parameters {
    values: BTreeMap<String, String>,
    keep: Vec<String>,
}

impl Parameters {
    fn get(&self, key: &str) -> Option<&String> {
        self.values.get(key)
    }
}

fn parameters(query: &str) -> Result<Parameters, u16> {
    let mut params = Parameters::default();
    for item in query.split('&').filter(|item| !item.is_empty()) {
        let (key, value) = item.split_once('=').unwrap_or((item, ""));
        let key = http::decode(&key.replace('+', " "))?;
        let value = http::decode(&value.replace('+', " "))?;
        if key == "keep" {
            params.keep.push(value);
        } else if params.values.insert(key, value).is_some() {
            return Err(400);
        }
    }
    Ok(params)
}

fn json_response(value: Value) -> Response {
    Response::bytes(200, "application/json; charset=utf-8", value.to_string())
        .header("Content-Security-Policy", "default-src 'none'; frame-ancestors 'none'")
}

fn filter(state: &State, params: &Parameters) -> Result<listing::Filter, u16> {
    fn choice<T: serde::de::DeserializeOwned>(params: &Parameters, key: &str, default: T) -> Result<T, u16> {
        match params.get(key) {
            Some(value) => serde_json::from_value(json!(value)).map_err(|_| 400),
            None => Ok(default),
        }
    }
    let query =
        params.get("q").map(|q| (!q.is_empty()).then(|| q.clone())).unwrap_or_else(|| state.initial.query.clone());
    let status = choice(params, "status", state.initial.status)?;
    let range = choice(params, "range", state.initial.range)?;
    let scope = choice(params, "scope", state.initial.scope)?;
    let period = match params.get("period").map(String::as_str) {
        Some("1") => true,
        Some("0") => false,
        None => state.initial.period,
        _ => return Err(400),
    };
    let days = params
        .get("days")
        .map(|days| days.parse::<i64>().map_err(|_| 400u16))
        .transpose()?
        .unwrap_or(state.initial.days);
    let filter = listing::Filter { query, status, period, days, range, scope };
    filter.cutoff(crate::room::today(&state.settings)).map_err(|_| 400u16)?;
    Ok(filter)
}

fn summaries(state: &State, params: &Parameters) -> Result<(Vec<Value>, String), u16> {
    let filter = filter(state, params)?;
    let (rooms, mut label) = listing::selected(state, &filter).map_err(|_| 500u16)?;
    let mut rooms: Vec<_> = rooms
        .iter()
        .filter(|room| paths::room_path(&state.settings.root, &room.label()).is_ok())
        .map(|room| listing::summary(state, room))
        .collect();
    // 説明の安全な読み取りで失敗した部屋も、外部ファイルの内容を検索結果に使わない。
    if let Some(query) = &filter.query {
        let query = query.to_lowercase();
        rooms.retain(|room| {
            room["room"].as_str().unwrap_or_default().to_lowercase().contains(&query)
                || room["description"].as_str().unwrap_or_default().to_lowercase().contains(&query)
        });
    }
    if params.get("unanswered").is_some_and(|v| v == "1") {
        rooms.retain(|room| room["unanswered"].as_u64().unwrap_or(0) > 0);
        label.push_str(" · 未回答あり");
    }
    for keep in &params.keep {
        if !rooms.iter().any(|room| room["room"] == *keep)
            && let Some(mut room) = listing::kept_summary(state, keep)
        {
            room["outside"] = json!(true);
            rooms.push(room);
        }
    }
    if rooms.iter().any(|room| room["outside"] == true) {
        // scan と同じ順に戻してから、安定ソートで更新順・回答待ち順を適用する。
        rooms.sort_by(|a, b| {
            let project =
                |room: &Value| room["room"].as_str().unwrap_or_default().rsplit_once('/').map(|(p, _)| p.to_owned());
            b["name"].as_str().cmp(&a["name"].as_str()).then_with(|| project(a).cmp(&project(b)))
        });
    }
    match params.get("sort").map(String::as_str).unwrap_or("date") {
        "date" => {}
        "updated" => rooms.sort_by_key(|room| std::cmp::Reverse(room["modified"].as_u64().unwrap_or(0))),
        "unanswered" => rooms.sort_by_key(|room| std::cmp::Reverse(room["unanswered"].as_u64().unwrap_or(0))),
        _ => return Err(400),
    }
    Ok((rooms, label))
}

pub fn validate_host(state: &State, request: &Request) -> Result<(), u16> {
    let hosts: Vec<_> = request.headers("host").collect();
    let host_ok = hosts.len() == 1
        && (hosts[0] == format!("127.0.0.1:{}", state.port) || hosts[0] == format!("localhost:{}", state.port));
    if host_ok { Ok(()) } else { Err(421) }
}

pub fn request_error(request: &Request, status: u16) -> Response {
    let response = if http::is_json_post(request) {
        json_error(status, json!({"error": http::error_message(status)})).header("Cache-Control", "no-store")
    } else {
        Response::error(status)
    };
    isolation_headers(request, response)
}

fn json_error(status: u16, value: Value) -> Response {
    let mut response = json_response(value);
    response.status = status;
    response
}

fn isolation_headers(request: &Request, response: Response) -> Response {
    if request.target.starts_with("/api/") || request.target.starts_with("/assets/") {
        response.header("Cross-Origin-Resource-Policy", "same-origin")
    } else {
        response
    }
}

pub fn handle(state: &State, request: &Request) -> Response {
    let result = if let Err(status) = validate_host(state, request) {
        Err(status)
    } else if http::is_json_post(request) {
        match request.target.split('?').next() {
            Some("/api/open") => post_open(state, request),
            _ => post_answers(state, request),
        }
    } else if request.method != "GET" {
        Err(405)
    } else {
        get(state, &request.target)
    };
    let response = match result {
        Ok(response) => response.header("Cache-Control", "no-store"),
        Err(status) => return request_error(request, status),
    };
    isolation_headers(request, response)
}

fn write_allowed(state: &State, request: &Request) -> bool {
    fn single<'a>(request: &'a Request, name: &str) -> Option<&'a str> {
        let mut values = request.headers.iter().filter(|(key, _)| key.eq_ignore_ascii_case(name));
        let (_, value) = values.next()?;
        if values.next().is_some() { None } else { Some(value) }
    }
    let origin_ok = single(request, "origin").is_some_and(|origin| {
        origin == format!("http://127.0.0.1:{}", state.port) || origin == format!("http://localhost:{}", state.port)
    });
    let content_type_ok = single(request, "content-type").is_some_and(|content_type| {
        content_type.split(';').next().is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
    });
    let token_ok = !state.token.is_empty() && single(request, "x-workroom-token") == Some(state.token.as_str());
    let fetch_ok =
        request.headers("sec-fetch-site").next().is_none() || single(request, "sec-fetch-site") == Some("same-origin");
    origin_ok && content_type_ok && token_ok && fetch_ok
}

fn post_open(state: &State, request: &Request) -> Result<Response, u16> {
    if !write_allowed(state, request) {
        return Err(403);
    }
    #[derive(Deserialize)]
    struct Submission {
        room: String,
        #[serde(default, deserialize_with = "plain_path")]
        path: Option<String>,
        #[serde(default)]
        folder: bool,
    }
    // 省略だけを許し、明示的な null など文字列以外は400にする。
    fn plain_path<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
        String::deserialize(deserializer).map(Some)
    }
    let submission: Submission = serde_json::from_slice(&request.body).map_err(|_| 400u16)?;
    let room = paths::room_path(&state.settings.root, &submission.room)?;
    let path = match submission.path {
        Some(relative) => {
            // JSON の path は平文。URL のパーセントデコードを通さない。
            let parts: Vec<_> = relative.split('/').collect();
            paths::resolve_path(&state.settings.root, &submission.room, &parts)?
        }
        None => room.clone(),
    };
    let extension = path.extension().and_then(|value| value.to_str()).unwrap_or_default().to_ascii_lowercase();
    let allowed = OPEN_EXTENSIONS.contains(&extension.as_str());
    let (target, opened, mut message) = if path.is_dir() {
        (path.as_path(), "folder", "フォルダを開くよう OS に依頼しました".to_owned())
    } else if submission.folder || !allowed {
        let message = if submission.folder {
            "フォルダを開くよう OS に依頼しました".to_owned()
        } else {
            let kind = if extension.is_empty() { "拡張子なし".to_owned() } else { format!(".{extension}") };
            format!("この種類のファイル（{kind}）は開かず、フォルダを開くよう OS に依頼しました")
        };
        (path.parent().ok_or(404u16)?, "folder", message)
    } else {
        (path.as_path(), "file", "既定のアプリで開くよう OS に依頼しました".to_owned())
    };
    let target = if opened == "folder" {
        let (target, bundle) = match safe_open_folder(&room, target) {
            Ok(safe) => safe,
            Err(422) => {
                return Ok(json_error(422, json!({"error": "アプリなどとして扱われるフォルダは開けません"})));
            }
            Err(status) => return Err(status),
        };
        if let Some(special) = bundle {
            message = match special {
                SpecialFolder::Bundle(extension) => format!(
                    "アプリとして扱われるフォルダ（.{extension}）は開かず、外側のフォルダを開くよう OS に依頼しました"
                ),
                _ => "アプリなどとして扱われるフォルダは開かず、外側のフォルダを開くよう OS に依頼しました".into(),
            };
        }
        target
    } else {
        target.to_path_buf()
    };
    let target = shell::strip_verbatim_for_open(target.as_os_str());
    #[cfg(windows)]
    if !unchanged_absolute_path(&target) {
        return Ok(json_error(422, json!({"error": "OS に渡すパスが正規化で変わるため開けません"})));
    }
    if let Err(error) = (state.opener)(&target) {
        eprintln!("OS に開くよう依頼できません: {error:#}");
        return Ok(json_error(500, json!({"error": "OS に開くよう依頼できませんでした"})));
    }
    Ok(json_response(json!({"opened": opened, "message": message})))
}

fn post_answers(state: &State, request: &Request) -> Result<Response, u16> {
    if !write_allowed(state, request) {
        return Err(403);
    }
    #[derive(Deserialize)]
    struct Submission {
        room: String,
        answers: Vec<Value>,
    }
    let submission: Submission = serde_json::from_slice(&request.body).map_err(|_| 400u16)?;
    let room = paths::room_path(&state.settings.root, &submission.room)?;
    let mut answers = Vec::with_capacity(submission.answers.len());
    for value in submission.answers {
        let id = value.get("id").and_then(Value::as_str).map(str::to_owned);
        let parsed: AnswerRequest = match serde_json::from_value(value) {
            Ok(parsed) => parsed,
            Err(_) => {
                let mut error = json!({"error": "回答の形式が不正です"});
                if let Some(id) = id {
                    error["id"] = json!(id);
                }
                return Ok(json_error(400, error));
            }
        };
        answers.push(parsed);
    }
    match questions::answer(&room, answers) {
        Ok(sent) => {
            // 追記によるサイズの変化を既存のキャッシュが検知し、読み直す。
            Ok(json_response(json!({"questions": listing::questions(state, &submission.room, &room), "sent": sent})))
        }
        Err(AnswerError::Invalid { id, message }) => {
            let mut error = json!({"error": message});
            if let Some(id) = id {
                error["id"] = json!(id);
            }
            Ok(json_error(422, error))
        }
        Err(AnswerError::TooLarge { id }) => {
            Ok(json_error(413, json!({"error": "回答の本文は64 KiB以内で指定してください", "id": id})))
        }
        Err(AnswerError::Withdrawn { id, reason, successor }) => Ok(json_error(
            409,
            json!({"error": "確認事項は取り下げ済みです", "id": id, "state": "withdrawn", "reason": reason, "successor": successor}),
        )),
        Err(AnswerError::Storage(error)) => {
            eprintln!("回答を追記できません: {error:#}");
            Err(500)
        }
    }
}

fn get(state: &State, target: &str) -> Result<Response, u16> {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let params = parameters(query)?;
    let response = match path {
        "/" => Response::bytes(200, "text/html; charset=utf-8", INDEX.replace("{{WORKROOM_TOKEN}}", &state.token))
            .header("Content-Security-Policy", APP_CSP),
        "/assets/app.js" => Response::bytes(200, "text/javascript; charset=utf-8", APP_JS),
        "/assets/app.css" => Response::bytes(200, "text/css; charset=utf-8", APP_CSS),
        "/api/hello" => json_response(json!({"app": "workroom", "root_hash": state.root_hash})),
        "/api/config" => {
            json_response(json!({"root": state.settings.root.display().to_string(), "filter": state.initial,
                "shell": if cfg!(windows) { "powershell" } else { "posix" }, "open_extensions": OPEN_EXTENSIONS}))
        }
        "/api/rooms" => {
            let (rooms, label) = summaries(state, &params)?;
            let count = rooms.iter().filter(|room| room["outside"] != true).count();
            json_response(json!({"rooms": rooms, "count": count, "label": label}))
        }
        "/api/files" | "/api/questions" => {
            let label = params.get("room").ok_or(400u16)?;
            let room = paths::room_path(&state.settings.root, label)?;
            if path == "/api/files" {
                json_response(listing::files(state, &room))
            } else {
                json_response(listing::questions(state, label, &room))
            }
        }
        "/api/questions/all" => {
            let (rooms, label) = summaries(state, &params)?;
            let groups: Vec<_> = rooms
                .iter()
                .filter_map(|room| {
                    let label = room["room"].as_str()?;
                    let path = paths::room_path(&state.settings.root, label).ok()?;
                    let mut group = listing::questions(state, label, &path);
                    if room["outside"] == true {
                        group["outside"] = json!(true);
                    }
                    Some(group)
                })
                .collect();
            json_response(json!({"rooms": groups, "label": label}))
        }
        _ if path.starts_with("/f/") => return serve_file(state, &path[3..], &params),
        _ => return Err(404),
    };
    Ok(response)
}

fn text_file(file: &mut File) -> std::io::Result<bool> {
    const SAMPLE_BYTES: usize = 8192;
    let mut sample = Vec::with_capacity(SAMPLE_BYTES);
    file.take(SAMPLE_BYTES as u64).read_to_end(&mut sample)?;
    let valid = match std::str::from_utf8(&sample) {
        Ok(text) => text.len(),
        // サンプルの境目で切れた UTF-8 は、それ以前の部分だけで判定する。
        Err(error) if error.error_len().is_none() && sample.len() == SAMPLE_BYTES => error.valid_up_to(),
        Err(_) => return Ok(false),
    };
    Ok(!sample[..valid].iter().any(|b| b.is_ascii_control() && !matches!(b, b'\n' | b'\r' | b'\t')))
}

pub fn file_type(path: &Path, file: &mut File) -> std::io::Result<&'static str> {
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    Ok(match extension.as_str() {
        "md" => "text/plain; charset=utf-8",
        "html" | "htm" => "text/html; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        "txt" | "csv" | "tsv" | "json" | "jsonl" | "rs" | "py" | "sql" | "js" | "ts" | "css" | "toml" | "yaml"
        | "yml" | "xml" | "log" | "sh" | "ps1" | "bat" | "cmd" | "ini" => "text/plain; charset=utf-8",
        _ if text_file(file)? => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    })
}

fn serve_file(state: &State, encoded: &str, params: &Parameters) -> Result<Response, u16> {
    let path = paths::file_path(&state.settings.root, encoded)?;
    let mut file = File::open(&path).map_err(|_| 404u16)?;
    let length = file.metadata().map_err(|_| 404u16)?.len();
    let markdown = markdown::is_markdown(&path);
    if markdown && params.get("raw").is_none_or(|v| v != "1") && length <= markdown::MAX_BYTES {
        let mut bytes = Vec::new();
        (&mut file).take(markdown::MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|_| 500u16)?;
        if bytes.len() as u64 <= markdown::MAX_BYTES {
            let parts = paths::components(encoded)?;
            let room_len = if crate::room::parse_room_name(&parts[0]).is_some() { 1 } else { 2 };
            let hub_url = markdown::hub_url(state.port, &parts[..room_len].join("/"), &parts[room_len..].join("/"));
            return Ok(Response::bytes(
                200,
                "text/html; charset=utf-8",
                markdown::render(
                    &bytes,
                    params.get("theme").map(String::as_str),
                    params.get("font").map(String::as_str),
                    Some(&hub_url),
                ),
            )
            .header("Content-Security-Policy", markdown::csp(state.port)));
        }
        file.rewind().map_err(|_| 500u16)?;
    }
    let content_type = file_type(&path, &mut file).map_err(|_| 500u16)?;
    let content_type = if params.get("raw").is_some_and(|v| v == "1")
        && matches!(content_type, "text/html; charset=utf-8" | "image/svg+xml")
    {
        "text/plain; charset=utf-8"
    } else {
        content_type
    };
    file.rewind().map_err(|_| 500u16)?;
    let policy = match content_type {
        "text/html; charset=utf-8" => format!(
            "sandbox allow-scripts allow-forms allow-modals allow-popups allow-popups-to-escape-sandbox; frame-ancestors http://127.0.0.1:{} http://localhost:{}",
            state.port, state.port
        ),
        "application/pdf" => format!("frame-ancestors http://127.0.0.1:{} http://localhost:{}", state.port, state.port),
        _ => "sandbox".into(),
    };
    let mut response = Response::bytes(200, content_type, Vec::new()).header("Content-Security-Policy", policy);
    if content_type == "application/octet-stream" {
        response = response.header("Content-Disposition", "attachment");
    }
    response.body = Body::File(file, length);
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn request(target: &str) -> Request {
        Request {
            method: "GET".into(),
            target: target.into(),
            headers: vec![("host".into(), "127.0.0.1:49123".into())],
            body: Vec::new(),
            head_bytes: 0,
        }
    }

    fn header<'a>(response: &'a Response, name: &str) -> &'a str {
        response
            .headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
            .unwrap_or_default()
    }

    fn open_request(state: &State, body: Value) -> Request {
        let mut req = request("/api/open");
        req.method = "POST".into();
        req.headers.extend([
            ("origin".into(), format!("http://127.0.0.1:{}", state.port)),
            ("content-type".into(), "application/json".into()),
            ("x-workroom-token".into(), state.token.clone()),
            ("sec-fetch-site".into(), "same-origin".into()),
        ]);
        req.body = serde_json::to_vec(&body).unwrap();
        req
    }

    fn open_json(response: Response, status: u16) -> Value {
        assert_eq!(response.status, status);
        assert_eq!(header(&response, "Content-Type"), "application/json; charset=utf-8");
        assert_eq!(header(&response, "Cache-Control"), "no-store");
        assert_eq!(header(&response, "Cross-Origin-Resource-Policy"), "same-origin");
        assert_eq!(header(&response, "X-Content-Type-Options"), "nosniff");
        assert_eq!(response.headers.iter().filter(|(name, _)| name.eq_ignore_ascii_case("Cache-Control")).count(), 1);
        let Body::Bytes(bytes) = response.body else { panic!("JSON の本文を返すこと") };
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        if status != 200 {
            assert!(value["error"].as_str().is_some_and(|message| !message.is_ascii()));
        }
        value
    }

    #[test]
    fn safe_open_folder_uses_outermost_bundle_without_filesystem_access() {
        let room = Path::new("rooms/2026-11-02-room");
        // 拡張子の一覧と大文字小文字の両方を、実在しないパスだけで検査する。
        for extension in [
            "app",
            "bundle",
            "framework",
            "plugin",
            "kext",
            "prefpane",
            "appex",
            "xpc",
            "workflow",
            "action",
            "saver",
            "qlgenerator",
            "mdimporter",
            "pkg",
            "mpkg",
        ] {
            for suffix in [extension.to_owned(), extension.to_ascii_uppercase()] {
                let folder = room.join(format!("a/x.{suffix}/nested.app/Contents"));
                assert_eq!(
                    safe_open_folder_with(room, &folder, |_| false),
                    Ok((room.join("a"), Some(SpecialFolder::Bundle(extension))))
                );
            }
        }
        for relative in ["", "a/b", "app/Contents", "x.app.txt/Contents"] {
            let folder = room.join(relative);
            assert_eq!(safe_open_folder_with(room, &folder, |_| false), Ok((folder, None)));
        }
        let room = Path::new("rooms/project.app/2026-11-02-room");
        assert_eq!(
            safe_open_folder_with(room, room, |_| false),
            Ok((room.to_path_buf(), None)),
            "部屋より上の要素は対象外"
        );
    }

    #[test]
    fn safe_open_folder_rejects_bundle_rooms_and_paths_outside_room() {
        let room = Path::new("rooms/2026-11-02-x.APP");
        for folder in [room.to_path_buf(), room.join("docs")] {
            assert_eq!(safe_open_folder_with(room, &folder, |_| false), Err(422));
        }
        let room = Path::new("rooms/2026-11-02-room");
        assert_eq!(safe_open_folder_with(room, Path::new("rooms/2026-11-02-room-other/docs"), |_| false), Err(404));
    }

    #[test]
    fn safe_open_folder_selects_outermost_structure_or_clsid_without_filesystem_access() {
        let room = Path::new("rooms/2026-11-02-room");
        let package = room.join("a/package");
        assert_eq!(
            safe_open_folder_with(room, &package.join("inner.app/Contents"), |path| path == package),
            Ok((room.join("a"), Some(SpecialFolder::Structure)))
        );
        let clsid = "folder.{00000000-0000-0000-0000-000000000000}";
        assert_eq!(
            safe_open_folder_with(room, &room.join(clsid).join("inner.app"), |_| false),
            Ok((room.to_path_buf(), Some(SpecialFolder::Clsid)))
        );
        for name in ["folder.{id", "folder.id}", "folder.{id}.txt"] {
            let folder = room.join(name);
            assert_eq!(safe_open_folder_with(room, &folder, |_| false), Ok((folder, None)));
        }
        assert_eq!(safe_open_folder_with(room, room, |_| true), Err(422));
        let clsid_room = Path::new("rooms/2026-11-02-room.{id}");
        assert_eq!(safe_open_folder_with(clsid_room, clsid_room, |_| false), Err(422));
    }

    #[test]
    fn open_avoids_package_structures_and_clsid_folders() {
        let dir = tempfile::tempdir().unwrap();
        let state = super::super::test_state(dir.path());
        let label = "2026-11-02-room";
        let room = dir.path().join(label);
        for relative in ["a/package", "a/macos", "a/folder.{00000000-0000-0000-0000-000000000000}"] {
            let package = room.join(relative);
            fs::create_dir_all(package.join("docs")).unwrap();
            if relative.ends_with("/package") {
                fs::create_dir(package.join("Contents")).unwrap();
                fs::write(package.join("Contents/Info.plist"), "構造だけを検査する").unwrap();
            } else if relative.ends_with("/macos") {
                fs::create_dir_all(package.join("Contents/MacOS")).unwrap();
            }
            fs::write(package.join("docs/readme.md"), "本文").unwrap();
            fs::write(package.join("a.ps1"), "実行しない").unwrap();
            for (path, folder, target, opened) in [
                (relative.to_owned(), false, room.join("a"), "folder"),
                (format!("{relative}/docs"), false, room.join("a"), "folder"),
                (format!("{relative}/docs/readme.md"), true, room.join("a"), "folder"),
                (format!("{relative}/a.ps1"), false, room.join("a"), "folder"),
                (format!("{relative}/docs/readme.md"), false, package.join("docs/readme.md"), "file"),
            ] {
                state.opened_paths.lock().unwrap().clear();
                let data = open_json(
                    handle(&state, &open_request(&state, json!({"room":label,"path":path,"folder":folder}))),
                    200,
                );
                assert_eq!(data["opened"], opened);
                if opened == "folder" {
                    assert_eq!(
                        data["message"],
                        "アプリなどとして扱われるフォルダは開かず、外側のフォルダを開くよう OS に依頼しました"
                    );
                }
                assert_eq!(
                    *state.opened_paths.lock().unwrap(),
                    [shell::strip_verbatim_for_open(target.canonicalize().unwrap().as_os_str())]
                );
            }
        }
    }

    #[test]
    fn open_rejects_package_structure_and_clsid_rooms_without_calling_opener() {
        let dir = tempfile::tempdir().unwrap();
        let state = super::super::test_state(dir.path());
        for (label, marker) in [
            ("2026-11-02-info", Some("Info.plist")),
            ("2026-11-02-macos", Some("MacOS")),
            ("2026-11-02-room.{00000000-0000-0000-0000-000000000000}", None),
        ] {
            let room = dir.path().join(label);
            fs::create_dir_all(room.join("docs")).unwrap();
            if let Some(marker) = marker {
                fs::create_dir(room.join("Contents")).unwrap();
                if marker == "MacOS" {
                    fs::create_dir(room.join("Contents/MacOS")).unwrap();
                } else {
                    fs::write(room.join("Contents/Info.plist"), "構造だけを検査する").unwrap();
                }
            }
            fs::write(room.join("docs/readme.md"), "本文").unwrap();
            fs::write(room.join("a.ps1"), "実行しない").unwrap();
            for body in [
                json!({"room":label}),
                json!({"room":label,"path":"docs"}),
                json!({"room":label,"path":"docs/readme.md","folder":true}),
                json!({"room":label,"path":"a.ps1"}),
            ] {
                let data = open_json(handle(&state, &open_request(&state, body)), 422);
                assert_eq!(data["error"], "アプリなどとして扱われるフォルダは開けません");
                assert!(state.opened_paths.lock().unwrap().is_empty());
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn stripped_open_paths_must_survive_windows_absolute_unchanged() {
        for path in [r"\\?\C:\work\a,b.md", r"\\?\UNC\server\share\報告.md"] {
            let stripped = shell::strip_verbatim_for_open(std::ffi::OsStr::new(path));
            assert!(unchanged_absolute_path(&stripped), "{path}");
        }
        for path in [r"\\?\C:\work\evil.exe.", "\\\\?\\C:\\work\\evil.exe ", r"C:\work\a\..\b.md"] {
            let stripped = shell::strip_verbatim_for_open(std::ffi::OsStr::new(path));
            assert!(!unchanged_absolute_path(&stripped), "{path}");
        }
    }

    #[test]
    fn open_redirects_bundle_folders_but_still_opens_allowed_files() {
        let dir = tempfile::tempdir().unwrap();
        let state = super::super::test_state(dir.path());
        let label = "2026-11-02-room";
        let room = dir.path().join(label);
        for directory in ["x.app", "x.APP/Contents", "a/b.framework/c", "a/x.bundle/y.app/Contents"] {
            fs::create_dir_all(room.join(directory)).unwrap();
        }
        for file in ["x.APP/Contents/Info.plist", "x.app/a.ps1", "x.app/readme.md"] {
            fs::write(room.join(file), "実行しない").unwrap();
        }
        for (path, folder, target, opened, bundle) in [
            ("x.app", false, "", "folder", Some("app")),
            ("x.APP/Contents/Info.plist", true, "", "folder", Some("app")),
            ("x.app/a.ps1", false, "", "folder", Some("app")),
            ("a/b.framework/c", false, "a", "folder", Some("framework")),
            ("a/x.bundle/y.app/Contents", true, "a", "folder", Some("bundle")),
            ("x.app/readme.md", false, "x.app/readme.md", "file", None),
        ] {
            state.opened_paths.lock().unwrap().clear();
            let data = open_json(
                handle(&state, &open_request(&state, json!({"room":label,"path":path,"folder":folder}))),
                200,
            );
            assert_eq!(data["opened"], opened);
            assert_eq!(
                *state.opened_paths.lock().unwrap(),
                [shell::strip_verbatim_for_open(room.join(target).canonicalize().unwrap().as_os_str())],
                "{path}"
            );
            if let Some(extension) = bundle {
                assert_eq!(
                    data["message"],
                    format!(
                        "アプリとして扱われるフォルダ（.{extension}）は開かず、外側のフォルダを開くよう OS に依頼しました"
                    )
                );
            } else {
                assert_eq!(data["message"], "既定のアプリで開くよう OS に依頼しました");
            }
        }
    }

    #[test]
    fn open_rejects_bundle_room_folders_without_calling_opener() {
        let dir = tempfile::tempdir().unwrap();
        let state = super::super::test_state(dir.path());
        let label = "2026-11-02-x.app";
        assert!(crate::room::parse_room_name(label).is_some());
        let room = dir.path().join(label);
        fs::create_dir_all(room.join("docs")).unwrap();
        for name in ["readme.md", "a.ps1"] {
            fs::write(room.join("docs").join(name), "実行しない").unwrap();
        }
        for body in [
            json!({"room":label}),
            json!({"room":label,"path":"docs"}),
            json!({"room":label,"path":"docs/readme.md","folder":true}),
            json!({"room":label,"path":"docs/a.ps1"}),
        ] {
            let data = open_json(handle(&state, &open_request(&state, body)), 422);
            assert_eq!(data["error"], "アプリなどとして扱われるフォルダは開けません");
            assert!(state.opened_paths.lock().unwrap().is_empty());
        }
        // 許可したファイルを既定のアプリで開く経路には、フォルダの規則を当てない。
        let data = open_json(handle(&state, &open_request(&state, json!({"room":label,"path":"docs/readme.md"}))), 200);
        assert_eq!(data["opened"], "file");
        assert_eq!(
            *state.opened_paths.lock().unwrap(),
            [shell::strip_verbatim_for_open(room.join("docs/readme.md").canonicalize().unwrap().as_os_str())]
        );
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn open_checks_canonical_folder_names_when_links_hide_bundles() {
        let dir = tempfile::tempdir().unwrap();
        let state = super::super::test_state(dir.path());
        let label = "2026-11-02-room";
        let room = dir.path().join(label);
        fs::create_dir_all(room.join("x.app/Contents")).unwrap();
        fs::write(room.join("x.app/Contents/a.ps1"), "実行しない").unwrap();
        #[cfg(unix)]
        let symlink = |from: &Path, to: &Path| std::os::unix::fs::symlink(from, to);
        #[cfg(windows)]
        let symlink = |from: &Path, to: &Path| std::os::windows::fs::symlink_dir(from, to);
        if let Err(error) = symlink(&room.join("x.app/Contents"), &room.join("alias")) {
            #[cfg(windows)]
            if error.kind() == std::io::ErrorKind::PermissionDenied || error.raw_os_error() == Some(1314) {
                eprintln!("シンボリックリンクの作成権限がないためスキップ: {error}");
                return;
            }
            panic!("シンボリックリンクを作れません: {error}");
        }
        for (path, folder) in [("alias", false), ("alias/a.ps1", true), ("alias/a.ps1", false)] {
            state.opened_paths.lock().unwrap().clear();
            let data = open_json(
                handle(&state, &open_request(&state, json!({"room":label,"path":path,"folder":folder}))),
                200,
            );
            assert_eq!(data["opened"], "folder");
            assert!(data["message"].as_str().unwrap().contains("フォルダ（.app）は開かず"));
            assert_eq!(
                *state.opened_paths.lock().unwrap(),
                [shell::strip_verbatim_for_open(room.canonicalize().unwrap().as_os_str())]
            );
        }
    }

    #[test]
    fn open_selects_rooms_files_parent_folders_and_nested_directories() {
        let dir = tempfile::tempdir().unwrap();
        let state = super::super::test_state(dir.path());
        for label in ["2026-11-02-room", "EXAMPLE/2026-11-03-調査"] {
            let room = dir.path().join(label);
            fs::create_dir_all(room.join("docs")).unwrap();
            for name in ["a.md", "A.MD", "a%20b.md", "a%ff.md"] {
                fs::write(room.join("docs").join(name), "本文").unwrap();
            }
            for (body, target, opened) in [
                (json!({"room":label}), room.clone(), "folder"),
                (json!({"room":label,"folder":true,"unknown":42}), room.clone(), "folder"),
                (json!({"room":label,"path":"docs"}), room.join("docs"), "folder"),
                (json!({"room":label,"path":"docs","folder":true}), room.join("docs"), "folder"),
                (json!({"room":label,"path":"docs/a.md","folder":true}), room.join("docs"), "folder"),
                (json!({"room":label,"path":"docs/a.md"}), room.join("docs/a.md"), "file"),
                (json!({"room":label,"path":"docs/A.MD"}), room.join("docs/A.MD"), "file"),
                (json!({"room":label,"path":"docs/a%20b.md"}), room.join("docs/a%20b.md"), "file"),
                (json!({"room":label,"path":"docs/a%ff.md"}), room.join("docs/a%ff.md"), "file"),
            ] {
                state.opened_paths.lock().unwrap().clear();
                let data = open_json(handle(&state, &open_request(&state, body)), 200);
                assert_eq!(data["opened"], opened);
                assert!(data["message"].as_str().unwrap().ends_with("OS に依頼しました"));
                let expected = shell::strip_verbatim_for_open(target.canonicalize().unwrap().as_os_str());
                assert_eq!(*state.opened_paths.lock().unwrap(), [expected]);
                assert!(!state.opened_paths.lock().unwrap()[0].to_string_lossy().starts_with(r"\\?\"));
            }
        }
    }

    #[test]
    fn open_falls_back_to_parent_for_unlisted_extensions_without_executing_files() {
        let dir = tempfile::tempdir().unwrap();
        let state = super::super::test_state(dir.path());
        let label = "2026-11-02-room";
        let room = dir.path().join(label);
        fs::create_dir(&room).unwrap();
        for name in ["a.ps1", "a.md.exe", "a.bat", "a.lnk", "a.url", "a.xlsm", "Makefile"] {
            fs::write(room.join(name), "実行しない").unwrap();
            state.opened_paths.lock().unwrap().clear();
            let data = open_json(handle(&state, &open_request(&state, json!({"room":label,"path":name}))), 200);
            assert_eq!(data["opened"], "folder");
            let message = data["message"].as_str().unwrap();
            assert!(message.contains("は開かず、フォルダ"));
            assert!(message.contains(if name == "Makefile" {
                "拡張子なし"
            } else {
                name.rsplit('.').next().unwrap()
            }));
            assert_eq!(
                *state.opened_paths.lock().unwrap(),
                [shell::strip_verbatim_for_open(room.canonicalize().unwrap().as_os_str())]
            );
        }
    }

    #[test]
    fn open_rejects_bad_paths_and_missing_rooms_without_calling_opener() {
        let dir = tempfile::tempdir().unwrap();
        let state = super::super::test_state(dir.path());
        let label = "2026-11-02-room";
        fs::create_dir(dir.path().join(label)).unwrap();
        for path in [
            "..",
            "../a.md",
            ".hidden",
            "docs//a.md",
            "",
            "/a.md",
            "docs/",
            r"docs\a.md",
            "a:b",
            "NUL",
            "a.",
            "a ",
            "missing.md",
        ] {
            open_json(handle(&state, &open_request(&state, json!({"room":label,"path":path}))), 404);
        }
        for room in ["2026-11-02-missing", "../2026-11-02-room", "", "EXAMPLE"] {
            open_json(handle(&state, &open_request(&state, json!({"room":room}))), 404);
        }
        assert!(state.opened_paths.lock().unwrap().is_empty());
    }

    #[test]
    fn open_checks_host_then_write_headers_before_parsing_json() {
        let dir = tempfile::tempdir().unwrap();
        let state = super::super::test_state(dir.path());
        for (name, value) in [
            ("x-workroom-token", None),
            ("x-workroom-token", Some("wrong")),
            ("origin", None),
            ("origin", Some("http://evil.example:49123")),
            ("content-type", Some("text/plain")),
            ("sec-fetch-site", Some("cross-site")),
        ] {
            let mut req = open_request(&state, json!({"room":"2026-11-02-room"}));
            req.headers.retain(|(key, _)| key != name);
            if let Some(value) = value {
                req.headers.push((name.into(), value.into()));
            }
            req.body = b"{".to_vec();
            open_json(handle(&state, &req), 403);
            req.headers[0].1 = "evil.example".into();
            open_json(handle(&state, &req), 421);
        }
        for name in ["origin", "content-type", "x-workroom-token", "sec-fetch-site"] {
            let mut req = open_request(&state, json!({}));
            let duplicate = req.headers.iter().find(|(key, _)| key == name).unwrap().clone();
            req.headers.push(duplicate);
            open_json(handle(&state, &req), 403);
        }
        assert!(state.opened_paths.lock().unwrap().is_empty());
    }

    #[test]
    fn open_rejects_broken_json_and_wrong_types_without_calling_opener() {
        let dir = tempfile::tempdir().unwrap();
        let state = super::super::test_state(dir.path());
        for body in [
            json!({}),
            json!([]),
            json!({"room":42}),
            json!({"room":"room","path":42}),
            json!({"room":"room","path":null}),
            json!({"room":"room","folder":"true"}),
            json!({"room":"room","folder":null}),
        ] {
            open_json(handle(&state, &open_request(&state, body)), 400);
        }
        for bytes in [b"{".as_slice(), b"\xff", b""] {
            let mut req = open_request(&state, json!({}));
            req.body = bytes.to_vec();
            open_json(handle(&state, &req), 400);
        }
        assert!(state.opened_paths.lock().unwrap().is_empty());
    }

    #[test]
    fn open_returns_japanese_json_when_opener_fails() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = super::super::test_state(dir.path());
        let recorded = std::sync::Arc::clone(&state.opened_paths);
        state.opener = Box::new(move |path| {
            recorded.lock().unwrap().push(path.to_owned());
            anyhow::bail!("テスト用の起動失敗")
        });
        fs::create_dir(dir.path().join("2026-11-02-room")).unwrap();
        let data = open_json(handle(&state, &open_request(&state, json!({"room":"2026-11-02-room"}))), 500);
        assert_eq!(data["error"], "OS に開くよう依頼できませんでした");
        assert_eq!(state.opened_paths.lock().unwrap().len(), 1);
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn open_uses_canonical_extension_and_rejects_links_outside_room() {
        let dir = tempfile::tempdir().unwrap();
        let state = super::super::test_state(dir.path());
        let label = "2026-11-02-room";
        let room = dir.path().join(label);
        fs::create_dir(&room).unwrap();
        fs::write(room.join("x.exe"), "実行しない").unwrap();
        fs::write(dir.path().join("outside.md"), "外").unwrap();
        #[cfg(unix)]
        let symlink = |from: &Path, to: &Path| std::os::unix::fs::symlink(from, to);
        #[cfg(windows)]
        let symlink = |from: &Path, to: &Path| std::os::windows::fs::symlink_file(from, to);
        if let Err(error) = symlink(&room.join("x.exe"), &room.join("a.md")) {
            #[cfg(windows)]
            if error.kind() == std::io::ErrorKind::PermissionDenied || error.raw_os_error() == Some(1314) {
                eprintln!("シンボリックリンクの作成権限がないためスキップ: {error}");
                return;
            }
            panic!("シンボリックリンクを作れません: {error}");
        }
        let data = open_json(handle(&state, &open_request(&state, json!({"room":label,"path":"a.md"}))), 200);
        assert_eq!(data["opened"], "folder");
        assert!(data["message"].as_str().unwrap().contains(".exe"));
        assert_eq!(
            *state.opened_paths.lock().unwrap(),
            [shell::strip_verbatim_for_open(room.canonicalize().unwrap().as_os_str())]
        );
        state.opened_paths.lock().unwrap().clear();
        symlink(&dir.path().join("outside.md"), &room.join("outside.md")).unwrap();
        for folder in [false, true] {
            open_json(
                handle(&state, &open_request(&state, json!({"room":label,"path":"outside.md","folder":folder}))),
                404,
            );
        }
        assert!(state.opened_paths.lock().unwrap().is_empty());
    }

    #[test]
    fn config_exposes_the_rust_open_extension_list() {
        let dir = tempfile::tempdir().unwrap();
        let state = super::super::test_state(dir.path());
        let data = open_json(handle(&state, &request("/api/config")), 200);
        assert_eq!(data["open_extensions"], json!(OPEN_EXTENSIONS));
    }

    #[test]
    fn host_is_checked_for_every_route_and_non_get_methods_are_405() {
        let dir = tempfile::tempdir().unwrap();
        let state = super::super::test_state(dir.path());
        for target in
            ["/", "/assets/app.js", "/api/hello", "/f/2026-11-02-room/a.md", "/missing", "/api/openx", "/api/open/x"]
        {
            for hosts in [
                vec![],
                vec!["evil.example:49123"],
                vec!["127.0.0.1:47100"],
                vec!["127.0.0.1:49123", "localhost:49123"],
            ] {
                let mut req = request(target);
                req.headers = hosts.into_iter().map(|host| ("host".into(), host.into())).collect();
                assert_eq!(handle(&state, &req).status, 421);
            }
            for method in ["POST", "PUT", "DELETE", "OPTIONS", "HEAD"] {
                let mut req = request(target);
                req.method = method.into();
                assert_eq!(handle(&state, &req).status, 405);
            }
        }
        let mut req = request("/api/hello");
        req.headers[0].1 = "localhost:49123".into();
        assert_eq!(handle(&state, &req).status, 200);
    }

    #[test]
    fn responses_have_correct_isolation_headers_and_use_actual_port() {
        let dir = tempfile::tempdir().unwrap();
        let state = super::super::test_state(dir.path());
        let room = dir.path().join("2026-11-02-room");
        fs::create_dir(&room).unwrap();
        for (file, bytes, mime) in [
            ("a.md", b"# title".as_slice(), "text/html; charset=utf-8"),
            ("a.html", b"<script>alert(1)</script>".as_slice(), "text/html; charset=utf-8"),
            ("a.svg", b"<svg/>".as_slice(), "image/svg+xml"),
            ("a.pdf", b"%PDF-1.4".as_slice(), "application/pdf"),
            ("a.bin", b"\0\xff".as_slice(), "application/octet-stream"),
            ("text.unknown", b"text".as_slice(), "text/plain; charset=utf-8"),
        ] {
            fs::write(room.join(file), bytes).unwrap();
            let response = handle(&state, &request(&format!("/f/2026-11-02-room/{file}")));
            assert_eq!(response.status, 200);
            assert_eq!(header(&response, "Content-Type"), mime);
            assert_eq!(header(&response, "X-Content-Type-Options"), "nosniff");
            assert_eq!(header(&response, "Cross-Origin-Resource-Policy"), "");
            let csp = header(&response, "Content-Security-Policy");
            assert!(!csp.contains("allow-same-origin"));
            if file == "a.pdf" {
                assert!(!csp.contains("sandbox"));
            } else {
                assert!(csp.contains("sandbox"));
            }
            if matches!(file, "a.md" | "a.html" | "a.pdf") {
                assert!(csp.contains("frame-ancestors http://127.0.0.1:49123 http://localhost:49123"));
                assert!(!csp.contains(":47100"));
            }
        }
        for target in ["/", "/assets/app.js", "/api/hello"] {
            let response = handle(&state, &request(target));
            assert_eq!(header(&response, "Cache-Control"), "no-store");
            assert_eq!(header(&response, "X-Content-Type-Options"), "nosniff");
            if target != "/" {
                assert_eq!(header(&response, "Cross-Origin-Resource-Policy"), "same-origin");
            }
        }
        assert!(header(&handle(&state, &request("/")), "Content-Security-Policy").contains("frame-ancestors 'none'"));
    }

    #[test]
    fn raw_html_htm_and_svg_are_plain_sandboxed_source_and_other_types_are_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let state = super::super::test_state(dir.path());
        let room = dir.path().join("2026-11-02-room");
        fs::create_dir(&room).unwrap();
        for (name, source, mime) in [
            ("a.html", b"<script>location='https://example.com/'</script>".as_slice(), "text/html; charset=utf-8"),
            ("b.HTM", b"<h1>https://example.com/</h1>", "text/html; charset=utf-8"),
            ("c.SVG", b"<svg><script>https://example.com/</script></svg>", "image/svg+xml"),
            ("a.png", b"\x89PNG\r\n\x1a\n", "image/png"),
            ("a.pdf", b"%PDF-1.4", "application/pdf"),
            ("a.txt", b"https://example.com/", "text/plain; charset=utf-8"),
            ("a.bin", b"\0\xff", "application/octet-stream"),
        ] {
            fs::write(room.join(name), source).unwrap();
            let target = format!("/f/2026-11-02-room/{name}");
            let normal = handle(&state, &request(&target));
            assert_eq!(header(&normal, "Content-Type"), mime);
            let normal_csp = header(&normal, "Content-Security-Policy");
            if mime.starts_with("text/html") {
                assert!(normal_csp.starts_with("sandbox allow-scripts allow-forms allow-modals allow-popups "));
            } else if mime == "image/svg+xml" {
                assert_eq!(normal_csp, "sandbox");
            }
            let raw = handle(&state, &request(&format!("{target}?raw=1")));
            assert_eq!(raw.status, 200);
            if matches!(mime, "text/html; charset=utf-8" | "image/svg+xml") {
                assert_eq!(header(&raw, "Content-Type"), "text/plain; charset=utf-8");
                assert_eq!(header(&raw, "Content-Security-Policy"), "sandbox");
                assert_eq!(header(&raw, "Content-Disposition"), "");
            } else {
                assert_eq!(raw.headers, normal.headers, "{name}");
            }
            let Body::File(mut file, length) = raw.body else { panic!("元のファイルを返すこと") };
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, source);
            assert_eq!(length, source.len() as u64);
            assert_eq!(handle(&state, &request(&format!("{target}?raw=0"))).headers, normal.headers);
        }
    }

    #[test]
    fn raw_and_large_markdown_are_plain_and_bad_paths_and_queries_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let state = super::super::test_state(dir.path());
        let room = dir.path().join("2026-11-02-room");
        fs::create_dir(&room).unwrap();
        fs::write(room.join("a.md"), "# title").unwrap();
        let file = File::create(room.join("large.md")).unwrap();
        file.set_len(markdown::MAX_BYTES + 1).unwrap();
        for target in ["/f/2026-11-02-room/a.md?raw=1", "/f/2026-11-02-room/large.md"] {
            let response = handle(&state, &request(target));
            assert_eq!(header(&response, "Content-Type"), "text/plain; charset=utf-8");
            assert_eq!(header(&response, "Content-Security-Policy"), "sandbox");
            assert!(matches!(response.body, Body::File(_, _)));
        }
        assert_eq!(handle(&state, &request("/f/2026-11-02-room/%2e%2e/a.md")).status, 404);
        assert_eq!(handle(&state, &request("/api/files?room=..%2F2026-11-02-room")).status, 404);
        for target in [
            "/api/rooms?period=1&scope=filtered&days=9223372036854775807",
            "/api/rooms?status=bad",
            "/api/rooms?range=bad",
            "/api/rooms?scope=bad",
            "/api/rooms?period=bad",
            "/api/files",
            "/api/rooms?q=a&q=b",
            "/api/rooms?q=%ff",
        ] {
            assert_eq!(handle(&state, &request(target)).status, 400, "{target}");
        }
    }

    #[test]
    fn unknown_extension_reads_only_eight_kib_of_large_text_and_allows_split_utf8() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.unknown");
        let mut bytes = vec![b'a'; 2 * 1024 * 1024];
        bytes[8191..8197].copy_from_slice("調査".as_bytes());
        fs::write(&path, bytes).unwrap();
        let mut file = File::open(&path).unwrap();
        assert_eq!(file_type(&path, &mut file).unwrap(), "text/plain; charset=utf-8");
        assert_eq!(file.stream_position().unwrap(), 8192);
    }

    #[test]
    fn markdown_double_extensions_render_with_hub_links_and_keep_raw_source() {
        let dir = tempfile::tempdir().unwrap();
        let state = super::super::test_state(dir.path());
        let room = dir.path().join("EXAMPLE/2026-11-03-調査");
        fs::create_dir_all(room.join("docs")).unwrap();
        for name in ["a.md.txt", "b.MD.TXT", "c.Md.TxT"] {
            fs::write(room.join("docs").join(name), "# 見出し\n").unwrap();
            let target = format!("/f/EXAMPLE/2026-11-03-%E8%AA%BF%E6%9F%BB/docs/{name}");
            let response = handle(&state, &request(&target));
            assert_eq!(response.status, 200);
            assert_eq!(header(&response, "Content-Type"), "text/html; charset=utf-8");
            assert_eq!(header(&response, "Content-Security-Policy"), markdown::csp(state.port));
            let Body::Bytes(bytes) = response.body else { panic!("描画した HTML を返すこと") };
            let html = String::from_utf8(bytes).unwrap();
            assert!(html.contains("<h1>見出し</h1>"));
            assert!(html.contains(
                "href=\"http://127.0.0.1:49123/#page=rooms&amp;room=EXAMPLE%2F2026-11-03-%E8%AA%BF%E6%9F%BB"
            ));
            assert!(html.contains(&format!("file=docs%2F{name}")));
            let response = handle(&state, &request(&format!("{target}?raw=1")));
            assert_eq!(header(&response, "Content-Type"), "text/plain; charset=utf-8");
            let Body::File(mut file, _) = response.body else { panic!("元のファイルを返すこと") };
            let mut source = String::new();
            file.read_to_string(&mut source).unwrap();
            assert_eq!(source, "# 見出し\n");
        }
    }

    #[test]
    fn text_sample_rejects_binary_and_short_invalid_utf8() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.unknown");
        for bytes in [b"a\0b".as_slice(), b"a\xffb", b"a\xe8"] {
            fs::write(&path, bytes).unwrap();
            assert_eq!(file_type(&path, &mut File::open(&path).unwrap()).unwrap(), "application/octet-stream");
        }
    }

    #[test]
    fn cache_control_occurs_once_on_success_and_error_responses() {
        let dir = tempfile::tempdir().unwrap();
        let state = super::super::test_state(dir.path());
        for (target, method, host, expected_status) in [
            ("/", "GET", "127.0.0.1:49123", 200),
            ("/api/hello", "GET", "127.0.0.1:49123", 200),
            ("/assets/app.js", "GET", "127.0.0.1:49123", 200),
            ("/missing", "GET", "127.0.0.1:49123", 404),
            ("/api/missing", "GET", "127.0.0.1:49123", 404),
            ("/api/hello", "POST", "127.0.0.1:49123", 405),
            ("/api/hello", "GET", "example.invalid", 421),
        ] {
            let mut req = request(target);
            req.method = method.into();
            req.headers[0].1 = host.into();
            let response = handle(&state, &req);
            assert_eq!(response.status, expected_status);
            assert_eq!(
                response.headers.iter().filter(|(name, _)| name.eq_ignore_ascii_case("Cache-Control")).count(),
                1
            );
            assert_eq!(header(&response, "Cache-Control"), "no-store");
        }
    }
}
