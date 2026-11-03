//! エージェントのルール（各部屋の AGENTS.md）。
//!
//! ルールの全文を各部屋の AGENTS.md にコピーする。Codex は Git 管理外のディレクトリでは
//! 親の AGENTS.md を自動では読まないので、親への案内ではなく全文を置く。
//!
//! - テンプレートは `<root>/_workroom/AGENTS.md`（設定 `agent_rules` で変更できる）。
//!   Claude Code は親ディレクトリの AGENTS.md も読むので、ルート直下には置かない。
//! - 案件の部屋には、案件のテンプレート `<root>/<案件>/_workroom/AGENTS.md`（あれば）を後ろに連結する。
//! - コピーの1行目に、本文の sha256 を入れた識別子を書く。`rules sync` は、本文が識別子と
//!   一致する（＝生成後に手で変更されていない）部屋だけを更新する。

use crate::config::{self, Settings};
use crate::room::{self, AGENTS};
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// テンプレートを置くディレクトリ（`_` で始まるので、部屋・案件とはみなされない）
pub const TEMPLATE_DIR: &str = "_workroom";

const BUILTIN: &str = include_str!("../templates/AGENTS.md");
const HEADER_PREFIX: &str = "<!-- workroom:rules sha256=";
const HEADER_NOTE: &str = "（workroom がテンプレートから生成。手で変更すると rules sync で更新されなくなる） -->";

/// 全体のテンプレートの場所
pub fn template_path(settings: &Settings) -> PathBuf {
    match &settings.config.agent_rules {
        Some(p) => config::expand_home(p),
        None => settings.root.join(TEMPLATE_DIR).join(AGENTS),
    }
}

/// 案件のテンプレートの場所（任意。なければ連結しない）
pub fn project_template_path(settings: &Settings, project: &str) -> PathBuf {
    settings.root.join(project).join(TEMPLATE_DIR).join(AGENTS)
}

/// 全体のテンプレートがなければ、内蔵の文面から作る
pub fn ensure_template(settings: &Settings) -> Result<PathBuf> {
    let path = template_path(settings);
    if !path.exists() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("作れません: {}", dir.display()))?;
        }
        room::write_text(&path, BUILTIN)?;
        eprintln!("ルールのテンプレートを作成しました: {}", path.display());
    }
    Ok(path)
}

/// BOM を除き、改行を LF にそろえ、末尾を改行1つにする
fn normalize(text: &str) -> String {
    let t = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    format!("{}\n", t.trim_end())
}

/// 全体のテンプレートと案件のテンプレートを連結した本文
pub fn compose(global: &str, project: Option<&str>) -> String {
    let mut body = normalize(global);
    if let Some(p) = project.filter(|p| !p.trim().is_empty()) {
        body.push('\n');
        body.push_str(&normalize(p));
    }
    body
}

fn sha256_hex(text: &str) -> String {
    // sha2 0.11 からダイジェストの型が `{:x}` に対応しないので、1バイトずつ16進にする
    Sha256::digest(text.as_bytes()).iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// 本文の先頭に識別子を付ける
pub fn with_header(body: &str) -> String {
    format!("{HEADER_PREFIX}{}{HEADER_NOTE}\n{body}", sha256_hex(body))
}

/// 識別子を読み取る。戻り値は（識別子に書かれた sha256、本文）
pub fn parse_header(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start_matches('\u{feff}');
    let (first, body) = text.split_once('\n')?;
    let hash = first.strip_prefix(HEADER_PREFIX)?.get(..64)?;
    hash.bytes().all(|b| b.is_ascii_hexdigit()).then_some((hash, body))
}

/// workroom が生成したまま、手で変更されていないか
pub fn is_unmodified(text: &str) -> bool {
    parse_header(text).is_some_and(|(hash, body)| hash.eq_ignore_ascii_case(&sha256_hex(body)))
}

/// 以前の PowerShell 版（`wk sync-rules`）が生成したコピーの目印
const LEGACY_PS1_PREFIX: &str = "<!-- wk:";

/// `rules sync` で最新のテンプレートに置き換えてよいか。
/// workroom が生成して手で変更されていないもの、または PowerShell 版が生成したもの（ハッシュがないので、
/// 生成されたものとみなす）
pub fn is_replaceable(text: &str) -> bool {
    is_unmodified(text) || text.trim_start_matches('\u{feff}').starts_with(LEGACY_PS1_PREFIX)
}

/// 部屋に置く AGENTS.md の内容（識別子付き）
pub fn render(settings: &Settings, project: Option<&str>) -> Result<String> {
    let path = ensure_template(settings)?;
    let global = std::fs::read_to_string(&path)
        .with_context(|| format!("ルールのテンプレートを読めません: {}", path.display()))?;
    render_with_global(settings, &global, project)
}

fn render_with_global(settings: &Settings, global: &str, project: Option<&str>) -> Result<String> {
    let project_text = match project {
        Some(p) => {
            let pp = project_template_path(settings, p);
            if pp.is_file() {
                Some(std::fs::read_to_string(&pp).with_context(|| format!("読めません: {}", pp.display()))?)
            } else {
                None
            }
        }
        None => None,
    };
    Ok(with_header(&compose(global, project_text.as_deref())))
}

/// 部屋に AGENTS.md を書く
pub fn write_room_rules(settings: &Settings, room_dir: &Path, project: Option<&str>) -> Result<()> {
    room::write_text(&room_dir.join(AGENTS), &render(settings, project)?)
}

#[derive(Debug, PartialEq, Eq)]
enum SyncAction {
    Create,
    Update,
    Unchanged,
    Skip,
}

/// ファイルの読み書きとは独立した同期の判定
fn sync_action(current: Option<&str>, desired: &str) -> SyncAction {
    match current {
        None => SyncAction::Create,
        Some(text) if text == desired => SyncAction::Unchanged,
        Some(text) if is_replaceable(text) => SyncAction::Update,
        Some(_) => SyncAction::Skip,
    }
}

#[derive(Default)]
struct SyncReport {
    updated: Vec<String>,
    created: Vec<String>,
    unchanged: usize,
    skipped: Vec<String>,
    failed: Vec<(String, String)>,
}

impl SyncReport {
    fn print(&self, dry_run: bool) {
        let update = if dry_run { "更新する予定" } else { "更新" };
        let create = if dry_run { "作成する予定" } else { "作成" };
        for (action, labels) in
            [(update, &self.updated), (create, &self.created), ("手の変更のため対象外", &self.skipped)]
        {
            for label in labels {
                eprintln!("{action}: {label}");
            }
        }
        eprintln!(
            "{update} {}件 / {create} {}件 / 変更なし {}件 / 手の変更のため対象外 {}件",
            self.updated.len(),
            self.created.len(),
            self.unchanged,
            self.skipped.len()
        );
        for (label, error) in &self.failed {
            eprintln!("失敗: {label}: {error}");
        }
    }
}

/// 全部屋（完了済みを含む）のルールを同期する。テンプレートは自動作成しない
pub fn sync(settings: &Settings, dry_run: bool) -> Result<()> {
    let report = sync_with_writer(settings, dry_run, room::write_text)?;
    report.print(dry_run);
    if !report.failed.is_empty() {
        bail!("ルールの同期に失敗した部屋が {}件あります", report.failed.len());
    }
    Ok(())
}

fn sync_with_writer(
    settings: &Settings,
    dry_run: bool,
    mut write: impl FnMut(&Path, &str) -> Result<()>,
) -> Result<SyncReport> {
    // ensure_template は呼ばない。旧版のコピーを内蔵の文面で上書きしないため、先に読み取る
    let template = template_path(settings);
    let global = std::fs::read_to_string(&template).with_context(|| {
        let mut message = format!("ルールのテンプレートを読めません: {}", template.display());
        let legacy = settings.root.join(AGENTS);
        if !template.exists() && legacy.is_file() {
            let target = if settings.config.agent_rules.is_some() {
                template.display().to_string()
            } else {
                format!("{TEMPLATE_DIR}/{AGENTS}")
            };
            message.push_str(&format!("。{} を {target} へ移してから実行してください", legacy.display()));
        }
        message
    })?;
    let mut report = SyncReport::default();
    for r in room::scan(&settings.root, None)? {
        let result = (|| -> Result<SyncAction> {
            let desired = render_with_global(settings, &global, r.project.as_deref())?;
            let path = r.path.join(AGENTS);
            let current = match std::fs::read_to_string(&path) {
                Ok(text) => Some(text),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(e).with_context(|| format!("読めません: {}", path.display())),
            };
            let action = sync_action(current.as_deref(), &desired);
            if !dry_run && matches!(action, SyncAction::Create | SyncAction::Update) {
                write(&path, &desired)?;
            }
            Ok(action)
        })();
        let label = r.label();
        match result {
            Ok(SyncAction::Create) => report.created.push(label),
            Ok(SyncAction::Update) => report.updated.push(label),
            Ok(SyncAction::Unchanged) => report.unchanged += 1,
            Ok(SyncAction::Skip) => report.skipped.push(label),
            Err(e) => report.failed.push((label, format!("{e:#}"))),
        }
    }
    Ok(report)
}

/// 作業ルート直下の AGENTS.md は、Claude Code が部屋のコピーと二重に読み込むので警告する
pub fn warn_root_agents(settings: &Settings) {
    let legacy = settings.root.join(AGENTS);
    if legacy.is_file() {
        eprintln!(
            "警告: 作業ルートに AGENTS.md があります（{}）。部屋の AGENTS.md と二重に読み込まれるため、\
             {TEMPLATE_DIR}/{AGENTS} へ移すことを検討してください",
            legacy.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 既存の部屋の識別子と照合するので、ハッシュの文字列（小文字の16進64桁）を変えない
    #[test]
    fn sha256_hex_is_stable() {
        assert_eq!(sha256_hex(""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256_hex("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn header_roundtrip() {
        let text = with_header(&compose("# ルール\r\n\r\n- a\r\n\r\n", None));
        assert!(text.starts_with(HEADER_PREFIX));
        let (hash, body) = parse_header(&text).unwrap();
        assert_eq!(hash.len(), 64);
        assert_eq!(body, "# ルール\n\n- a\n");
        assert!(is_unmodified(&text));
        assert!(!is_unmodified(&format!("{text}- 手で追記\n")));
        assert!(!is_unmodified("# 手で書いた AGENTS.md\n"));
        assert!(parse_header("<!-- workroom:rules sha256=xyz -->\n").is_none());
    }

    #[test]
    fn replaceable_rules() {
        let generated = with_header(&compose("# ルール\n", None));
        assert!(is_replaceable(&generated));
        assert!(!is_replaceable(&format!("{generated}- 手で追記\n")));
        // PowerShell 版のコピーは、手で変更されていても生成されたものとみなす
        assert!(is_replaceable("<!-- wk: ルート（../AGENTS.md）のコピー。 -->\n\n# ルール\n- 追記\n"));
        assert!(is_replaceable("\u{feff}<!-- wk: x -->\n"));
        assert!(!is_replaceable("# 手で書いた AGENTS.md\n<!-- wk: 途中 -->\n"));
    }

    #[test]
    fn project_rules_are_appended() {
        assert_eq!(compose("\u{feff}# 全体\n", Some("# 案件\n")), "# 全体\n\n# 案件\n");
        assert_eq!(compose("# 全体", Some("  \n")), "# 全体\n");
    }

    #[test]
    fn builtin_template_has_no_header() {
        assert!(parse_header(BUILTIN).is_none());
        assert!(!BUILTIN.contains('\r'));
    }

    #[test]
    fn sync_decisions() {
        let desired = with_header(&compose("# 新しいルール\n", None));
        let old = with_header(&compose("# 古いルール\n", None));
        assert_eq!(sync_action(None, &desired), SyncAction::Create);
        assert_eq!(sync_action(Some(&desired), &desired), SyncAction::Unchanged);
        assert_eq!(sync_action(Some(&old), &desired), SyncAction::Update);
        assert_eq!(sync_action(Some("\u{feff}<!-- wk: コピー -->\n# 古いルール\n"), &desired), SyncAction::Update);
        assert_eq!(sync_action(Some(&format!("{old}- 手で追記\n")), &desired), SyncAction::Skip);
        assert_eq!(sync_action(Some("# 手で作成\n"), &desired), SyncAction::Skip);
        assert_eq!(sync_action(Some(""), &desired), SyncAction::Skip);
    }

    #[test]
    fn sync_continues_after_write_failure() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings {
            config: crate::config::Config::default(),
            config_path: dir.path().join("config.toml"),
            root: dir.path().join("root"),
            state_dir: dir.path().join("state"),
            claude_dir: dir.path().join("claude"),
        };
        let template = template_path(&settings);
        std::fs::create_dir_all(template.parent().unwrap()).unwrap();
        std::fs::write(&template, "# ルール\n").unwrap();
        let failed_room = settings.root.join("2026-11-02-failed");
        let next_room = settings.root.join("2026-11-01-next");
        std::fs::create_dir(&failed_room).unwrap();
        std::fs::create_dir(&next_room).unwrap();
        let old = with_header(&compose("# 古いルール", None));
        std::fs::write(failed_room.join(AGENTS), &old).unwrap();

        let mut attempted = Vec::new();
        let report = sync_with_writer(&settings, false, |path, text| {
            attempted.push(path.to_path_buf());
            if path == failed_room.join(AGENTS) {
                bail!("テスト用の書き込み失敗");
            }
            room::write_text(path, text)
        })
        .unwrap();
        assert_eq!(attempted, vec![failed_room.join(AGENTS), next_room.join(AGENTS)]);
        assert!(report.updated.is_empty());
        assert_eq!(report.created, vec!["2026-11-01-next"]);
        assert_eq!(report.failed, vec![("2026-11-02-failed".into(), "テスト用の書き込み失敗".into())]);
        assert_eq!(std::fs::read_to_string(failed_room.join(AGENTS)).unwrap(), old);
        assert!(is_unmodified(&std::fs::read_to_string(next_room.join(AGENTS)).unwrap()));
    }
}
