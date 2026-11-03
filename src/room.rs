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
    validate_slug(s).is_ok()
}

fn validate_name(name: &str, kind: &str) -> Result<()> {
    if name.is_empty() {
        bail!("{kind}は空にできません");
    }
    if name.starts_with(['-', '.']) {
        bail!("{kind}の先頭に `-` と `.` は使えません: {name}");
    }
    if name.ends_with('.') {
        bail!("{kind}の末尾に `.` は使えません: {name}");
    }
    for c in name.chars() {
        if c.is_control() {
            bail!("{kind}に制御文字は使えません: {c:?}");
        }
        if c.is_whitespace() {
            bail!("{kind}に空白は使えません（全角空白も不可）: {name}");
        }
        if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            bail!("{kind}に `{c}` は使えません: {name}");
        }
    }
    Ok(())
}

pub fn validate_slug(slug: &str) -> Result<()> {
    validate_name(slug, "部屋の名前")
}

pub fn validate_project(project: &str) -> Result<()> {
    validate_name(project, "案件名")?;
    if project.starts_with('_') {
        bail!("案件名の先頭に `_` は使えません（隠しディレクトリとして無視されます）: {project}");
    }
    if parse_room_name(project).is_some() {
        bail!("案件名に `YYYY-MM-DD-名前` の形は使えません（部屋の名前と区別できません）: {project}");
    }
    let stem = project.split('.').next().unwrap_or_default().to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|n| n.len() == 1 && matches!(n.as_bytes()[0], b'1'..=b'9'))
        })
    {
        bail!("案件名に Windows の予約名は使えません（拡張子付きも不可）: {project}");
    }
    Ok(())
}

/// 空白を正規化し、設定に応じて ASCII 英小文字だけを大文字にする
pub fn normalize_project(project: &str, uppercase: bool) -> Result<String> {
    let mut project = derive_slug(project);
    if uppercase {
        project.make_ascii_uppercase();
    }
    validate_project(&project)?;
    Ok(project)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomTarget {
    pub project: Option<String>,
    pub slug: String,
}

impl RoomTarget {
    pub fn label(&self) -> String {
        match &self.project {
            Some(p) => format!("{p}/{}", self.slug),
            None => self.slug.clone(),
        }
    }

    pub fn path(&self, root: &Path, date: NaiveDate) -> PathBuf {
        let parent = self.project.as_ref().map_or_else(|| root.to_path_buf(), |p| root.join(p));
        parent.join(format!("{}-{}", date.format("%Y-%m-%d"), self.slug))
    }
}

/// `案件/名前` または名前と `-p` を解釈する。TUI の絞り込みは `/` のないときだけ project に渡す
pub fn parse_new_name(input: &str, project: Option<&str>, uppercase: bool) -> Result<RoomTarget> {
    let (project, slug) = if let Some((p, slug)) = input.split_once('/') {
        if project.is_some() {
            bail!("`案件/名前` と `-p <案件>` は併用できません");
        }
        if slug.contains('/') || p.trim().is_empty() || slug.trim().is_empty() {
            bail!("`/` は案件と部屋の区切りとして1つだけ使い、両側に名前を指定してください");
        }
        (Some(p), slug)
    } else {
        (project, input)
    };
    let slug = derive_slug(slug);
    validate_slug(&slug)?;
    let project = project.map(|p| normalize_project(p, uppercase)).transpose()?;
    Ok(RoomTarget { project, slug })
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
    let project = normalize_project(project, settings.config.project_uppercase)?;
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
    // 検査が済むまで、ルート・テンプレート・案件も作らない。
    let target = parse_new_name(req.slug, req.project, settings.config.project_uppercase)?;
    let path = target.path(&settings.root, req.date);
    if path.exists() {
        ensure_root(settings)?;
        return Ok((path, false));
    }
    check_claude_collision(settings, &path)?;
    ensure_root(settings)?;
    if let Some(p) = &target.project {
        ensure_project(settings, p)?;
    }
    std::fs::create_dir(&path)?;
    let title = if req.description.is_empty() { &target.slug } else { req.description };
    write_text(&path.join(README), &format!("# {title}\n\n- created: {}\n", Local::now().format("%Y-%m-%d %H:%M")))?;
    crate::rules::write_room_rules(settings, &path, target.project.as_deref())?;
    warn_existing_claude_history(settings, &path)?;
    Ok((path, true))
}

/// 前後の空白を除き、途中の空白の並びを `-` にする。大小文字と `_` は保つ
pub fn derive_slug(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join("-")
}

/// Claude Code が `projects/` 配下で使うディレクトリ名（英数字以外を `-` に置換）
pub fn claude_project_key(path: &Path) -> String {
    path.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// 設定の作業ルートが相対パスでも、Claude と同じ絶対パスから履歴名を計算する
pub fn claude_history_key(path: &Path) -> Result<String> {
    let absolute = std::path::absolute(path).with_context(|| format!("絶対パスにできません: {}", path.display()))?;
    Ok(claude_project_key(&absolute))
}

/// 新しい部屋と、既存のすべての部屋の履歴名を比較する。長い名前の Claude 側の規則は対象外
pub fn check_claude_collision(settings: &Settings, path: &Path) -> Result<()> {
    let key = claude_history_key(path)?;
    if key.len() > 200 {
        return Ok(());
    }
    for room in scan(&settings.root, None)? {
        if key.eq_ignore_ascii_case(&claude_history_key(&room.path)?) {
            let name = path.strip_prefix(&settings.root).unwrap_or(path).display();
            bail!(
                "`{name}` は、Claude Code の履歴の名前が `{}` とぶつかります。名前に英数字を加えてください",
                room.label()
            );
        }
    }
    Ok(())
}

/// 既存の部屋がなくても、削除済みの部屋の履歴が残っている場合は案内する
pub fn warn_existing_claude_history(settings: &Settings, path: &Path) -> Result<()> {
    let key = claude_history_key(path)?;
    let dir = settings.claude_dir.join("projects").join(&key);
    if key.len() <= 200 && dir.is_dir() {
        eprintln!(
            "警告: Claude Code の履歴が既にあります。削除した部屋の履歴が残っている可能性があります: {}",
            dir.display()
        );
    }
    Ok(())
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
        for name in ["fix-bug", "2fa", "調査", "fix_bug", "Fix-Bug", "v2", "_x"] {
            assert!(is_valid_slug(name), "{name}");
        }
        for name in [
            "a/b", r"a\b", "a:b", "a*b", "a?b", "a\"b", "a<b", "a>b", "a|b", "末尾.", "-x", ".x", "", "a b", "a　b",
            "a\0b", "a\nb",
        ] {
            assert!(!is_valid_slug(name), "{name:?}");
        }
        assert!(validate_slug("a:b").unwrap_err().to_string().contains("`:`"));
        assert!(validate_slug("a b").unwrap_err().to_string().contains("空白"));
        assert!(validate_slug("a\0b").unwrap_err().to_string().contains("制御文字"));
    }

    #[test]
    fn project_validation() {
        for name in ["EXAMPLE営業", "example", "Fix_Bug", "COM0", "COM10", "console"] {
            assert!(validate_project(name).is_ok(), "{name}");
        }
        for name in [
            "_x",
            ".x",
            "2026-11-03-x",
            "CON",
            "con",
            "PRN",
            "aux",
            "nul.txt",
            "COM1",
            "com9.md",
            "LPT1",
            "lpt9.log",
            "a/b",
            r"a\b",
            "a:b",
            "a b",
            "-x",
            "末尾.",
            "",
        ] {
            assert!(validate_project(name).is_err(), "{name}");
        }
    }

    #[test]
    fn slug_derivation() {
        assert_eq!(derive_slug("novel_review"), "novel_review");
        assert_eq!(derive_slug("My Task"), "My-Task");
        assert_eq!(derive_slug(" Fix  Bug　 "), "Fix-Bug");
        assert_eq!(derive_slug("　調査　 分析  x　"), "調査-分析-x");
        assert!(is_valid_slug(&derive_slug("不具合")));
        assert_eq!(normalize_project(" example営業 ", false).unwrap(), "example営業");
        assert_eq!(normalize_project(" example営業 ", true).unwrap(), "EXAMPLE営業");
        assert_eq!(normalize_project("école　営業", true).unwrap(), "éCOLE-営業");
    }

    #[test]
    fn new_name_parsing() {
        let cases = [
            (" 調査 ", None, false, None, "調査"),
            (" EXAMPLE / My Task ", None, false, Some("EXAMPLE"), "My-Task"),
            ("Fix_Bug", Some(" example営業 "), false, Some("example営業"), "Fix_Bug"),
            ("Fix_Bug", Some("example営業"), true, Some("EXAMPLE営業"), "Fix_Bug"),
            ("example営業/調査", None, true, Some("EXAMPLE営業"), "調査"),
        ];
        for (input, project, uppercase, expected_project, slug) in cases {
            let target = parse_new_name(input, project, uppercase).unwrap();
            assert_eq!(target.project.as_deref(), expected_project);
            assert_eq!(target.slug, slug);
        }
        for input in ["a/b/c", "/x", "a/", " /x", "a/ ", r"a\b"] {
            assert!(parse_new_name(input, None, false).is_err(), "{input}");
        }
        assert!(parse_new_name("a/b", Some("a"), false).unwrap_err().to_string().contains("併用"));
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
        std::fs::create_dir_all(root.join("example").join("2026-11-01-report")).unwrap();
        let labels: Vec<_> = scan(root, None).unwrap().iter().map(|r| r.label()).collect();
        assert_eq!(labels, vec!["2026-11-02-room", "example/2026-11-01-report"]);
    }

    #[test]
    fn create_copies_rules_and_scan_skips_templates() {
        let d = tempfile::tempdir().unwrap();
        let s = test_settings(d.path());
        let date = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
        let (a, created) = create(&s, &NewRoom { slug: "a", description: "説明", project: None, date }).unwrap();
        assert!(created);
        assert!(s.root.join("_workroom").join(AGENTS).is_file());

        let project_rules = s.root.join("example").join("_workroom");
        std::fs::create_dir_all(&project_rules).unwrap();
        std::fs::write(project_rules.join(AGENTS), "# 案件のルール\n").unwrap();
        let (b, _) = create(&s, &NewRoom { slug: "b", description: "", project: Some("example"), date }).unwrap();

        let rules_a = std::fs::read_to_string(a.join(AGENTS)).unwrap();
        let rules_b = std::fs::read_to_string(b.join(AGENTS)).unwrap();
        assert!(crate::rules::is_unmodified(&rules_a));
        assert!(crate::rules::is_unmodified(&rules_b));
        assert!(rules_a.contains("# 作業ディレクトリのルール"));
        assert!(!rules_a.contains("# 案件のルール"));
        assert!(rules_b.contains("# 作業ディレクトリのルール") && rules_b.ends_with("# 案件のルール\n"));

        let rooms = scan(&s.root, None).unwrap();
        let labels: Vec<_> = rooms.iter().map(|r| r.label()).collect();
        assert_eq!(labels, vec!["example/2026-11-02-b", "2026-11-02-a"]);
        assert_eq!(rooms[1].description, "説明");
        assert_eq!(rooms[0].description, "");

        // 既にあれば作らない
        let (again, created) = create(&s, &NewRoom { slug: "a", description: "", project: None, date }).unwrap();
        assert_eq!((again, created), (a, false));
    }

    #[test]
    fn invalid_names_do_not_create_root_or_template() {
        let d = tempfile::tempdir().unwrap();
        let s = test_settings(d.path());
        let date = NaiveDate::from_ymd_opt(2026, 11, 3).unwrap();
        for (slug, project) in [("a:b", None), ("ok", Some("_bad")), ("ok", Some("CON"))] {
            assert!(create(&s, &NewRoom { slug, description: "", project, date }).is_err());
            assert!(!s.root.exists());
            assert!(!crate::rules::template_path(&s).exists());
        }
        assert!(ensure_project(&s, "_bad").is_err());
        assert!(!s.root.exists());
    }

    #[test]
    fn history_collision_prevents_creation_but_existing_rooms_are_entered() {
        let d = tempfile::tempdir().unwrap();
        let s = test_settings(d.path());
        let date = NaiveDate::from_ymd_opt(2026, 11, 3).unwrap();
        let req = NewRoom { slug: "調査", description: "", project: None, date };
        let (first, _) = create(&s, &req).unwrap();
        // 完了済みも検査する。
        set_done_at(&first, true).unwrap();
        let error = create(&s, &NewRoom { slug: "分析", ..req }).unwrap_err().to_string();
        assert!(error.contains("2026-11-03-調査") && error.contains("英数字"), "{error}");
        assert!(!s.root.join("2026-11-03-分析").exists());
        assert!(create(&s, &NewRoom { slug: "分析中", ..req }).unwrap().1);
        // 以前から衝突する部屋があっても、既存の部屋へは移動できる。
        std::fs::create_dir(s.root.join("2026-11-03-報告")).unwrap();
        assert_eq!(create(&s, &req).unwrap(), (first, false));
    }

    #[test]
    fn collision_checks_all_projects_and_ignores_ascii_case() {
        let d = tempfile::tempdir().unwrap();
        let s = test_settings(d.path());
        std::fs::create_dir_all(s.root.join("調査").join("2026-11-03-Task")).unwrap();
        let dest = s.root.join("分析").join("2026-11-03-task");
        assert!(check_claude_collision(&s, &dest).is_err());
        assert!(!dest.exists());
    }

    #[test]
    fn long_history_keys_are_not_checked() {
        let d = tempfile::tempdir().unwrap();
        let mut s = test_settings(d.path());
        // 各要素を短く保ちながら、履歴の名前を 200 文字より長くする。
        while claude_project_key(&s.root).len() <= 200 {
            s.root.push("long-parent-directory");
        }
        std::fs::create_dir_all(s.root.join("2026-11-03-調査")).unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 11, 3).unwrap();
        assert!(create(&s, &NewRoom { slug: "分析", description: "", project: None, date }).unwrap().1);
    }
}
