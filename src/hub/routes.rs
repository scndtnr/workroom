//! ソケットを使わない要求の振り分け。ディスクは読むだけで、応答を組み立てる。

use super::http::{self, Body, Request, Response};
use super::{State, listing, markdown, paths};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;

const INDEX: &str = include_str!("assets/index.html");
const APP_JS: &str = include_str!("assets/app.js");
const APP_CSS: &str = include_str!("assets/app.css");
const APP_CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; frame-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

type Parameters = BTreeMap<String, String>;

fn parameters(query: &str) -> Result<Parameters, u16> {
    let mut params = BTreeMap::new();
    for item in query.split('&').filter(|item| !item.is_empty()) {
        let (key, value) = item.split_once('=').unwrap_or((item, ""));
        let key = http::decode(&key.replace('+', " "))?;
        let value = http::decode(&value.replace('+', " "))?;
        if params.insert(key, value).is_some() {
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
    match params.get("sort").map(String::as_str).unwrap_or("date") {
        "date" => {}
        "updated" => rooms.sort_by_key(|room| std::cmp::Reverse(room["modified"].as_u64().unwrap_or(0))),
        "unanswered" => rooms.sort_by_key(|room| std::cmp::Reverse(room["unanswered"].as_u64().unwrap_or(0))),
        _ => return Err(400),
    }
    Ok((rooms, label))
}

pub fn handle(state: &State, request: &Request) -> Response {
    let hosts: Vec<_> = request.headers("host").collect();
    let host_ok = hosts.len() == 1
        && (hosts[0] == format!("127.0.0.1:{}", state.port) || hosts[0] == format!("localhost:{}", state.port));
    let result = if !host_ok {
        Err(421)
    } else if request.method != "GET" {
        Err(405)
    } else {
        get(state, &request.target)
    };
    let response = match result {
        Ok(response) => response.header("Cache-Control", "no-store"),
        Err(status) => Response::error(status),
    };
    if request.target.starts_with("/api/") || request.target.starts_with("/assets/") {
        response.header("Cross-Origin-Resource-Policy", "same-origin")
    } else {
        response
    }
}

fn get(state: &State, target: &str) -> Result<Response, u16> {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let params = parameters(query)?;
    let response = match path {
        "/" => Response::bytes(200, "text/html; charset=utf-8", INDEX).header("Content-Security-Policy", APP_CSP),
        "/assets/app.js" => Response::bytes(200, "text/javascript; charset=utf-8", APP_JS),
        "/assets/app.css" => Response::bytes(200, "text/css; charset=utf-8", APP_CSS),
        "/api/hello" => json_response(json!({"app": "workroom", "root_hash": state.root_hash})),
        "/api/config" => {
            json_response(json!({"root": state.settings.root.display().to_string(), "filter": state.initial}))
        }
        "/api/rooms" => {
            let (rooms, label) = summaries(state, &params)?;
            json_response(json!({"rooms": rooms, "label": label}))
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
                    Some(listing::questions(state, label, &path))
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
                markdown::render(&bytes, params.get("theme").map(String::as_str), Some(&hub_url)),
            )
            .header("Content-Security-Policy", markdown::csp(state.port)));
        }
        file.rewind().map_err(|_| 500u16)?;
    }
    let content_type = file_type(&path, &mut file).map_err(|_| 500u16)?;
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

    #[test]
    fn host_is_checked_for_every_route_and_non_get_methods_are_405() {
        let dir = tempfile::tempdir().unwrap();
        let state = super::super::test_state(dir.path());
        for target in ["/", "/assets/app.js", "/api/hello", "/f/2026-11-02-room/a.md", "/missing"] {
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
    fn raw_and_large_markdown_are_plain_and_bad_paths_and_queries_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let state = super::super::test_state(dir.path());
        let room = dir.path().join("2026-11-02-room");
        fs::create_dir(&room).unwrap();
        fs::write(room.join("a.md"), "# title").unwrap();
        let file = File::create(room.join("large.md")).unwrap();
        file.set_len(markdown::MAX_BYTES + 1).unwrap();
        for target in ["/f/2026-11-02-room/a.md?raw=1", "/f/2026-11-02-room/large.md"] {
            assert_eq!(header(&handle(&state, &request(target)), "Content-Type"), "text/plain; charset=utf-8");
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
