//! ディスクに何も生成しない、読み取り専用のローカル成果物ハブ。

mod http;
mod listing;
mod markdown;
mod paths;
mod routes;

use crate::{config::Settings, shell};
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct Options {
    pub query: Option<String>,
    pub all: bool,
    pub old: bool,
    pub days: Option<i64>,
    pub port: Option<u16>,
    pub no_open: bool,
}

pub struct State {
    pub settings: Settings,
    pub port: u16,
    pub root_hash: String,
    pub initial: listing::Filter,
    pub cache: Mutex<listing::Cache>,
}

fn root_hash(settings: &Settings) -> Result<String> {
    use std::fmt::Write;
    let absolute = std::path::absolute(&settings.root)?;
    // 未作成のルートも、存在する最も近い祖先から正規化する。
    // Windows の verbatim prefix をそろえ、後からルートができても同じハブと判定する。
    let root = absolute
        .ancestors()
        .find_map(|ancestor| {
            ancestor.canonicalize().ok().map(|base| {
                let suffix = absolute.strip_prefix(ancestor).unwrap();
                if suffix.as_os_str().is_empty() { base } else { base.join(suffix) }
            })
        })
        .unwrap_or(absolute);
    let mut root = root.to_string_lossy().into_owned();
    if cfg!(windows) {
        root.make_ascii_lowercase();
    }
    Ok(Sha256::digest(root.as_bytes()).iter().fold(String::with_capacity(64), |mut output, byte| {
        let _ = write!(output, "{byte:02x}");
        output
    }))
}

fn announce(port: u16, no_open: bool) -> Result<()> {
    let url = format!("http://127.0.0.1:{port}/");
    {
        let mut output = std::io::stdout().lock();
        writeln!(output, "{url}")?;
        output.flush()?;
    }
    if !no_open && let Err(error) = shell::open_with_os(OsStr::new(&url)) {
        eprintln!("ブラウザを開けませんでした。上の URL を開いてください: {error:#}");
    }
    Ok(())
}

pub fn run(settings: Settings, options: Options) -> Result<()> {
    let port = options.port.unwrap_or(settings.config.hub_port);
    let hash = root_hash(&settings)?;
    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse && port != 0 => {
            if existing_hub(port, &hash) {
                announce(port, options.no_open)?;
                eprintln!("同じ作業ルートのハブが既に起動しています");
                return Ok(());
            }
            bail!("ポート {port} は使用中です。別の --port N を指定してください");
        }
        Err(error) => return Err(error).context("ハブの待ち受けを開始できません"),
    };
    let port = listener.local_addr()?.port();
    let initial = listing::Filter {
        query: options.query,
        mode: if options.old {
            "old"
        } else if options.all {
            "all"
        } else {
            "recent"
        }
        .into(),
        days: options.days.unwrap_or(settings.config.recent_days),
    };
    let state = Arc::new(State { settings, port, root_hash: hash, initial, cache: Mutex::default() });
    announce(port, options.no_open)?;
    eprintln!("成果物ハブを起動しました。Ctrl+C で終了します（読むだけ）");
    serve(listener, state)
}

fn existing_hub(port: u16, hash: &str) -> bool {
    let probe = || -> Result<bool> {
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        write!(stream, "GET /api/hello HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")?;
        let mut bytes = Vec::new();
        stream.take(64 * 1024).read_to_end(&mut bytes)?;
        let text = std::str::from_utf8(&bytes)?;
        let Some((headers, body)) = text.split_once("\r\n\r\n") else { return Ok(false) };
        if !headers.starts_with("HTTP/1.1 200 ") {
            return Ok(false);
        }
        let value: serde_json::Value = serde_json::from_str(body)?;
        Ok(value["app"] == "workroom" && value["root_hash"] == hash)
    };
    probe().unwrap_or(false)
}

struct Connection(Arc<AtomicUsize>);

fn try_connection(active: &Arc<AtomicUsize>) -> Option<Connection> {
    active
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < 32).then_some(n + 1))
        .ok()
        .map(|_| Connection(Arc::clone(active)))
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn serve(listener: TcpListener, state: Arc<State>) -> Result<()> {
    let active = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let mut stream = stream.context("接続を受け付けられません")?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.set_write_timeout(Some(Duration::from_secs(60)))?;
        let Some(connection) = try_connection(&active) else {
            let _ = http::write_response(&mut stream, http::Response::error(503));
            continue;
        };
        let state = Arc::clone(&state);
        if let Err(error) = std::thread::Builder::new().name("workroom-hub".into()).spawn(move || {
            let _connection = connection;
            let response = match http::read_request(&stream) {
                Ok(request) => routes::handle(&state, &request),
                Err(status) => http::Response::error(status),
            };
            if let Err(error) = http::write_response(&mut stream, response) {
                eprintln!("ハブの応答を送れません: {error}");
            }
        }) {
            eprintln!("ハブの処理を開始できません: {error}");
        }
    }
    Ok(())
}

#[cfg(test)]
fn test_state(root: &std::path::Path) -> State {
    State {
        settings: Settings {
            config: crate::config::Config::default(),
            config_path: root.join("config.toml"),
            root: root.into(),
            state_dir: root.join("state"),
            claude_dir: root.join("claude"),
        },
        port: 49123,
        root_hash: "test-root".into(),
        initial: listing::Filter { query: None, mode: "all".into(), days: 14 },
        cache: Mutex::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_limit_releases_slots_on_drop() {
        let active = Arc::new(AtomicUsize::new(0));
        let mut connections: Vec<_> = (0..32).map(|_| try_connection(&active).unwrap()).collect();
        assert!(try_connection(&active).is_none());
        assert_eq!(active.load(Ordering::Acquire), 32);
        connections.pop();
        let restored = try_connection(&active).unwrap();
        drop(connections);
        drop(restored);
        assert_eq!(active.load(Ordering::Acquire), 0);
    }

    #[test]
    fn root_identity_is_stable_when_the_root_is_created_after_startup() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = test_state(dir.path());
        state.settings.root = dir.path().join("nested").join("rooms");
        let before = root_hash(&state.settings).unwrap();
        std::fs::create_dir_all(&state.settings.root).unwrap();
        assert_eq!(before, root_hash(&state.settings).unwrap());
    }
}
