//! 一覧・確認事項の読み取りと、ファイルの更新日時・サイズによるメモリ内キャッシュ。

use super::{State, paths};
use crate::{fsinfo, questions, room};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Active,
    Done,
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DateRange {
    Within,
    Before,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchScope {
    Filtered,
    All,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Filter {
    pub query: Option<String>,
    pub status: Status,
    pub period: bool,
    pub days: i64,
    pub range: DateRange,
    pub scope: SearchScope,
}

impl Filter {
    pub fn from_cli(query: Option<String>, all: bool, old: bool, days: i64) -> Self {
        Self {
            scope: if query.is_some() { SearchScope::All } else { SearchScope::Filtered },
            query,
            status: if all && !old { Status::All } else { Status::Active },
            period: !all || old,
            days,
            range: if old { DateRange::Before } else { DateRange::Within },
        }
    }

    pub fn cutoff(&self, today: chrono::NaiveDate) -> Result<Option<chrono::NaiveDate>> {
        if self.scope == SearchScope::All || !self.period {
            return Ok(None);
        }
        let delta = chrono::TimeDelta::try_days(self.days).ok_or_else(|| anyhow::anyhow!("日数が大きすぎます"))?;
        Ok(Some(today.checked_sub_signed(delta).ok_or_else(|| anyhow::anyhow!("日数が大きすぎます"))?))
    }

    fn since(&self, today: chrono::NaiveDate) -> Result<Option<chrono::NaiveDate>> {
        Ok(if self.range == DateRange::Within { self.cutoff(today)? } else { None })
    }

    fn retain(&self, rooms: &mut Vec<room::Room>, today: chrono::NaiveDate) -> Result<()> {
        let cutoff = self.cutoff(today)?;
        let query = self.query.as_ref().map(|q| q.to_lowercase());
        rooms.retain(|room| {
            let status_matches = self.scope == SearchScope::All
                || match self.status {
                    Status::Active => !room.done,
                    Status::Done => room.done,
                    Status::All => true,
                };
            let period_matches = cutoff.is_none_or(|date| match self.range {
                DateRange::Within => room.date >= date,
                DateRange::Before => room.date < date,
            });
            status_matches
                && period_matches
                && query.as_ref().is_none_or(|q| {
                    room.label().to_lowercase().contains(q) || room.description.to_lowercase().contains(q)
                })
        });
        Ok(())
    }

    fn label(&self) -> String {
        let mut parts = if self.scope == SearchScope::All {
            vec!["全部屋から検索".to_string()]
        } else {
            vec![
                match self.status {
                    Status::Active => "進行中",
                    Status::Done => "完了",
                    Status::All => "すべて",
                }
                .into(),
                if self.period {
                    format!(
                        "{}日{}",
                        self.days,
                        match self.range {
                            DateRange::Within => "以内",
                            DateRange::Before => "より前",
                        }
                    )
                } else {
                    "期間なし".into()
                },
            ]
        };
        if let Some(query) = &self.query
            && !query.is_empty()
        {
            parts.push(format!("「{query}」"));
        }
        parts.join(" · ")
    }
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
    let today = room::today(&state.settings);
    let since = filter.since(today)?;
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
    let mut rooms = room::scan(&state.settings.root, since)?;
    filter.retain(&mut rooms, today)?;
    let label = filter.label();
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
    fn independent_filter_combinations_preserve_date_boundaries_and_search_scope() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 11, 7).unwrap();
        let rooms: Vec<_> = [
            ("new-open", 0, false, "Needle"),
            ("new-done", 0, true, "NEEDLE"),
            ("edge-open", 14, false, "境界"),
            ("edge-done", 14, true, "境界"),
            ("old-open", 15, false, "needle"),
            ("old-done", 15, true, "needle"),
            ("future-open", -1, false, "将来"),
        ]
        .into_iter()
        .map(|(slug, age, done, description)| {
            let date = today - chrono::TimeDelta::days(age);
            room::Room {
                name: format!("{date}-{slug}"),
                path: PathBuf::new(),
                project: None,
                date,
                slug: slug.into(),
                description: description.into(),
                done,
            }
        })
        .collect();
        let all = vec!["new-open", "new-done", "edge-open", "edge-done", "old-open", "old-done", "future-open"];
        let cases = [
            (
                Status::Active,
                true,
                DateRange::Within,
                SearchScope::Filtered,
                None,
                vec!["new-open", "edge-open", "future-open"],
            ),
            (Status::Done, true, DateRange::Within, SearchScope::Filtered, None, vec!["new-done", "edge-done"]),
            (
                Status::All,
                true,
                DateRange::Within,
                SearchScope::Filtered,
                None,
                vec!["new-open", "new-done", "edge-open", "edge-done", "future-open"],
            ),
            (Status::Active, true, DateRange::Before, SearchScope::Filtered, None, vec!["old-open"]),
            (Status::Done, true, DateRange::Before, SearchScope::Filtered, None, vec!["old-done"]),
            (Status::All, true, DateRange::Before, SearchScope::Filtered, None, vec!["old-open", "old-done"]),
            (
                Status::Active,
                false,
                DateRange::Before,
                SearchScope::Filtered,
                None,
                vec!["new-open", "edge-open", "old-open", "future-open"],
            ),
            (
                Status::Done,
                false,
                DateRange::Within,
                SearchScope::Filtered,
                None,
                vec!["new-done", "edge-done", "old-done"],
            ),
            (Status::All, false, DateRange::Within, SearchScope::Filtered, None, all.clone()),
            (Status::Active, true, DateRange::Within, SearchScope::Filtered, Some("needle"), vec!["new-open"]),
            (Status::Done, true, DateRange::Within, SearchScope::Filtered, Some("needle"), vec!["new-done"]),
            (Status::Active, true, DateRange::Before, SearchScope::Filtered, Some("needle"), vec!["old-open"]),
            (Status::All, false, DateRange::Within, SearchScope::Filtered, Some("OLD-"), vec!["old-open", "old-done"]),
            (
                Status::Active,
                true,
                DateRange::Within,
                SearchScope::All,
                Some("needle"),
                vec!["new-open", "new-done", "old-open", "old-done"],
            ),
            (
                Status::Done,
                true,
                DateRange::Before,
                SearchScope::All,
                Some("needle"),
                vec!["new-open", "new-done", "old-open", "old-done"],
            ),
            (Status::Done, true, DateRange::Before, SearchScope::All, None, all),
            (Status::Active, true, DateRange::Within, SearchScope::Filtered, Some("一致なし"), vec![]),
        ];
        for (status, period, range, scope, query, expected) in cases {
            let filter = Filter { query: query.map(str::to_string), status, period, days: 14, range, scope };
            let mut actual = rooms.clone();
            filter.retain(&mut actual, today).unwrap();
            assert_eq!(actual.iter().map(|r| r.slug.as_str()).collect::<Vec<_>>(), expected, "{filter:?}");
            assert_eq!(
                filter.since(today).unwrap(),
                if period && range == DateRange::Within && scope == SearchScope::Filtered {
                    Some(today - chrono::TimeDelta::days(14))
                } else {
                    None
                },
                "{filter:?}"
            );
        }
        let mut zero = rooms.clone();
        Filter::from_cli(None, false, false, 0).retain(&mut zero, today).unwrap();
        assert_eq!(zero.iter().map(|r| r.slug.as_str()).collect::<Vec<_>>(), ["new-open", "future-open"]);
    }

    #[test]
    fn cli_arguments_set_initial_independent_conditions() {
        for (query, all, old, days, status, period, range, scope) in [
            (None, false, false, 14, Status::Active, true, DateRange::Within, SearchScope::Filtered),
            (None, true, false, 14, Status::All, false, DateRange::Within, SearchScope::Filtered),
            (None, false, true, 3, Status::Active, true, DateRange::Before, SearchScope::Filtered),
            (Some("調査"), false, false, 7, Status::Active, true, DateRange::Within, SearchScope::All),
            (Some("調査"), true, false, 0, Status::All, false, DateRange::Within, SearchScope::All),
        ] {
            let filter = Filter::from_cli(query.map(str::to_string), all, old, days);
            assert_eq!(filter.status, status);
            assert_eq!(filter.period, period);
            assert_eq!(filter.range, range);
            assert_eq!(filter.scope, scope);
            assert_eq!(filter.days, days);
            assert_eq!(filter.query.as_deref(), query);
        }
    }

    #[test]
    fn selection_uses_workroom_today_and_keeps_scan_order() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = super::super::test_state(dir.path());
        state.settings.config.day_start_hour = 23;
        let today = room::today(&state.settings);
        for (age, done, slug) in [(0, false, "new"), (2, true, "edge"), (3, true, "old")] {
            let path = dir.path().join(format!("{}-{slug}", today - chrono::TimeDelta::days(age)));
            fs::create_dir(&path).unwrap();
            fs::write(path.join(room::README), "# 共通の説明\n").unwrap();
            if done {
                fs::write(path.join(room::DONE_MARKER), "").unwrap();
            }
        }
        let filter = Filter { status: Status::Done, ..Filter::from_cli(None, false, false, 2) };
        let (rooms, label) = selected(&state, &filter).unwrap();
        assert_eq!(rooms.iter().map(|r| r.slug.as_str()).collect::<Vec<_>>(), ["edge"]);
        assert_eq!(label, "完了 · 2日以内");
        let (rooms, _) = selected(&state, &Filter { range: DateRange::Before, ..filter }).unwrap();
        assert_eq!(rooms.iter().map(|r| r.slug.as_str()).collect::<Vec<_>>(), ["old"]);
    }

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
