//! 一時ディレクトリと生の HTTP だけで、読むだけのハブを検証する。

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
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, self.port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        let host =
            host.map(|h| format!("Host: {h}\r\n")).unwrap_or_else(|| format!("Host: 127.0.0.1:{}\r\n", self.port));
        write!(stream, "{method} {target} HTTP/1.1\r\n{host}Connection: close\r\n\r\n").unwrap();
        let mut bytes = Vec::new();
        stream.take(8 * 1024 * 1024).read_to_end(&mut bytes).unwrap();
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
    assert_eq!(hub.request("POST", "/api/answers", None).status, 405);
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
    assert_eq!(config["filter"], json!({"query":"ハブ", "mode":"old", "days":2}));
    assert_eq!(hub.json("/api/rooms")["rooms"].as_array().unwrap().len(), 1);
    assert_eq!(hub.json("/api/rooms?q=一致なし")["rooms"].as_array().unwrap().len(), 0);
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
