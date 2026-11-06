//! 一覧・確認事項の読み取りと、ファイルの更新日時・サイズによるメモリ内キャッシュ。

use super::{State, paths};
use crate::{fsinfo, ls, questions, room};
use anyhow::Result;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Filter {
    pub query: Option<String>,
    pub mode: String,
    pub days: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    size: u64,
}

fn stamp(path: &Path) -> Option<Stamp> {
    fs::metadata(path).ok().map(|m| Stamp { modified: m.modified().ok(), size: m.len() })
}

type Key = [Option<Stamp>; 3];
type Inventory = Vec<(PathBuf, Option<Stamp>, bool)>;

struct Selection {
    filter: Filter,
    today: chrono::NaiveDate,
    inventory: Inventory,
    rooms: Vec<room::Room>,
    label: String,
}

#[derive(Default)]
pub struct Cache {
    selection: Option<Selection>,
    rooms: HashMap<PathBuf, (Key, Arc<RoomData>)>,
}

pub struct RoomData {
    pub description: String,
    pub status: Option<String>,
    pub modified: Option<u64>,
    pub questions: Vec<questions::Question>,
    pub warnings: Vec<questions::Warning>,
    pub error: Option<String>,
}

fn timestamp(time: SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH).ok().map(|t| t.as_secs())
}

fn inventory(root: &Path) -> Inventory {
    let mut result = Vec::new();
    let Ok(read) = fs::read_dir(root) else { return result };
    for entry in read.filter_map(|entry| entry.ok()) {
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if room::parse_room_name(&name).is_some() {
            let path = entry.path().canonicalize().unwrap_or_else(|_| entry.path());
            result.push((path.clone(), stamp(&path.join(room::README)), path.join(room::DONE_MARKER).exists()));
        } else if !name.starts_with(['.', '_']) {
            let Ok(children) = fs::read_dir(entry.path()) else { continue };
            for child in children.filter_map(|entry| entry.ok()) {
                if child.file_type().is_ok_and(|t| t.is_dir())
                    && room::parse_room_name(&child.file_name().to_string_lossy()).is_some()
                {
                    let path = child.path().canonicalize().unwrap_or_else(|_| child.path());
                    result.push((path.clone(), stamp(&path.join(room::README)), path.join(room::DONE_MARKER).exists()));
                }
            }
        }
    }
    result.sort_by(|a, b| a.0.cmp(&b.0));
    result
}

pub fn selected(state: &State, filter: &Filter) -> Result<(Vec<room::Room>, String)> {
    let delta = chrono::TimeDelta::try_days(filter.days).ok_or_else(|| anyhow::anyhow!("日数が大きすぎます"))?;
    let today = room::today(&state.settings);
    anyhow::ensure!(today.checked_sub_signed(delta).is_some(), "日数が大きすぎます");
    let current = inventory(&state.settings.root);
    {
        let cache = state.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(selection) = &cache.selection
            && selection.filter == *filter
            && selection.today == today
            && selection.inventory == current
        {
            return Ok((selection.rooms.clone(), selection.label.clone()));
        }
    }
    let (rooms, label) = ls::select_rooms(
        &state.settings,
        &ls::Filter {
            query: filter.query.as_deref(),
            all: filter.mode == "all",
            old: filter.mode == "old",
            days: Some(filter.days),
        },
    )?;
    let mut cache = state.cache.lock().unwrap_or_else(|e| e.into_inner());
    cache.rooms.retain(|path, _| current.iter().any(|entry| &entry.0 == path));
    cache.selection = Some(Selection {
        filter: filter.clone(),
        today,
        inventory: current,
        rooms: rooms.clone(),
        label: label.clone(),
    });
    Ok((rooms, label))
}

/// 既存の questions::read に、部屋の外を指すファイルを渡さない。
fn contained_files(path: &Path) -> Result<()> {
    let canonical = path.canonicalize()?;
    for name in [room::README, questions::QUESTIONS, questions::ANSWERS] {
        let file = path.join(name);
        if fs::symlink_metadata(&file).is_ok() {
            anyhow::ensure!(file.canonicalize()?.starts_with(&canonical), "部屋の外を指すファイルは読めません: {name}");
        }
    }
    Ok(())
}

pub fn room_data(state: &State, path: &Path) -> Arc<RoomData> {
    // 一覧のパスと API の canonicalize 済みのパスでキャッシュを共有する。
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let path = canonical.as_path();
    let key = [
        stamp(&path.join(room::README)),
        stamp(&path.join(questions::QUESTIONS)),
        stamp(&path.join(questions::ANSWERS)),
    ];
    {
        let cache = state.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((old_key, data)) = cache.rooms.get(path)
            && old_key == &key
        {
            return Arc::clone(data);
        }
    }
    let modified = key.iter().flatten().filter_map(|s| s.modified).max().and_then(timestamp);
    let mut data = RoomData {
        description: String::new(),
        status: None,
        modified,
        questions: Vec::new(),
        warnings: Vec::new(),
        error: None,
    };
    let read = || -> Result<(String, questions::Questions)> {
        contained_files(path)?;
        let mut head = Vec::new();
        match fs::File::open(path.join(room::README)) {
            Ok(file) => {
                // 巨大な README でも一覧では先頭だけを読む。
                file.take(64 * 1024).read_to_end(&mut head)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok((String::from_utf8_lossy(&head).into_owned(), questions::read(path)?))
    };
    match read() {
        Ok((head, questions)) => {
            data.description = head
                .lines()
                .next()
                .unwrap_or_default()
                .trim_start_matches('\u{feff}')
                .trim()
                .trim_start_matches('#')
                .trim()
                .into();
            data.status = head
                .trim_start_matches('\u{feff}')
                .lines()
                .take(10)
                .find_map(|line| line.trim().strip_prefix("- status:").map(|s| s.trim().to_string()));
            data.questions = questions.items.into_values().collect();
            data.warnings = questions.warnings;
        }
        Err(error) => data.error = Some(format!("読み取れません: {error:#}")),
    }
    let data = Arc::new(data);
    let mut cache = state.cache.lock().unwrap_or_else(|e| e.into_inner());
    if data.error.is_none() {
        cache.rooms.insert(path.to_path_buf(), (key, Arc::clone(&data)));
    } else {
        cache.rooms.remove(path);
    }
    data
}

pub fn summary(state: &State, room: &room::Room) -> Value {
    let data = room_data(state, &room.path);
    let unanswered = data.questions.iter().filter(|q| q.state == questions::State::Unanswered).count();
    let pending = data.questions.iter().filter(|q| q.state == questions::State::Pending).count();
    let status = if room.done {
        "完了"
    } else if unanswered > 0 {
        "ユーザー待ち"
    } else if pending > 0 {
        "エージェント待ち"
    } else {
        data.status.as_deref().unwrap_or("不明")
    };
    let description = if data.description == room.slug { "" } else { &data.description };
    let readme = room.path.join(room::README).is_file()
        && room
            .path
            .canonicalize()
            .is_ok_and(|base| room.path.join(room::README).canonicalize().is_ok_and(|path| path.starts_with(base)));
    json!({"room": room.label(), "date": room.date.format("%y-%m-%d").to_string(), "name": room.name,
        "title": room.project.as_ref().map_or_else(|| room.slug.clone(), |p| format!("{p}/{}", room.slug)),
        "description": description, "done": room.done, "state": status, "status": data.status,
        "unanswered": unanswered, "pending": pending, "modified": data.modified, "error": data.error,
        "warnings": data.warnings, "readme": readme})
}

pub fn questions(state: &State, label: &str, path: &Path) -> Value {
    let data = room_data(state, path);
    let legacy: Vec<_> = ["QUESTIONS.md", "確認事項.md"].into_iter().filter(|name| path.join(name).is_file()).collect();
    json!({"room": label, "questions": data.questions, "warnings": data.warnings, "error": data.error,
        "legacy": legacy})
}

pub fn files(state: &State, path: &Path) -> Value {
    let mut main_exclude = Vec::new();
    let mut other_exclude = Vec::new();
    if let Ok(read) = fs::read_dir(path) {
        for entry in read.filter_map(|entry| entry.ok()) {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
            let main = if is_dir {
                matches!(name.as_str(), "outputs" | "docs" | "artifacts" | "out")
            } else {
                !matches!(
                    name.as_str(),
                    room::README | room::AGENTS | questions::QUESTIONS | questions::ANSWERS | "log.md"
                )
            };
            if main {
                other_exclude.push(name);
            } else {
                main_exclude.push(name);
            }
        }
    }
    let group = |root_exclude: &[String]| {
        let listing = fsinfo::entries(
            path,
            &fsinfo::EntriesOptions { depth: 4, max_items: 500, exclude: &state.settings.config.exclude, root_exclude },
        );
        let items: Vec<_> = listing.items.into_iter().filter_map(|entry| {
            let parts: Vec<_> = entry.path.iter().map(|p| p.to_str()).collect::<Option<_>>()?;
            if !parts.iter().all(|p| paths::valid_component(p)) { return None; }
            Some(json!({"path": parts.join("/"), "name": entry.name, "kind": match entry.kind {
                fsinfo::EntryKind::Directory => "directory", fsinfo::EntryKind::File => "file", fsinfo::EntryKind::Symlink => "symlink", fsinfo::EntryKind::Other => "other"
            }, "size": entry.size, "modified": entry.modified.and_then(timestamp)}))
        }).collect();
        json!({"items": items, "omitted": listing.omitted})
    };
    json!({"main": group(&main_exclude), "other": group(&other_exclude)})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_categories_mix_new_and_old_layouts_with_separate_limits() {
        let dir = tempfile::tempdir().unwrap();
        let state = super::super::test_state(dir.path());
        let room = dir.path().join("2026-11-02-room");
        for name in ["outputs", "docs", "artifacts", "out", "interim", "sources", "scripts", "logs", "tmp"] {
            fs::create_dir_all(room.join(name)).unwrap();
            fs::write(room.join(name).join("file.md"), "本文").unwrap();
        }
        for name in ["README.md", "AGENTS.md", "QUESTIONS.jsonl", "ANSWERS.jsonl", "log.md", "report.html", ".lock"] {
            fs::write(room.join(name), "").unwrap();
        }
        let listing = files(&state, &room);
        let main = listing["main"]["items"].as_array().unwrap();
        for name in ["outputs/file.md", "docs/file.md", "artifacts/file.md", "out/file.md", "report.html"] {
            assert!(main.iter().any(|item| item["path"] == name), "{name}");
        }
        assert!(!main.iter().any(|item| item["path"] == "log.md"));
        fs::create_dir_all(room.join("outputs/sources")).unwrap();
        fs::write(room.join("outputs/sources/log.md"), "主な表示の配下では残す").unwrap();
        let nested = files(&state, &room);
        assert!(
            nested["main"]["items"].as_array().unwrap().iter().any(|item| item["path"] == "outputs/sources/log.md")
        );
        for index in 0..510 {
            fs::write(room.join("outputs").join(format!("{index:03}.txt")), "").unwrap();
            fs::write(room.join("sources").join(format!("{index:03}.txt")), "").unwrap();
        }
        fs::create_dir_all(room.join("docs/a/b/c/d")).unwrap();
        fs::write(room.join("docs/a/b/c/d/deep.txt"), "深すぎる").unwrap();
        let listing = files(&state, &room);
        for group in ["main", "other"] {
            assert_eq!(listing[group]["items"].as_array().unwrap().len(), 500);
            assert!(listing[group]["omitted"].as_u64().unwrap() > 0);
            assert!(
                !listing[group]["items"].as_array().unwrap().iter().any(|item| item["path"]
                    .as_str()
                    .unwrap()
                    .split('/')
                    .count()
                    > 4)
            );
        }
    }

    #[test]
    fn room_cache_reuses_unchanged_data_and_invalidates_on_size_changes() {
        let dir = tempfile::tempdir().unwrap();
        let state = super::super::test_state(dir.path());
        fs::write(dir.path().join(room::README), "# 説明\n- status: 進行中\n").unwrap();
        let first = room_data(&state, dir.path());
        assert_eq!(first.status.as_deref(), Some("進行中"));
        assert!(Arc::ptr_eq(&first, &room_data(&state, dir.path())));
        fs::write(dir.path().join(room::README), "# 更新した説明\n- status: 待ち（確認）\n").unwrap();
        let changed = room_data(&state, dir.path());
        assert!(!Arc::ptr_eq(&first, &changed));
        assert_eq!(changed.status.as_deref(), Some("待ち（確認）"));
    }

    #[test]
    fn missing_readme_is_distinct_from_a_read_failure() {
        let dir = tempfile::tempdir().unwrap();
        let state = super::super::test_state(dir.path());
        let missing = room_data(&state, dir.path());
        assert!(missing.error.is_none());
        assert!(missing.description.is_empty());
        assert!(Arc::ptr_eq(&missing, &room_data(&state, dir.path())));

        // ディレクトリは、OS の権限設定によらず通常ファイルとして読めない。
        fs::create_dir(dir.path().join(room::README)).unwrap();
        let failed = room_data(&state, dir.path());
        assert!(failed.error.is_some());
        assert!(state.cache.lock().unwrap().rooms.is_empty());
    }

    #[test]
    fn read_failures_are_retried_even_when_metadata_is_unchanged() {
        for name in [room::README, questions::QUESTIONS, questions::ANSWERS] {
            let dir = tempfile::tempdir().unwrap();
            let state = super::super::test_state(dir.path());
            let path = dir.path().join(name);
            fs::create_dir(&path).unwrap();
            let before = stamp(&path);
            let first = room_data(&state, dir.path());
            let second = room_data(&state, dir.path());
            assert_eq!(stamp(&path), before, "{name}");
            assert!(first.error.is_some(), "{name}");
            assert!(second.error.is_some(), "{name}");
            assert!(!Arc::ptr_eq(&first, &second), "{name}");
            assert!(state.cache.lock().unwrap().rooms.is_empty(), "{name}");

            fs::remove_dir(&path).unwrap();
            fs::write(&path, if name == room::README { "# 読み取りが回復\n" } else { "" }).unwrap();
            let recovered = room_data(&state, dir.path());
            assert!(recovered.error.is_none(), "{name}");
            assert!(Arc::ptr_eq(&recovered, &room_data(&state, dir.path())), "{name}");
        }
    }
}
