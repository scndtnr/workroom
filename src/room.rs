//! 作業部屋（`YYYY-MM-DD-<slug>` ディレクトリ）の読み取りと作成。
//!
//! 配置は2通りを自動判別する。
//! - `<root>/YYYY-MM-DD-<slug>/`
//! - `<root>/<案件>/YYYY-MM-DD-<slug>/`

use crate::config::Settings;
use anyhow::{Context, Result, bail};
use chrono::{Local, NaiveDate, TimeDelta};
use std::io::BufRead;
use std::path::{Path, PathBuf};

pub const README: &str = "README.md";
pub const AGENTS: &str = "AGENTS.md";
pub const DONE_MARKER: &str = ".done";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Room {
    /// ディレクトリ名（`YYYY-MM-DD-<slug>`）
    pub name: String,
    pub path: PathBuf,
    pub project: Option<String>,
    pub date: NaiveDate,
    pub slug: String,
    /// README.md の1行目（slug と同じなら空）
    pub description: String,
    pub done: bool,
}

impl Room {
    /// 一覧や検索に使う表示名（案件があれば `案件/名前`）
    pub fn label(&self) -> String {
        match &self.project {
            Some(p) => format!("{p}/{}", self.name),
            None => self.name.clone(),
        }
    }
}

pub fn is_valid_slug(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// `YYYY-MM-DD-<slug>` を日付と slug に分ける
pub fn parse_room_name(name: &str) -> Option<(NaiveDate, String)> {
    if name.len() < 12 || !name.is_char_boundary(10) || name.as_bytes()[10] != b'-' {
        return None;
    }
    let date = NaiveDate::parse_from_str(&name[..10], "%Y-%m-%d").ok()?;
    Some((date, name[11..].to_string()))
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.') || name.starts_with('_')
}

/// 作業ルート配下の部屋を新しい順に返す。`since` より古い日付のものは README を読む前に除外する
pub fn scan(root: &Path, since: Option<NaiveDate>) -> Result<Vec<Room>> {
    let mut rooms = Vec::new();
    if !root.is_dir() {
        return Ok(rooms);
    }
    for entry in read_dirs(root)? {
        let name = entry.0;
        if let Some((date, slug)) = parse_room_name(&name) {
            push_room(&mut rooms, entry.1, name, None, date, slug, since);
        } else if !is_hidden(&name) {
            // 案件ディレクトリ
            for (child, path) in read_dirs(&entry.1)? {
                if let Some((date, slug)) = parse_room_name(&child) {
                    push_room(&mut rooms, path, child, Some(name.clone()), date, slug, since);
                }
            }
        }
    }
    rooms.sort_by(|a, b| b.name.cmp(&a.name).then_with(|| a.project.cmp(&b.project)));
    Ok(rooms)
}

fn push_room(
    rooms: &mut Vec<Room>,
    path: PathBuf,
    name: String,
    project: Option<String>,
    date: NaiveDate,
    slug: String,
    since: Option<NaiveDate>,
) {
    if since.is_some_and(|s| date < s) {
        return;
    }
    let description = read_description(&path, &slug);
    let done = path.join(DONE_MARKER).exists();
    rooms.push(Room { name, path, project, date, slug, description, done });
}

fn read_dirs(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("読めません: {}", dir.display()))? {
        let entry = entry?;
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            out.push((entry.file_name().to_string_lossy().into_owned(), entry.path()));
        }
    }
    Ok(out)
}

fn read_description(dir: &Path, slug: &str) -> String {
    let Ok(file) = std::fs::File::open(dir.join(README)) else {
        return String::new();
    };
    let mut line = String::new();
    if std::io::BufReader::new(file).read_line(&mut line).is_err() {
        return String::new();
    }
    let desc = line.trim_start_matches('\u{feff}').trim().trim_start_matches('#').trim().to_string();
    if desc == slug { String::new() } else { desc }
}

/// 名前（部分一致）またはカレントディレクトリから部屋を特定する
pub fn resolve(settings: &Settings, query: Option<&str>) -> Result<Room> {
    let rooms = scan(&settings.root, None)?;
    match query {
        Some(q) => {
            if let Some(r) = rooms.iter().find(|r| r.name == q || r.label() == q) {
                return Ok(r.clone());
            }
            let hits: Vec<&Room> = rooms.iter().filter(|r| r.label().contains(q)).collect();
            match hits.as_slice() {
                [one] => Ok((*one).clone()),
                [] => bail!("見つかりません: {q}"),
                many => bail!("候補が複数あります: {}", many.iter().map(|r| r.label()).collect::<Vec<_>>().join(", ")),
            }
        }
        None => {
            let cwd = std::env::current_dir()?;
            rooms
                .into_iter()
                .filter(|r| cwd.starts_with(&r.path))
                .max_by_key(|r| r.path.components().count())
                .context("作業部屋の中にいません。名前を指定してください")
        }
    }
}

/// 日付の区切り時刻を考慮した「今日」
pub fn today(settings: &Settings) -> NaiveDate {
    (Local::now() - TimeDelta::hours(settings.config.day_start_hour as i64)).date_naive()
}

pub fn parse_date(s: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").with_context(|| format!("日付は YYYY-MM-DD で指定してください: {s}"))
}

/// ルートと、ルールのテンプレート（`_workroom/AGENTS.md`）を用意する
pub fn ensure_root(settings: &Settings) -> Result<()> {
    let root = &settings.root;
    if !root.exists() {
        std::fs::create_dir_all(root).with_context(|| format!("作れません: {}", root.display()))?;
        eprintln!("作業ルートを作成しました: {}", root.display());
    }
    crate::rules::ensure_template(settings)?;
    crate::rules::warn_root_agents(settings);
    Ok(())
}

pub fn ensure_project(settings: &Settings, project: &str) -> Result<PathBuf> {
    if !is_valid_slug(project) {
        bail!("案件名は英小文字・数字・ハイフンのみ使えます: {project}");
    }
    let dir = settings.root.join(project);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub struct NewRoom<'a> {
    pub slug: &'a str,
    pub description: &'a str,
    pub project: Option<&'a str>,
    pub date: NaiveDate,
}

/// 部屋を作る。既にあれば作らずにそのパスを返す（戻り値の bool は新規作成したか）
pub fn create(settings: &Settings, req: &NewRoom) -> Result<(PathBuf, bool)> {
    if !is_valid_slug(req.slug) {
        bail!("slug は英小文字・数字・ハイフンのみ使えます（日本語の説明は2つ目の引数へ）: {}", req.slug);
    }
    ensure_root(settings)?;
    let parent = match req.project {
        Some(p) => ensure_project(settings, p)?,
        None => settings.root.clone(),
    };
    let path = parent.join(format!("{}-{}", req.date.format("%Y-%m-%d"), req.slug));
    if path.exists() {
        return Ok((path, false));
    }
    std::fs::create_dir(&path)?;
    let title = if req.description.is_empty() { req.slug } else { req.description };
    write_text(&path.join(README), &format!("# {title}\n\n- created: {}\n", Local::now().format("%Y-%m-%d %H:%M")))?;
    crate::rules::write_room_rules(settings, &path, req.project)?;
    Ok((path, true))
}

/// 名前から slug を作る（小文字にし、`_` と空白を `-` に置き換える）。結果が slug として正しいとは限らない
pub fn derive_slug(s: &str) -> String {
    let lowered = s.trim().to_lowercase();
    let re = regex::Regex::new(r"[_\s]+").expect("static regex");
    re.replace_all(&lowered, "-").into_owned()
}

pub fn set_done(room: &Room, done: bool) -> Result<()> {
    set_done_at(&room.path, done)
}

/// 部屋のパスを指定して完了マーカーを付ける・外す
pub fn set_done_at(path: &Path, done: bool) -> Result<()> {
    let marker = path.join(DONE_MARKER);
    if done {
        write_text(&marker, &format!("{}\n", Local::now().format("%Y-%m-%d %H:%M")))?;
    } else if marker.exists() {
        std::fs::remove_file(&marker)?;
    }
    Ok(())
}

/// UTF-8（BOM なし）で書き出す
pub fn write_text(path: &Path, text: &str) -> Result<()> {
    std::fs::write(path, text).with_context(|| format!("書き込めません: {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_validation() {
        assert!(is_valid_slug("fix-bug"));
        assert!(is_valid_slug("2fa"));
        assert!(!is_valid_slug("-x"));
        assert!(!is_valid_slug("Bad_Slug"));
        assert!(!is_valid_slug("日本語"));
        assert!(!is_valid_slug(""));
    }

    #[test]
    fn slug_derivation() {
        assert_eq!(derive_slug("novel_review"), "novel-review");
        assert_eq!(derive_slug("My Task"), "my-task");
        assert_eq!(derive_slug(" Fix  Bug "), "fix-bug");
        assert!(!is_valid_slug(&derive_slug("不具合")));
    }

    #[test]
    fn room_name_parsing() {
        let (d, s) = parse_room_name("2026-11-02-wk-command").unwrap();
        assert_eq!(d, NaiveDate::from_ymd_opt(2026, 11, 2).unwrap());
        assert_eq!(s, "wk-command");
        assert!(parse_room_name("2026-11-02").is_none());
        assert!(parse_room_name("2026-11-02-").is_none());
        assert!(parse_room_name("novel_review").is_none());
        assert!(parse_room_name("2026-13-40-x").is_none());
        assert!(parse_room_name("日本語日本語日本語").is_none());
    }

    fn test_settings(root: &Path) -> Settings {
        Settings {
            config: crate::config::Config::default(),
            config_path: root.join("config.toml"),
            root: root.join("作業ルート"),
            state_dir: root.join("state"),
            claude_dir: root.join("claude"),
        }
    }

    #[test]
    fn scan_skips_hidden_dirs_such_as_drive_sync_temp() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        // Google Drive for desktop がルート直下に作る一時ディレクトリ。中に部屋のような名前があっても拾わない
        for hidden in [".tmp.drivedownload", ".tmp.driveupload", "_workroom", ".git"] {
            std::fs::create_dir_all(root.join(hidden).join("2026-11-02-inside")).unwrap();
        }
        std::fs::create_dir_all(root.join("2026-11-02-room")).unwrap();
        std::fs::create_dir_all(root.join("acme").join("2026-11-01-report")).unwrap();
        let labels: Vec<_> = scan(root, None).unwrap().iter().map(|r| r.label()).collect();
        assert_eq!(labels, vec!["2026-11-02-room", "acme/2026-11-01-report"]);
    }

    #[test]
    fn create_copies_rules_and_scan_skips_templates() {
        let d = tempfile::tempdir().unwrap();
        let s = test_settings(d.path());
        let date = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
        let (a, created) = create(&s, &NewRoom { slug: "a", description: "説明", project: None, date }).unwrap();
        assert!(created);
        assert!(s.root.join("_workroom").join(AGENTS).is_file());

        let project_rules = s.root.join("acme").join("_workroom");
        std::fs::create_dir_all(&project_rules).unwrap();
        std::fs::write(project_rules.join(AGENTS), "# 案件のルール\n").unwrap();
        let (b, _) = create(&s, &NewRoom { slug: "b", description: "", project: Some("acme"), date }).unwrap();

        let rules_a = std::fs::read_to_string(a.join(AGENTS)).unwrap();
        let rules_b = std::fs::read_to_string(b.join(AGENTS)).unwrap();
        assert!(crate::rules::is_unmodified(&rules_a));
        assert!(crate::rules::is_unmodified(&rules_b));
        assert!(rules_a.contains("# 作業ディレクトリのルール"));
        assert!(!rules_a.contains("# 案件のルール"));
        assert!(rules_b.contains("# 作業ディレクトリのルール") && rules_b.ends_with("# 案件のルール\n"));

        let rooms = scan(&s.root, None).unwrap();
        let labels: Vec<_> = rooms.iter().map(|r| r.label()).collect();
        assert_eq!(labels, vec!["acme/2026-11-02-b", "2026-11-02-a"]);
        assert_eq!(rooms[1].description, "説明");
        assert_eq!(rooms[0].description, "");

        // 既にあれば作らない
        let (again, created) = create(&s, &NewRoom { slug: "a", description: "", project: None, date }).unwrap();
        assert_eq!((again, created), (a, false));
    }
}
