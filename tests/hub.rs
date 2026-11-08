//! 一時ディレクトリと生の HTTP だけで、ハブの読み取りと回答の追記を検証する。

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

struct Env {
    _dir: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let root = base.join("作業 ルート");
        Self { _dir: dir, base, root }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_workroom"));
        command
            .current_dir(&self.base)
            .env("WORKROOM_ROOT", &self.root)
            .env("WORKROOM_CONFIG", self.base.join("config.toml"))
            .env("WORKROOM_STATE_DIR", self.base.join("state"))
            .env("WORKROOM_CLAUDE_DIR", self.base.join("claude"));
        command
    }

    fn start(&self, extra: &[&str]) -> Hub {
        let mut command = self.command();
        command.args(["hub", "--port", "0", "--no-open"]).args(extra).stdout(Stdio::piped()).stderr(Stdio::inherit());
        let child = command.spawn().unwrap();
        let mut hub = Hub { child, port: 0 };
        let stdout = hub.child.stdout.take().unwrap();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let read = BufReader::new(stdout).read_line(&mut line);
            let _ = sender.send((read, line));
        });
        let (read, line) = receiver.recv_timeout(Duration::from_secs(10)).expect("URL の出力がありません");
        assert!(read.unwrap() > 0, "URL の出力前に終了しました");
        let port =
            line.trim().strip_prefix("http://127.0.0.1:").and_then(|s| s.strip_suffix('/')).expect("1行目は URL だけ");
        hub.port = port.parse().unwrap();
        assert_ne!(hub.port, 0);
        hub
    }

    fn run_hub(&self, port: u16) -> Output {
        let mut command = self.command();
        command.args(["hub", "--no-open", "--port", &port.to_string()]).stdout(Stdio::piped()).stderr(Stdio::piped());
        let child = command.spawn().unwrap();
        let mut guard = Hub { child, port };
        let stdout = guard.child.stdout.take().unwrap();
        let stderr = guard.child.stderr.take().unwrap();
        let read_stdout = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            BufReader::new(stdout).read_to_end(&mut bytes).unwrap();
            bytes
        });
        let read_stderr = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            BufReader::new(stderr).read_to_end(&mut bytes).unwrap();
            bytes
        });
        let started = std::time::Instant::now();
        let status = loop {
            if let Some(status) = guard.child.try_wait().unwrap() {
                break status;
            }
            assert!(started.elapsed() < Duration::from_secs(10), "使用中ポートの確認で終了しませんでした");
            std::thread::sleep(Duration::from_millis(10));
        };
        Output { status, stdout: read_stdout.join().unwrap(), stderr: read_stderr.join().unwrap() }
    }
}

struct Hub {
    child: Child,
    port: u16,
}

impl Drop for Hub {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Response {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl Hub {
    fn request(&self, method: &str, target: &str, host: Option<&str>) -> Response {
        let headers = host.map(|h| vec![("Host".into(), h.into())]).unwrap_or_default();
        self.raw_request(method, target, &headers, &[])
    }

    fn raw_request(&self, method: &str, target: &str, headers: &[(String, String)], body: &[u8]) -> Response {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, self.port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(stream, "{method} {target} HTTP/1.1\r\n").unwrap();
        if !headers.iter().any(|(key, _)| key.eq_ignore_ascii_case("host")) {
            write!(stream, "Host: 127.0.0.1:{}\r\n", self.port).unwrap();
        }
        for (name, value) in headers {
            write!(stream, "{name}: {value}\r\n").unwrap();
        }
        write!(stream, "Connection: close\r\n\r\n").unwrap();
        stream.write_all(body).unwrap();
        stream.shutdown(std::net::Shutdown::Write).unwrap();
        let mut bytes = Vec::new();
        if let Err(error) = stream.take(8 * 1024 * 1024).read_to_end(&mut bytes) {
            // Host の拒否で本文を読まずに閉じると Windows は reset を返すことがある。
            // 下で Content-Length を照合し、応答を最後まで受け取れた場合だけ許す。
            assert_eq!(error.kind(), std::io::ErrorKind::ConnectionReset);
        }
        let end = bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = std::str::from_utf8(&bytes[..end]).unwrap();
        let mut lines = head.split("\r\n");
        let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
        let headers: BTreeMap<_, _> = lines
            .map(|line| {
                let (name, value) = line.split_once(':').unwrap();
                (name.to_ascii_lowercase(), value.trim().to_string())
            })
            .collect();
        let body = bytes[end + 4..].to_vec();
        assert_eq!(headers["content-length"].parse::<usize>().unwrap(), body.len());
        assert_eq!(headers["connection"], "close");
        assert_eq!(headers["x-content-type-options"], "nosniff");
        Response { status, headers, body }
    }

    fn token(&self) -> String {
        let response = self.get("/");
        assert_eq!(response.status, 200);
        let html = String::from_utf8(response.body).unwrap();
        let token = html.split("<meta name=\"workroom-token\" content=\"").nth(1).unwrap().split('"').next().unwrap();
        assert_eq!(token.len(), 32);
        assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(!html.contains("{{WORKROOM_TOKEN}}"));
        token.into()
    }

    fn answer_headers(&self, token: &str, body: &[u8]) -> Vec<(String, String)> {
        vec![
            ("Origin".into(), format!("http://127.0.0.1:{}", self.port)),
            ("Content-Type".into(), "application/json; charset=utf-8".into()),
            ("X-Workroom-Token".into(), token.into()),
            ("Sec-Fetch-Site".into(), "same-origin".into()),
            ("Content-Length".into(), body.len().to_string()),
        ]
    }

    fn post_answers(&self, token: &str, body: Value) -> Response {
        let bytes = serde_json::to_vec(&body).unwrap();
        self.raw_request("POST", "/api/answers", &self.answer_headers(token, &bytes), &bytes)
    }

    fn get(&self, target: &str) -> Response {
        self.request("GET", target, None)
    }

    fn json(&self, target: &str) -> Value {
        let response = self.get(target);
        assert_eq!(response.status, 200, "{target}: {}", String::from_utf8_lossy(&response.body));
        assert_eq!(response.headers["cross-origin-resource-policy"], "same-origin");
        serde_json::from_slice(&response.body).unwrap()
    }
}

fn encode(value: &str) -> String {
    use std::fmt::Write;
    value.bytes().fold(String::new(), |mut result, byte| {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            result.push(byte as char);
        } else {
            write!(result, "%{byte:02X}").unwrap();
        }
        result
    })
}

fn snapshot(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(base: &Path, path: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let Ok(read) = fs::read_dir(path) else { return };
        for entry in read.map(Result::unwrap) {
            let path = entry.path();
            let relative = path.strip_prefix(base).unwrap().to_path_buf();
            if path.is_dir() {
                result.insert(relative, Vec::new());
                visit(base, &path, result);
            } else {
                result.insert(relative, fs::read(&path).unwrap());
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(path, path, &mut result);
    result
}

fn fixture(env: &Env) -> PathBuf {
    let room = env.root.join("EXAMPLE/2026-11-03-調査");
    fs::create_dir_all(room.join("outputs")).unwrap();
    fs::create_dir_all(room.join("docs")).unwrap();
    fs::write(room.join("README.md"), "# ハブの動作確認\n- status: 待ち（確認）\n").unwrap();
    fs::write(room.join("outputs/report.html"), "<!doctype html><h1>報告</h1>").unwrap();
    fs::write(
        room.join("docs/a.md"),
        "# 本文\n\n![図](image.svg) [外部](https://example.com/)\n\n<script>alert(1)</script>",
    )
    .unwrap();
    fs::write(room.join("docs/image.svg"), "<svg xmlns=\"http://www.w3.org/2000/svg\"/>").unwrap();
    fs::write(room.join("docs/report.MD.TXT"), "# 二重拡張子の本文\n").unwrap();
    fs::write(room.join("QUESTIONS.md"), "# 旧形式の質問\n").unwrap();
    let at = "2026-11-06T12:00:00+09:00";
    let events = [
        json!({"v":1,"type":"ask","id":"Q-20261106-01","at":at,"question":"未回答の質問","by":"エージェント","choices":["表","文章"],"recommend":1}),
        json!({"v":1,"type":"ask","id":"Q-20261106-02","at":at,"question":"未反映の質問"}),
        json!({"v":1,"type":"ask","id":"Q-20261106-03","at":at,"question":"解決した質問"}),
        json!({"v":1,"type":"done","id":"Q-20261106-03","at":at,"to":"docs/判断.md","upto":0,"chat":"会話の要旨"}),
        json!({"v":1,"type":"ask","id":"Q-20261106-04","at":at,"question":"取り下げた質問"}),
        json!({"v":1,"type":"withdraw","id":"Q-20261106-04","at":at,"reason":"前提の変更","successor":"Q-20261106-01"}),
    ];
    fs::write(
        room.join("QUESTIONS.jsonl"),
        events.iter().map(Value::to_string).collect::<Vec<_>>().join("\n") + "\n壊れた行\n",
    )
    .unwrap();
    fs::write(room.join("ANSWERS.jsonl"), json!({"v":1,"type":"answer","id":"Q-20261106-02","n":1,"at":at,"kind":"answer","text":"表でよい","question":{"question":"未反映の質問"}}).to_string() + "\n").unwrap();
    room
}

#[test]
fn lists_states_questions_archives_and_files_without_any_disk_writes() {
    let env = Env::new();
    let room = fixture(&env);
    let done = env.root.join("2026-11-02-done");
    fs::create_dir(&done).unwrap();
    fs::write(done.join(".done"), "").unwrap();
    let before = snapshot(&env.base);
    let hub = env.start(&["-a"]);
    let listing = hub.json("/api/rooms");
    let rooms = listing["rooms"].as_array().unwrap();
    let summary = rooms.iter().find(|r| r["room"] == "EXAMPLE/2026-11-03-調査").unwrap();
    assert_eq!(summary["unanswered"], 1);
    assert_eq!(summary["pending"], 1);
    assert_eq!(summary["state"], "ユーザー待ち");
    assert_eq!(summary["status"], "待ち（確認）");
    assert_eq!(summary["description"], "ハブの動作確認");
    assert_eq!(rooms.iter().find(|r| r["room"] == "2026-11-02-done").unwrap()["state"], "完了");
    assert_eq!(rooms.iter().find(|r| r["room"] == "2026-11-02-done").unwrap()["readme"], false);
    let query = format!("?room={}", encode("EXAMPLE/2026-11-03-調査"));
    let questions = hub.json(&format!("/api/questions{query}"));
    let items = questions["questions"].as_array().unwrap();
    assert_eq!(
        items.iter().map(|q| q["state"].as_str().unwrap()).collect::<Vec<_>>(),
        ["unanswered", "pending", "resolved", "withdrawn"]
    );
    assert_eq!(items[2]["done"][0]["chat"], "会話の要旨");
    assert_eq!(items[3]["withdrawals"][0]["reason"], "前提の変更");
    assert_eq!(questions["legacy"][0], "QUESTIONS.md");
    assert_eq!(questions["warnings"].as_array().unwrap().len(), 1);
    let files = hub.json(&format!("/api/files{query}"));
    assert!(files["main"]["items"].as_array().unwrap().iter().any(|item| item["path"] == "outputs/report.html"));
    assert_eq!(hub.json("/api/questions/all")["rooms"].as_array().unwrap().len(), 2);
    let prefix = format!("/f/EXAMPLE/{}/", encode("2026-11-03-調査"));
    for target in ["/", "/assets/app.js", "/assets/app.css", "/api/hello"] {
        assert_eq!(hub.get(target).status, 200);
    }
    for name in ["README.md", "outputs/report.html", "docs/a.md", "docs/image.svg", "QUESTIONS.md"] {
        assert_eq!(hub.get(&format!("{prefix}{name}")).status, 200);
    }
    assert_eq!(hub.request("POST", "/api/answers", None).status, 411);
    assert_eq!(hub.request("POST", "/api/open", None).status, 405);
    assert_eq!(snapshot(&env.base), before);
    // サイズの変化を検知し、再起動せずに確認事項のキャッシュを更新する。
    fs::write(room.join("ANSWERS.jsonl"), "").unwrap();
    assert_eq!(hub.json("/api/rooms")["rooms"][0]["unanswered"], 2);
}

#[test]
fn renders_markdown_raw_images_and_rejects_bad_hosts_and_paths() {
    let env = Env::new();
    fixture(&env);
    let hub = env.start(&["-a"]);
    let prefix = format!("/f/EXAMPLE/{}/", encode("2026-11-03-調査"));
    let response = hub.get(&format!("{prefix}docs/a.md?theme=dark"));
    assert_eq!(response.status, 200);
    let html = String::from_utf8(response.body).unwrap();
    assert!(html.contains("<h1>本文</h1>"));
    assert!(html.contains("&lt;script&gt;"));
    assert!(html.contains("src=\"image.svg\""));
    assert!(html.contains("class=\"dark\""));
    assert!(response.headers["content-security-policy"].contains(&format!("http://127.0.0.1:{}", hub.port)));
    assert!(!response.headers.contains_key("cross-origin-resource-policy"));
    let raw = hub.get(&format!("{prefix}docs/a.md?raw=1"));
    assert_eq!(raw.headers["content-type"], "text/plain; charset=utf-8");
    assert!(raw.body.starts_with("# 本文".as_bytes()));
    let rendered = hub.get(&format!("{prefix}docs/report.MD.TXT"));
    assert_eq!(rendered.status, 200);
    let html = String::from_utf8(rendered.body).unwrap();
    assert!(html.contains("<h1>二重拡張子の本文</h1>"));
    assert!(html.contains(&format!(
        "href=\"http://127.0.0.1:{}/#page=rooms&amp;room=EXAMPLE%2F2026-11-03-%E8%AA%BF%E6%9F%BB",
        hub.port
    )));
    assert!(html.contains("file=docs%2Freport.MD.TXT"));
    assert!(html.contains("target=\"_blank\" rel=\"noopener noreferrer\""));
    let raw = hub.get(&format!("{prefix}docs/report.MD.TXT?raw=1"));
    assert_eq!(raw.headers["content-type"], "text/plain; charset=utf-8");
    assert_eq!(String::from_utf8(raw.body).unwrap(), "# 二重拡張子の本文\n");
    for suffix in ["../secret", "%2e%2e/secret", "docs%2Fa.md", "NUL", ".done", "a.txt:stream", "a%5cb", "a%20"] {
        assert_eq!(hub.get(&format!("{prefix}{suffix}")).status, 404, "{suffix}");
    }
    for target in ["/", "/api/hello", &format!("{prefix}docs/a.md")] {
        assert_eq!(hub.request("GET", target, Some("evil.example")).status, 421);
    }
}

#[test]
fn initial_filter_cwd_file_and_missing_root_are_preserved() {
    let env = Env::new();
    let cwd = env.base.join("cwd.txt");
    fs::write(&cwd, "そのまま\n").unwrap();
    let hub = env.start(&["--cwd-file", cwd.to_str().unwrap()]);
    assert!(hub.json("/api/rooms")["rooms"].as_array().unwrap().is_empty());
    assert!(!env.root.exists());
    assert_eq!(fs::read_to_string(cwd).unwrap(), "そのまま\n");
    drop(hub);
    fixture(&env);
    fs::write(env.base.join("config.toml"), "hub_port = 1\nrecent_days = 3\n").unwrap();
    let hub = env.start(&["ハブ", "-o", "--days", "2"]);
    let config = hub.json("/api/config");
    assert_eq!(
        config["filter"],
        json!({"query":"ハブ", "status":"active", "period":true, "days":2, "range":"before", "scope":"all"})
    );
    assert_eq!(hub.json("/api/rooms")["rooms"].as_array().unwrap().len(), 1);
    assert_eq!(hub.json("/api/rooms?q=一致なし")["rooms"].as_array().unwrap().len(), 0);
}

#[test]
fn room_and_cross_room_question_apis_share_independent_filters() {
    let env = Env::new();
    fs::write(env.base.join("config.toml"), "day_start_hour = 23\nrecent_days = 2\n").unwrap();
    let today = (chrono::Local::now() - chrono::TimeDelta::hours(23)).date_naive();
    for (age, done, slug) in
        [(0, false, "new-open"), (2, true, "edge-done"), (3, false, "old-open"), (3, true, "old-done")]
    {
        let room = env.root.join(format!("{}-{slug}", today - chrono::TimeDelta::days(age)));
        fs::create_dir_all(&room).unwrap();
        fs::write(room.join("README.md"), "# 共通の説明\n").unwrap();
        if done {
            fs::write(room.join(".done"), "").unwrap();
        }
    }
    let before = snapshot(&env.base);
    let hub = env.start(&[]);
    assert_eq!(
        hub.json("/api/config")["filter"],
        json!({"query":null, "status":"active", "period":true, "days":2, "range":"within", "scope":"filtered"})
    );
    let cases = [
        ("", vec!["new-open"]),
        ("status=done", vec!["edge-done"]),
        ("status=all", vec!["new-open", "edge-done"]),
        ("status=all&period=0", vec!["new-open", "edge-done", "old-open", "old-done"]),
        ("status=done&range=before", vec!["old-done"]),
        ("status=active&range=before", vec!["old-open"]),
        ("q=共通&scope=filtered", vec!["new-open"]),
        ("q=共通&scope=all", vec!["new-open", "edge-done", "old-open", "old-done"]),
        ("q=共通&scope=all&status=done&range=before", vec!["new-open", "edge-done", "old-open", "old-done"]),
        ("status=done&period=0&days=9223372036854775807", vec!["edge-done", "old-done"]),
        ("scope=all&days=9223372036854775807", vec!["new-open", "edge-done", "old-open", "old-done"]),
    ];
    for (params, expected) in cases {
        let data = hub.json(&format!("/api/rooms?{params}"));
        let actual: std::collections::BTreeSet<_> =
            data["rooms"].as_array().unwrap().iter().map(|room| room["title"].as_str().unwrap()).collect();
        assert_eq!(actual, expected.into_iter().collect(), "{params}");
        let questions = hub.json(&format!("/api/questions/all?{params}"));
        let room_labels: std::collections::BTreeSet<_> =
            data["rooms"].as_array().unwrap().iter().map(|room| room["room"].as_str().unwrap()).collect();
        assert_eq!(
            questions["rooms"]
                .as_array()
                .unwrap()
                .iter()
                .map(|room| room["room"].as_str().unwrap())
                .collect::<std::collections::BTreeSet<_>>(),
            room_labels,
            "{params}"
        );
    }
    assert_eq!(snapshot(&env.base), before);
}

#[test]
fn orphan_answers_and_broken_questions_still_have_an_archive() {
    let env = Env::new();
    let room = fixture(&env);
    fs::write(room.join("QUESTIONS.jsonl"), "壊れた行\n").unwrap();
    let hub = env.start(&["-a"]);
    let data = hub.json("/api/questions/all");
    let question = &data["rooms"][0]["questions"][0];
    assert!(question["ask"].is_null());
    assert_eq!(question["answers"][0]["question"]["question"], "未反映の質問");
    assert_eq!(question["state"], "pending");
}

#[test]
fn occupied_port_reuses_only_a_hub_for_the_same_root() {
    let env = Env::new();
    let hub = env.start(&[]);
    fixture(&env);
    let output = env.run_hub(hub.port);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), format!("http://127.0.0.1:{}/", hub.port));
    assert!(String::from_utf8_lossy(&output.stderr).contains("既に起動"));
    let different = Env::new();
    let output = different.run_hub(hub.port);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--port"));
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut request = [0; 1024];
        let _ = stream.read(&mut request);
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}");
    });
    let output = env.run_hub(port);
    server.join().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--port"));
}

const ANSWER_ROOM: &str = "EXAMPLE/2026-11-03-調査";

fn submission(id: &str, kind: &str, text: &str) -> Value {
    json!({"room": ANSWER_ROOM, "answers": [{"id": id, "kind": kind, "text": text}]})
}

fn assert_answer_error(response: &Response, status: u16, id: Option<&str>, token: &str) -> Value {
    assert_eq!(response.status, status, "{}", String::from_utf8_lossy(&response.body));
    assert_eq!(response.headers["content-type"], "application/json; charset=utf-8");
    assert_eq!(response.headers["cache-control"], "no-store");
    assert_eq!(response.headers["cross-origin-resource-policy"], "same-origin");
    assert!(!response.headers.keys().any(|key| key.starts_with("access-control-allow-")));
    assert!(!String::from_utf8_lossy(&response.body).contains(token));
    let error: Value = serde_json::from_slice(&response.body).unwrap();
    assert!(error["error"].as_str().is_some_and(|message| !message.is_empty()));
    if let Some(id) = id {
        assert_eq!(error["id"], id);
    }
    error
}

#[test]
fn answers_append_snapshots_normalize_text_and_reopen_resolved_questions() {
    let env = Env::new();
    let room = fixture(&env);
    let before = snapshot(&env.base);
    let previous = fs::read(room.join("ANSWERS.jsonl")).unwrap();
    let hub = env.start(&["-a"]);
    let token = hub.token();
    // 読み取りキャッシュを先に作ってから、追記直後のモデルが更新されることも確認する。
    let target = format!("/api/questions?room={}", encode(ANSWER_ROOM));
    assert_eq!(hub.json(&target)["questions"][2]["state"], "resolved");
    let body = json!({"room": ANSWER_ROOM, "answers": [
        {"id":"Q-20261106-01", "kind":"answer", "text":"\r\n \t\r\n  日本語 <タグ> & \\\"\r\n\r次の行  \r\n\t\n"},
        {"id":"Q-20261106-02", "kind":"answer", "text":"追加の回答"},
        {"id":"Q-20261106-03", "kind":"not_needed", "text":""}
    ]});
    let response = hub.post_answers(&token, body);
    assert_eq!(response.status, 200);
    assert_eq!(response.headers["cache-control"], "no-store");
    assert_eq!(response.headers["cross-origin-resource-policy"], "same-origin");
    let sent: Value = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(sent["sent"], json!(["Q-20261106-01", "Q-20261106-02", "Q-20261106-03"]));
    assert_eq!(sent["questions"], hub.json(&target));
    assert!(sent["questions"]["questions"].as_array().unwrap()[..3].iter().all(|q| q["state"] == "pending"));
    let bytes = fs::read(room.join("ANSWERS.jsonl")).unwrap();
    assert!(bytes.starts_with(&previous));
    assert!(!bytes.contains(&b'\r'));
    assert!(!bytes.starts_with(b"\xef\xbb\xbf"));
    let added = std::str::from_utf8(&bytes[previous.len()..]).unwrap();
    assert!(added.contains("日本語"));
    assert!(!added.contains("\\u"));
    let records: Vec<Value> = added.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(records.len(), 3);
    assert_eq!(records[0]["v"], 1);
    assert_eq!(records[0]["type"], "answer");
    assert_eq!(records[0]["n"], 1);
    assert_eq!(records[1]["n"], 2);
    assert_eq!(records[0]["text"], "  日本語 <タグ> & \\\"\n\n次の行  ");
    assert_eq!(records[2]["kind"], "not_needed");
    assert_eq!(records[2]["text"], "");
    assert_eq!(records[0]["question"]["question"], "未回答の質問");
    assert_eq!(records[0]["question"]["choices"], json!(["表", "文章"]));
    assert_eq!(records[0]["question"]["recommend"], 1);
    assert_eq!(records[0]["question"]["by"], "エージェント");
    assert_eq!(records[0]["question"]["asked_at"], "2026-11-06T12:00:00+09:00");
    let at = records[0]["at"].as_str().unwrap();
    assert_eq!(at.len(), 25);
    chrono::DateTime::parse_from_rfc3339(at).unwrap();
    assert!(records.iter().all(|record| record["at"] == at));
    let mut after = snapshot(&env.base);
    after.insert(room.join("ANSWERS.jsonl").strip_prefix(&env.base).unwrap().into(), previous);
    assert_eq!(after, before);
    let output = env.command().args(["q", "pending", "--room", ANSWER_ROOM]).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(String::from_utf8_lossy(&output.stdout).contains("追加の回答"));
}

#[test]
fn answer_security_rejects_missing_wrong_and_duplicate_headers_without_writing() {
    let env = Env::new();
    let room = fixture(&env);
    fs::remove_file(room.join("ANSWERS.jsonl")).unwrap();
    let hub = env.start(&[]);
    let token = hub.token();
    let bytes = serde_json::to_vec(&submission("Q-20261106-01", "answer", "回答")).unwrap();
    for existing in [false, true] {
        if existing {
            fs::write(room.join("ANSWERS.jsonl"), "壊れた行\n").unwrap();
        }
        let before = snapshot(&env.base);
        for (name, value, status) in [
            ("Origin", None, 403),
            ("Origin", Some(""), 403),
            ("Origin", Some("null"), 403),
            ("Origin", Some("http://127.0.0.1:1"), 403),
            ("Origin", Some("http://evil.example"), 403),
            ("Origin", Some("https://localhost:1"), 403),
            ("X-Workroom-Token", None, 403),
            ("X-Workroom-Token", Some(""), 403),
            ("X-Workroom-Token", Some("wrong"), 403),
            ("Content-Type", None, 403),
            ("Content-Type", Some("text/plain"), 403),
            ("Content-Type", Some("application/jsonp"), 403),
            ("Sec-Fetch-Site", Some("cross-site"), 403),
            ("Sec-Fetch-Site", Some("same-site"), 403),
            ("Host", Some("evil.example"), 421),
        ] {
            let mut headers = hub.answer_headers(&token, &bytes);
            headers.retain(|(key, _)| key != name);
            if let Some(value) = value {
                headers.push((name.into(), value.into()));
            }
            let response = hub.raw_request("POST", "/api/answers", &headers, &bytes);
            assert_answer_error(&response, status, None, &token);
            assert_eq!(snapshot(&env.base), before, "{name}: {value:?}");
        }
        for name in ["Origin", "Content-Type", "X-Workroom-Token", "Sec-Fetch-Site", "Host"] {
            let mut headers = hub.answer_headers(&token, &bytes);
            let value = if name == "Host" {
                format!("localhost:{}", hub.port)
            } else {
                headers.iter().find(|(key, _)| key == name).unwrap().1.clone()
            };
            if name == "Host" {
                headers.push(("Host".into(), format!("127.0.0.1:{}", hub.port)));
            }
            headers.push((name.into(), value));
            let response = hub.raw_request("POST", "/api/answers", &headers, &bytes);
            assert_answer_error(&response, if name == "Host" { 421 } else { 403 }, None, &token);
            assert_eq!(snapshot(&env.base), before);
        }
        // 別ホストだが実際のポートを持つ Origin も拒否する。
        let mut headers = hub.answer_headers(&token, &bytes);
        headers[0].1 = format!("http://evil.example:{}", hub.port);
        assert_answer_error(&hub.raw_request("POST", "/api/answers", &headers, &bytes), 403, None, &token);
        assert_eq!(snapshot(&env.base), before);
    }
    let options = hub.request("OPTIONS", "/api/answers", None);
    assert_eq!(options.status, 405);
    assert!(!options.headers.keys().any(|key| key.starts_with("access-control-allow-")));
    let mut headers = hub.answer_headers(&token, &bytes);
    headers[0].1 = format!("http://localhost:{}", hub.port);
    headers[1].1 = "Application/JSON; charset=UTF-8".into();
    headers.retain(|(key, _)| key != "Sec-Fetch-Site");
    assert_eq!(hub.raw_request("POST", "/api/answers", &headers, &bytes).status, 200);
}

#[test]
fn invalid_answer_batches_are_rejected_atomically_with_ids_and_current_withdrawal() {
    let env = Env::new();
    let room = fixture(&env);
    let duplicate =
        json!({"v":1,"type":"ask","id":"Q-20261106-02","at":"2026-11-06T12:00:00+09:00","question":"重複した質問"});
    let mut file = fs::OpenOptions::new().append(true).open(room.join("QUESTIONS.jsonl")).unwrap();
    writeln!(file, "{duplicate}").unwrap();
    drop(file);
    let hub = env.start(&[]);
    let token = hub.token();
    let before = snapshot(&env.base);
    for (answers, status, id) in [
        (json!([]), 422, None),
        (json!([{"id":"Q-20261106-99","kind":"answer","text":"回答"}]), 422, Some("Q-20261106-99")),
        (json!([{"kind":"answer","text":"回答"}]), 422, Some("")),
        (json!([{"id":"Q-20261106-02","kind":"answer","text":"回答"}]), 422, Some("Q-20261106-02")),
        (json!([{"id":"Q-20261106-04","kind":"answer","text":"回答"}]), 409, Some("Q-20261106-04")),
        (json!([{"id":"Q-20261106-01","kind":"other","text":"回答"}]), 422, Some("Q-20261106-01")),
        (json!([{"id":"Q-20261106-01","kind":"answer","text":"\r\n \t\r\n"}]), 422, Some("Q-20261106-01")),
        (json!([{"id":"Q-20261106-01","kind":"answer","text":"あ".repeat(21846)}]), 413, Some("Q-20261106-01")),
        (json!([{"id":"Q-20261106-01","kind":"not_needed","text":" " .repeat(65537)}]), 413, Some("Q-20261106-01")),
        (json!([{"id":"Q-20261106-01","kind":"answer","text":42}]), 400, Some("Q-20261106-01")),
        (json!([{"id":"Q-20261106-01","kind":"answer"}]), 400, Some("Q-20261106-01")),
    ] {
        let response = hub.post_answers(&token, json!({"room": ANSWER_ROOM, "answers": answers}));
        let error = assert_answer_error(&response, status, id, &token);
        if status == 409 {
            assert_eq!(error["state"], "withdrawn");
            assert_eq!(error["reason"], "前提の変更");
            assert_eq!(error["successor"], "Q-20261106-01");
        }
        assert_eq!(snapshot(&env.base), before);
        // 正常な回答が先にあっても、後の1件が通らなければ何も追記しない。
        if !answers.as_array().unwrap().is_empty() {
            let mut mixed = vec![json!({"id":"Q-20261106-03","kind":"answer","text":"先の正常な回答"})];
            mixed.extend(answers.as_array().unwrap().iter().cloned());
            assert_answer_error(
                &hub.post_answers(&token, json!({"room":ANSWER_ROOM,"answers":mixed})),
                status,
                id,
                &token,
            );
            assert_eq!(snapshot(&env.base), before);
        }
    }
    let answer = json!({"id":"Q-20261106-01","kind":"answer","text":"回答"});
    assert_answer_error(
        &hub.post_answers(&token, json!({"room": ANSWER_ROOM, "answers": [answer,answer]})),
        422,
        Some("Q-20261106-01"),
        &token,
    );
    assert_eq!(snapshot(&env.base), before);
    for body in
        [json!({}), json!([]), json!({"room":ANSWER_ROOM,"answers":{}}), json!({"room":ANSWER_ROOM,"answers":[null]})]
    {
        assert_answer_error(&hub.post_answers(&token, body), 400, None, &token);
        assert_eq!(snapshot(&env.base), before);
    }
    for label in ["2026-11-03-missing", "../2026-11-03-room", ""] {
        assert_answer_error(&hub.post_answers(&token, json!({"room":label,"answers":[answer]})), 404, None, &token);
        assert_eq!(snapshot(&env.base), before);
    }
}

#[test]
fn answer_framing_errors_are_json_and_host_is_checked_before_body() {
    let env = Env::new();
    fixture(&env);
    let hub = env.start(&[]);
    let token = hub.token();
    let before = snapshot(&env.base);
    for (header, status) in [
        (None, 411),
        (Some(("Content-Length", "1048577")), 413),
        (Some(("Content-Length", "1048576")), 413),
        (Some(("Content-Length", "2")), 400),
        (Some(("Content-Length", "-1")), 400),
        (Some(("Transfer-Encoding", "chunked")), 400),
    ] {
        let mut headers = hub.answer_headers(&token, &[]);
        headers.retain(|(key, _)| key != "Content-Length");
        if let Some((key, value)) = header {
            headers.push((key.into(), value.into()));
        }
        assert_answer_error(&hub.raw_request("POST", "/api/answers", &headers, &[]), status, None, &token);
        headers.push(("Host".into(), "evil.example".into()));
        assert_answer_error(&hub.raw_request("POST", "/api/answers", &headers, &[]), 421, None, &token);
        assert_eq!(snapshot(&env.base), before);
    }
    for bytes in [b"{".as_slice(), b"\xff", b""] {
        assert_answer_error(
            &hub.raw_request("POST", "/api/answers", &hub.answer_headers(&token, bytes), bytes),
            400,
            None,
            &token,
        );
    }
    let mut headers = hub.answer_headers(&token, &[]);
    headers.push(("Content-Length".into(), "0".into()));
    assert_answer_error(&hub.raw_request("POST", "/api/answers", &headers, &[]), 400, None, &token);
    assert_eq!(hub.request("POST", "/api/missing", None).status, 405);
    assert_eq!(snapshot(&env.base), before);
}

#[test]
fn tokens_are_stable_per_hub_different_on_startup_and_absent_from_apis() {
    let env = Env::new();
    fixture(&env);
    let hub = env.start(&["-a"]);
    let token = hub.token();
    assert_eq!(token, hub.token());
    let other = env.start(&[]);
    assert_ne!(token, other.token());
    for target in [
        "/api/hello".into(),
        "/api/config".into(),
        "/api/rooms".into(),
        "/api/questions/all".into(),
        format!("/api/questions?room={}", encode(ANSWER_ROOM)),
        format!("/api/files?room={}", encode(ANSWER_ROOM)),
    ] {
        let response = hub.get(&target);
        assert_eq!(response.status, 200);
        assert!(!String::from_utf8_lossy(&response.body).contains(&token));
        assert!(!response.headers.values().any(|value| value.contains(&token)));
        assert!(!response.headers.keys().any(|key| key.starts_with("access-control-allow-")));
    }
    let response = hub.post_answers(&token, submission("Q-20261106-01", "answer", "回答"));
    assert_eq!(response.status, 200);
    assert!(!String::from_utf8_lossy(&response.body).contains(&token));
    assert!(!response.headers.values().any(|value| value.contains(&token)));
}
