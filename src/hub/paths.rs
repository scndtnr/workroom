//! URL の要素を検査し、部屋の中の実在するパスだけを返す。

use super::http;
use crate::room;
use std::path::{Path, PathBuf};

pub fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('.')
        && !value.ends_with(['.', ' '])
        && !value.chars().last().is_some_and(char::is_whitespace)
        && !value.chars().any(|c| c.is_control() || matches!(c, '/' | '\\' | ':'))
        && !room::is_windows_reserved_name(value)
}

pub fn components(encoded: &str) -> Result<Vec<String>, u16> {
    encoded
        .split('/')
        .map(|part| {
            let value = http::decode(part)?;
            if valid_component(&value) { Ok(value) } else { Err(404) }
        })
        .collect()
}

/// クエリの room はデコード済み。全要素を同じ規則で検査する。
pub fn room_path(root: &Path, label: &str) -> Result<PathBuf, u16> {
    let parts: Vec<_> = label.split('/').collect();
    if !parts.iter().all(|p| valid_component(p)) {
        return Err(404);
    }
    room::locate(root, &parts).and_then(|path| path.canonicalize().ok()).ok_or(404)
}

pub fn file_path(root: &Path, encoded: &str) -> Result<PathBuf, u16> {
    let parts = components(encoded)?;
    let first = parts.first().ok_or(404u16)?;
    let room_len = if room::parse_room_name(first).is_some() { 1 } else { 2 };
    if parts.len() <= room_len {
        return Err(404);
    }
    let label = parts[..room_len].join("/");
    let room = room_path(root, &label)?;
    let mut path = room.clone();
    for part in &parts[room_len..] {
        path.push(part);
    }
    let path = path.canonicalize().map_err(|_| 404u16)?;
    if path.starts_with(&room) && path.is_file() { Ok(path) } else { Err(404) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn rejects_traversal_hidden_and_windows_aliases() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("2026-11-02-room")).unwrap();
        for path in [
            "../x",
            "%2e%2e/x",
            "..%5c/x",
            "%2F/x",
            "C:/x",
            r"\\server/x",
            "a.txt:stream/x",
            "NUL/x",
            "con.txt/x",
            "a./x",
            "a%20/x",
            ".hidden/x",
            "_workroom/2026-11-02-room/x",
            "missing/x",
            "2026-11-02-room/../x",
            "2026-11-02-room/.hidden",
            "2026-11-02-room/a%2Fb",
            "2026-11-02-room/%00",
            "2026-11-02-room//x",
        ] {
            assert_eq!(file_path(dir.path(), path), Err(404), "{path}");
        }
        assert_eq!(file_path(dir.path(), "%ff/x"), Err(400));
    }

    #[test]
    fn resolves_project_and_standalone_rooms_and_decodes_only_once() {
        let dir = tempfile::tempdir().unwrap();
        for label in ["2026-11-02-room", "EXAMPLE/2026-11-03-調査"] {
            let room = dir.path().join(label);
            fs::create_dir_all(room.join("docs")).unwrap();
            fs::write(room.join("docs/a+b.md"), "本文").unwrap();
            assert_eq!(
                file_path(dir.path(), &format!("{label}/docs/a+b.md")).unwrap(),
                room.join("docs/a+b.md").canonicalize().unwrap()
            );
            fs::write(room.join("%2e%2e"), "名前にパーセントを含む").unwrap();
            assert!(file_path(dir.path(), &format!("{label}/%252e%252e")).is_ok());
            assert!(room_path(dir.path(), label).is_ok());
        }
        assert!(file_path(dir.path(), "EXAMPLE/2026-11-03-%E8%AA%BF%E6%9F%BB/docs/a+b.md").is_ok());
        assert_eq!(room_path(dir.path(), "EXAMPLE/2026-11-03-調査/docs"), Err(404));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn canonicalization_rejects_links_outside_the_room() {
        let dir = tempfile::tempdir().unwrap();
        let room = dir.path().join("2026-11-02-room");
        fs::create_dir(&room).unwrap();
        fs::write(dir.path().join("outside.txt"), "外").unwrap();
        fs::write(room.join("inside.txt"), "内").unwrap();
        #[cfg(unix)]
        let symlink = |from: &Path, to: &Path| std::os::unix::fs::symlink(from, to);
        #[cfg(windows)]
        let symlink = |from: &Path, to: &Path| std::os::windows::fs::symlink_file(from, to);
        if let Err(error) = symlink(&dir.path().join("outside.txt"), &room.join("outside-link")) {
            #[cfg(windows)]
            if error.kind() == std::io::ErrorKind::PermissionDenied || error.raw_os_error() == Some(1314) {
                eprintln!("シンボリックリンクの作成権限がないためスキップ: {error}");
                return;
            }
            panic!("シンボリックリンクを作れません: {error}");
        }
        symlink(&room.join("inside.txt"), &room.join("inside-link")).unwrap();
        assert_eq!(file_path(dir.path(), "2026-11-02-room/outside-link"), Err(404));
        assert!(file_path(dir.path(), "2026-11-02-room/inside-link").is_ok());
    }
}
