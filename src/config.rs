//! 設定ファイルと、環境変数による上書きの解決。
//!
//! 外部の場所（作業ルート・設定ファイル・状態ファイル・Claude のデータ）はすべて
//! 環境変数で差し替えられるようにしている。テストで実データに触れないため。

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const ENV_CONFIG: &str = "WORKROOM_CONFIG";
pub const ENV_ROOT: &str = "WORKROOM_ROOT";
pub const ENV_STATE_DIR: &str = "WORKROOM_STATE_DIR";
pub const ENV_CLAUDE_DIR: &str = "WORKROOM_CLAUDE_DIR";

/// 設定ファイル（config.toml）の内容
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// 作業ルート
    pub root: Option<String>,
    /// 日付の区切り時刻（5 にすると 4:59 までは前日扱い）
    pub day_start_hour: u32,
    /// `ls` で既定表示する日数
    pub recent_days: i64,
    /// Nerd Font のアイコンを使うか
    pub icons: bool,
    /// 案件名の ASCII 英小文字を大文字にそろえるか（既存のディレクトリ名は変えない）
    pub project_uppercase: bool,
    /// tree 表示・検索・サイズ計算から除外する名前
    pub exclude: Vec<String>,
    /// 部屋の AGENTS.md にコピーするルールのテンプレート（既定は `<root>/_workroom/AGENTS.md`）
    pub agent_rules: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            root: None,
            day_start_hour: 0,
            recent_days: 14,
            icons: false,
            project_uppercase: false,
            exclude: vec!["node_modules".into(), ".venv".into(), "__pycache__".into(), ".git".into()],
            agent_rules: None,
        }
    }
}

/// 解決済みの設定
#[derive(Debug, Clone)]
pub struct Settings {
    pub config: Config,
    pub config_path: PathBuf,
    pub root: PathBuf,
    pub state_dir: PathBuf,
    pub claude_dir: PathBuf,
}

impl Settings {
    pub fn load() -> Result<Self> {
        let config_path = env_path(ENV_CONFIG).unwrap_or_else(default_config_path);
        let config = if config_path.is_file() {
            let text = std::fs::read_to_string(&config_path)
                .with_context(|| format!("設定ファイルを読めません: {}", config_path.display()))?;
            toml::from_str(text.trim_start_matches('\u{feff}'))
                .with_context(|| format!("設定ファイルの形式が不正です: {}", config_path.display()))?
        } else {
            Config::default()
        };
        if config.day_start_hour > 23 {
            anyhow::bail!("day_start_hour は 0〜23 で指定してください");
        }

        let root = env_path(ENV_ROOT)
            .or_else(|| config.root.as_deref().map(expand_home))
            .unwrap_or_else(|| home().join("workroom"));
        let state_dir = env_path(ENV_STATE_DIR).unwrap_or_else(|| {
            dirs::data_local_dir().unwrap_or_else(|| home().join(".local").join("share")).join("workroom")
        });
        let claude_dir = env_path(ENV_CLAUDE_DIR)
            .or_else(|| env_path("CLAUDE_CONFIG_DIR"))
            .unwrap_or_else(|| home().join(".claude"));

        Ok(Self { config, config_path, root, state_dir, claude_dir })
    }

    pub fn is_excluded(&self, name: &str) -> bool {
        self.config.exclude.iter().any(|e| e == name)
    }
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key).filter(|v| !v.is_empty()).map(|v| expand_home(&v.to_string_lossy()))
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// `$XDG_CONFIG_HOME/workroom/config.toml`、なければ `~/.config/workroom/config.toml`
/// （Windows でも `~/.config` を使う。dotfiles で管理しやすくするため）
fn default_config_path() -> PathBuf {
    let base = env_path("XDG_CONFIG_HOME").unwrap_or_else(|| home().join(".config"));
    base.join("workroom").join("config.toml")
}

pub fn expand_home(s: &str) -> PathBuf {
    if s == "~" {
        return home();
    }
    if let Some(rest) = s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\")) {
        return home().join(rest);
    }
    PathBuf::from(s)
}

/// 初期の設定ファイルの内容
pub fn sample_config(root: &Path) -> String {
    format!(
        r#"# workroom の設定
# 作業ルート（環境変数 WORKROOM_ROOT で上書きできる）
root = '{}'

# 日付の区切り時刻。5 にすると 4:59 までは前日の日付で作成する
day_start_hour = 0

# `ls` で既定表示する日数
recent_days = 14

# Nerd Font のアイコンを使う
icons = false

# 案件名の ASCII 英小文字を大文字にそろえる（既存の案件名は変えない）
project_uppercase = false

# tree 表示・検索・サイズ計算から除外する名前
exclude = ["node_modules", ".venv", "__pycache__", ".git"]

# 部屋の AGENTS.md にコピーするルールのテンプレート（既定は <root>/_workroom/AGENTS.md）
# ファイルがなければ、内蔵の文面から作る
# agent_rules = '~/.config/workroom/AGENTS.md'
"#,
        root.display()
    )
}
