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

const DEFAULT_AGENT_RULES: &str = include_str!("../templates/AGENTS.md");

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

/// ルートと、ルールの本体（ルートの AGENTS.md）を用意する
pub fn ensure_root(settings: &Settings) -> Result<()> {
    let root = &settings.root;
    if !root.exists() {
        std::fs::create_dir_all(root).with_context(|| format!("作れません: {}", root.display()))?;
        eprintln!("作業ルートを作成しました: {}", root.display());
    }
    let agents = root.join(AGENTS);
    if !agents.exists() {
        let rules = match &settings.config.agent_rules {
            Some(p) => {
                let p = crate::config::expand_home(p);
                std::fs::read_to_string(&p).with_context(|| format!("agent_rules を読めません: {}", p.display()))?
            }
            None => DEFAULT_AGENT_RULES.to_string(),
        };
        write_text(&agents, &rules)?;
    }
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

/// 各部屋に置く AGENTS.md（Codex は Git 管理外だと親の AGENTS.md を読まないため、案内を置く）
pub fn agent_pointer(in_project: bool) -> String {
    let mut s = String::from(
        "# ルール\n\nこの作業ディレクトリのルールは、親ディレクトリの AGENTS.md に書かれている。\n作業を始める前に必ず以下を読み、それに従うこと。\n\n",
    );
    if in_project {
        s.push_str("- `../../AGENTS.md`（全体のルール）\n- `../AGENTS.md`（案件のルール。存在する場合）\n");
    } else {
        s.push_str("- `../AGENTS.md`\n");
    }
    s
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
    write_text(&path.join(AGENTS), &agent_pointer(req.project.is_some()))?;
    Ok((path, true))
}

pub fn set_done(room: &Room, done: bool) -> Result<()> {
    let marker = room.path.join(DONE_MARKER);
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
}
