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
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
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
    format!("{:x}", Sha256::digest(text.as_bytes()))
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
    Ok(with_header(&compose(&global, project_text.as_deref())))
}

/// 部屋に AGENTS.md を書く
pub fn write_room_rules(settings: &Settings, room_dir: &Path, project: Option<&str>) -> Result<()> {
    room::write_text(&room_dir.join(AGENTS), &render(settings, project)?)
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
}
