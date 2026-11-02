//! workroom: AI エージェントに日付付きの作業部屋を用意する CLI。
//! 設計は docs/design.md、進捗は docs/roadmap.md を参照。

// プレビュー（M3）と rules sync（M4）で使う関数がまだ呼ばれていないため
#![allow(dead_code)]

mod config;
mod fsinfo;
mod grep;
mod history;
mod import;
mod ls;
mod room;
mod rules;
mod shell;
mod tui;

use anyhow::Result;
use clap::{Arg, ArgAction, Args, Command, CommandFactory, FromArgMatches, Parser, Subcommand};
use config::Settings;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const HELP_TEMPLATE: &str = "\
{about-with-newline}
使い方: {usage}

{all-args}{after-help}
";

#[derive(Parser)]
#[command(
    name = "workroom",
    bin_name = "workroom",
    version,
    about = "AI エージェント（Claude Code / Codex など）に日付付きの作業部屋を用意する",
    override_usage = "workroom [検索語...]\n       workroom <コマンド> [引数...]",
    after_help = "\
普段は、シェル連携で作ったラッパー関数（wk など）から呼び出す。

シェル連携（cd するためのラッパー関数）の設定:
  PowerShell: workroom init pwsh --cmd wk | Out-String | Invoke-Expression
  bash / zsh: eval \"$(workroom init bash --cmd wk)\"
  nushell:    workroom init nu --cmd wk | save -f ~/.workroom.nu  （config.nu で source ~/.workroom.nu）",
    disable_help_subcommand = true,
    disable_version_flag = true
)]
struct Cli {
    /// 移動先を書き出すファイル（シェル連携のラッパー関数が使う）
    #[arg(long, global = true, value_name = "FILE", hide = true)]
    cwd_file: Option<PathBuf>,

    /// バージョンを表示する
    #[arg(short = 'V', long, action = ArgAction::Version)]
    version: Option<bool>,

    /// 検索語（部屋を選んで移動する）
    #[arg(value_name = "検索語")]
    query: Vec<String>,

    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// 部屋を作って移動する（既にあれば移動のみ）
    New(NewArgs),
    /// 部屋の一覧を tree 形式で表示する（既定は直近の進行中のもの）
    Ls(LsArgs),
    /// 全部屋の中身を横断検索する（rg があれば rg を使う）
    Grep {
        /// 検索する正規表現
        pattern: String,
        /// rg に渡す追加のオプション
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, value_name = "rg のオプション")]
        rg_args: Vec<String>,
    },
    /// 完了マーカー（.done）を付ける
    Done(NameArg),
    /// 完了マーカーを外す
    Undone(NameArg),
    /// 部屋をファイラーで開く（名前を省くとカレントの部屋、部屋の外ならルート）
    Open(NameArg),
    /// 作業ルートへ移動する
    Root,
    /// 既存のディレクトリを部屋として移行する（Claude Code の会話履歴も引き継ぐ）
    Import(ImportArgs),
    /// 部屋のエージェント向けルールを管理する
    Rules {
        #[command(subcommand)]
        command: RulesCmd,
    },
    /// シェル連携のラッパー関数を出力する
    Init {
        /// シェルの種類
        #[arg(value_enum)]
        shell: shell::Shell,
        /// ラッパー関数の名前
        #[arg(long, default_value = "wk", value_name = "名前")]
        cmd: String,
    },
    /// 設定ファイルのパスと内容を表示する
    Config {
        /// 設定ファイルの雛形を作る
        #[arg(long)]
        init: bool,
    },
    /// 部屋の名前を一覧する（補完用）
    #[command(hide = true)]
    Names,
}

#[derive(Subcommand)]
enum RulesCmd {
    /// 全部屋の AGENTS.md をテンプレートに合わせて更新する
    Sync {
        /// ファイルを書かずに、更新・作成の予定を表示する
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Args)]
struct NewArgs {
    /// 部屋の名前、または案件/名前（例: fix_bug、ACME/調査。省略時は -p が必要）
    #[arg(value_name = "名前")]
    slug: Option<String>,
    /// README.md の1行目に書く説明（日本語可）
    #[arg(value_name = "説明")]
    description: Vec<String>,
    /// 案件ディレクトリの下に作る
    #[arg(short, long, value_name = "案件")]
    project: Option<String>,
    /// 日付（YYYY-MM-DD。省略時は今日）
    #[arg(long, value_name = "YYYY-MM-DD")]
    date: Option<String>,
    /// 移動後に claude を起動する
    #[arg(short = 'c', long, conflicts_with = "codex")]
    claude: bool,
    /// 移動後に codex を起動する
    #[arg(short = 'x', long)]
    codex: bool,
}

#[derive(Args)]
struct LsArgs {
    /// 名前・説明で絞り込む（指定すると日数と完了の絞り込みは外れ、-a / -o より優先する）
    #[arg(value_name = "検索語")]
    query: Option<String>,
    /// 全件を表示する
    #[arg(short, long, conflicts_with = "old")]
    all: bool,
    /// 直近 N 日より前の、未完了の部屋だけを表示する（古い部屋を探す）
    #[arg(short, long)]
    old: bool,
    /// 表示する日数（-o では、これより前を表示する。既定は設定の recent_days）
    #[arg(long, value_name = "N")]
    days: Option<i64>,
    /// 部屋の中を表示する深さ
    #[arg(long, default_value_t = 1, value_name = "N")]
    depth: usize,
}

#[derive(Args)]
struct NameArg {
    /// 部屋の名前（部分一致。省略するとカレントの部屋）
    #[arg(value_name = "名前")]
    name: Option<String>,
}

#[derive(Args)]
struct ImportArgs {
    /// 移行するディレクトリ
    path: PathBuf,
    /// 部屋の名前（省略時は移行元の名前から作る）
    #[arg(value_name = "名前")]
    slug: Option<String>,
    /// README.md の1行目に書く説明（README.md がない場合のみ）
    #[arg(value_name = "説明")]
    description: Vec<String>,
    /// 日付（YYYY-MM-DD。省略時は名前の先頭の日付、なければ作成日）
    #[arg(long, value_name = "YYYY-MM-DD")]
    date: Option<String>,
    /// 案件ディレクトリの下に移す
    #[arg(short, long, value_name = "案件")]
    project: Option<String>,
}

/// clap の見出しとヘルプを日本語にする（テンプレートはサブコマンドに引き継がれないので、再帰的に設定する）
fn localize(cmd: Command) -> Command {
    let cmd = cmd
        .help_template(HELP_TEMPLATE)
        .subcommand_help_heading("コマンド")
        .subcommand_value_name("コマンド")
        .disable_help_flag(true)
        .arg(Arg::new("help").short('h').long("help").action(ArgAction::Help).help("ヘルプを表示する"))
        .mut_args({
            // 見出しを付けると名前順に並ぶため、定義順を明示する
            let mut order = 0;
            move |a| {
                order += 1;
                let heading = if a.is_positional() { "引数" } else { "オプション" };
                a.help_heading(heading).display_order(order)
            }
        });
    cmd.mut_subcommands(localize)
}

fn main() -> ExitCode {
    let matches = localize(Cli::command()).get_matches();
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(e) => e.exit(),
    };
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("workroom: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    let cwd_file = cli.cwd_file.as_deref();
    let Some(command) = cli.command else {
        let settings = Settings::load()?;
        return pick(&settings, cwd_file, &cli.query.join(" "), None, None, None);
    };
    // init は設定を読まずに動かす（設定ファイルが壊れていてもシェルの起動を止めない）
    if let Cmd::Init { shell, cmd } = &command {
        shell::validate_cmd_name(cmd)?;
        print!("{}", shell::init_script(*shell, cmd));
        return Ok(ExitCode::SUCCESS);
    }
    let settings = Settings::load()?;
    match command {
        Cmd::New(a) => {
            if a.slug.is_none() && a.project.is_none() {
                eprintln!("workroom: 部屋の名前か、`-p <案件>` を指定してください");
                return Ok(ExitCode::from(2));
            }
            let date = a.date.as_deref().map(room::parse_date).transpose()?;
            let then = if a.claude {
                Some("claude")
            } else if a.codex {
                Some("codex")
            } else {
                None
            };
            let Some(slug) = a.slug else {
                let project =
                    room::normalize_project(a.project.as_deref().unwrap(), settings.config.project_uppercase)?;
                return pick(&settings, cwd_file, "", Some(&project), date, then);
            };
            let description = a.description.join(" ");
            let req = room::NewRoom {
                slug: &slug,
                description: &description,
                project: a.project.as_deref(),
                date: date.unwrap_or_else(|| room::today(&settings)),
            };
            let (path, created) = room::create(&settings, &req)?;
            if created {
                eprintln!("作成しました: {}", path.display());
            } else {
                eprintln!("既にあるため移動します: {}", path.display());
            }
            enter(&settings, cwd_file, &path, then)?;
        }
        Cmd::Ls(a) => {
            ls::run(
                &settings,
                &ls::LsArgs { query: a.query.as_deref(), all: a.all, old: a.old, days: a.days, depth: a.depth },
            )?;
        }
        Cmd::Grep { pattern, rg_args } => {
            let code = grep::run(&settings, &pattern, &rg_args)?;
            return Ok(ExitCode::from(code.clamp(0, 255) as u8));
        }
        Cmd::Done(n) => set_done(&settings, n.name.as_deref(), true)?,
        Cmd::Undone(n) => set_done(&settings, n.name.as_deref(), false)?,
        Cmd::Open(n) => {
            let path = match n.name.as_deref() {
                Some(q) => room::resolve(&settings, Some(q))?.path,
                None => room::resolve(&settings, None).map(|r| r.path).unwrap_or_else(|_| settings.root.clone()),
            };
            shell::open_in_filer(&path)?;
        }
        Cmd::Root => {
            room::ensure_root(&settings)?;
            shell::write_cwd_file(cwd_file, &settings.root, None)?;
        }
        Cmd::Import(a) => {
            let date = a.date.as_deref().map(room::parse_date).transpose()?;
            let description = a.description.join(" ");
            let dest = import::run(
                &settings,
                &import::ImportArgs {
                    source: &a.path,
                    slug: a.slug.as_deref(),
                    description: &description,
                    date,
                    project: a.project.as_deref(),
                },
            )?;
            enter(&settings, cwd_file, &dest, None)?;
        }
        Cmd::Config { init } => show_config(&settings, init)?,
        Cmd::Rules { command: RulesCmd::Sync { dry_run } } => rules::sync(&settings, dry_run)?,
        Cmd::Names => {
            for r in room::scan(&settings.root, None)? {
                println!("{}", r.label());
            }
        }
        Cmd::Init { .. } => unreachable!(),
    }
    Ok(ExitCode::SUCCESS)
}

/// 部屋へ移動する（使用履歴を記録して、ラッパー関数に移動先を渡す）
fn enter(settings: &Settings, cwd_file: Option<&Path>, path: &Path, then: Option<&str>) -> Result<()> {
    history::record(&settings.state_dir, path);
    shell::write_cwd_file(cwd_file, path, then)
}

/// TUI ピッカー（検索語・案件の絞り込みを指定できる）
fn pick(
    settings: &Settings,
    cwd_file: Option<&Path>,
    query: &str,
    project: Option<&str>,
    date: Option<chrono::NaiveDate>,
    then: Option<&str>,
) -> Result<ExitCode> {
    if !tui::has_terminal() {
        eprintln!("workroom: ピッカーは端末でのみ使えます。一覧は `wk ls` で表示できます");
        return Ok(ExitCode::from(2));
    }
    // 端末を元に戻してから、メッセージの表示と移動先の書き出しを行う
    finish_pick(settings, cwd_file, tui::run(settings, query, project)?, date, then)?;
    Ok(ExitCode::SUCCESS)
}

fn finish_pick(
    settings: &Settings,
    cwd_file: Option<&Path>,
    outcome: tui::Outcome,
    date: Option<chrono::NaiveDate>,
    then: Option<&str>,
) -> Result<()> {
    match outcome {
        tui::Outcome::Quit => {}
        tui::Outcome::Enter(path) => enter(settings, cwd_file, &path, then)?,
        tui::Outcome::Create { project, slug } => {
            let req = room::NewRoom {
                slug: &slug,
                description: "",
                project: project.as_deref(),
                // 日付の指定がなければ、従来どおり作成時点の「今日」を使う。
                date: date.unwrap_or_else(|| room::today(settings)),
            };
            let (path, created) = room::create(settings, &req)?;
            if created {
                eprintln!("作成しました: {}", path.display());
            }
            enter(settings, cwd_file, &path, then)?;
        }
    }
    Ok(())
}

fn set_done(settings: &Settings, name: Option<&str>, done: bool) -> Result<()> {
    let r = room::resolve(settings, name)?;
    room::set_done(&r, done)?;
    eprintln!("{}: {}", if done { "完了にしました" } else { "完了を外しました" }, r.label());
    Ok(())
}

fn show_config(settings: &Settings, init: bool) -> Result<()> {
    let path = &settings.config_path;
    if init {
        if path.exists() {
            anyhow::bail!("設定ファイルは既にあります: {}", path.display());
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        room::write_text(path, &config::sample_config(&settings.root))?;
        eprintln!("設定ファイルを作成しました: {}", path.display());
        return Ok(());
    }
    println!("設定ファイル: {}", path.display());
    println!("作業ルート:   {}", settings.root.display());
    println!("テンプレート: {}", rules::template_path(settings).display());
    println!("状態:         {}", settings.state_dir.display());
    println!("Claude:       {}", settings.claude_dir.display());
    println!();
    match std::fs::read_to_string(path) {
        Ok(text) => print!("{}", text.trim_start_matches('\u{feff}')),
        Err(_) => {
            println!("（設定ファイルはありません。既定値を使っています。`workroom config --init` で雛形を作れます）")
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_outcomes_keep_creation_date_project_and_agent_command() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings {
            config: config::Config::default(),
            config_path: dir.path().join("config.toml"),
            root: dir.path().join("rooms"),
            state_dir: dir.path().join("state"),
            claude_dir: dir.path().join("claude"),
        };
        let file = dir.path().join("cwd");
        let date = room::parse_date("2000-01-02").unwrap();
        for command in ["claude", "codex"] {
            let slug = format!("調査-{command}");
            let outcome = tui::Outcome::Create { project: Some("ACME".into()), slug: slug.clone() };
            finish_pick(&settings, Some(&file), outcome, Some(date), Some(command)).unwrap();
            let path = settings.root.join("ACME").join(format!("2000-01-02-{slug}"));
            assert!(path.join("README.md").is_file());
            let expected = format!("{}\n{command}\n", path.display());
            assert_eq!(std::fs::read_to_string(&file).unwrap(), expected);
            finish_pick(&settings, Some(&file), tui::Outcome::Enter(path), Some(date), Some(command)).unwrap();
            assert_eq!(std::fs::read_to_string(&file).unwrap(), expected);
        }
        std::fs::write(&file, "").unwrap();
        finish_pick(&settings, Some(&file), tui::Outcome::Quit, None, Some("claude")).unwrap();
        assert_eq!(std::fs::read_to_string(file).unwrap(), "");
    }
}
