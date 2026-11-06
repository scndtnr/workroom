//! 実行ファイルを動かす統合テスト。
//!
//! 作業ルート・設定・状態・Claude のデータは、すべて `WORKROOM_*` で一時ディレクトリに向ける。
//! （`std::env::set_var` は並列のテストで競合するので使わず、子プロセスごとに `.env()` で渡す）

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Env {
    _dir: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        // TEMP が 8.3 形式の短いパスの環境（GitHub Actions の Windows など）でも、
        // 移行元（正規化される）と作業ルートのパスの形をそろえるため、正規化して使う
        let base = canonical_import_path(dir.path());
        // 日本語を含むパスでも動くことを確かめる
        let root = base.join("作業 ルート");
        Self { _dir: dir, base, root }
    }

    fn cmd(&self) -> Command {
        self.command(env!("CARGO_BIN_EXE_workroom"))
    }

    fn command(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        c.env("WORKROOM_CONFIG", self.base.join("config").join("config.toml"))
            .env("WORKROOM_ROOT", &self.root)
            .env("WORKROOM_STATE_DIR", self.base.join("state"))
            .env("WORKROOM_CLAUDE_DIR", self.base.join("claude"))
            .env("NO_COLOR", "1")
            .current_dir(&self.base);
        c
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cmd().args(args).output().unwrap()
    }

    fn write_config(&self, text: &str) {
        let dir = self.base.join("config");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.toml"), text).unwrap();
    }

    /// `--cwd-file` 付きで実行し、書き出された行を返す
    fn run_cwd(&self, args: &[&str]) -> (Output, Vec<String>) {
        self.run_cwd_in(&self.base, args)
    }

    fn run_cwd_in(&self, cwd: &Path, args: &[&str]) -> (Output, Vec<String>) {
        let file = self.base.join("cwd.txt");
        fs::write(&file, "").unwrap();
        let out = self.cmd().current_dir(cwd).arg("--cwd-file").arg(&file).args(args).output().unwrap();
        let bytes = fs::read(&file).unwrap();
        assert!(!bytes.starts_with(&[0xEF, 0xBB, 0xBF]), "BOM が付いている");
        let lines = String::from_utf8(bytes).unwrap().lines().map(str::to_string).collect();
        (out, lines)
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn new_creates_room_and_writes_cwd_file() {
    let env = Env::new();
    let (out, lines) = env.run_cwd(&["new", "fix-bug", "不具合", "調査", "--date", "2026-11-02", "-c"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let room = env.root.join("2026-11-02-fix-bug");
    assert_eq!(lines, vec![room.display().to_string(), "claude".to_string()]);

    let readme = fs::read_to_string(room.join("README.md")).unwrap();
    assert!(readme.starts_with("# 不具合 調査\n\n- created: "));
    let agents = fs::read_to_string(room.join("AGENTS.md")).unwrap();
    assert!(agents.starts_with("<!-- workroom:rules sha256="));
    assert!(agents.contains("# 作業ディレクトリのルール"));
    assert!(env.root.join("_workroom").join("AGENTS.md").is_file());
    assert!(!env.root.join("AGENTS.md").exists());

    // 使用履歴は状態ディレクトリに書かれる
    assert!(env.base.join("state").join("history.json").is_file());

    // 2回目は作らずに移動だけ
    let (out, lines) = env.run_cwd(&["new", "fix-bug", "--date", "2026-11-02"]);
    assert!(out.status.success());
    assert!(stderr(&out).contains("既にある"));
    assert_eq!(lines, vec![room.display().to_string()]);
    assert!(fs::read_to_string(room.join("README.md")).unwrap().starts_with("# 不具合 調査"));
}

#[test]
fn new_in_project_appends_project_rules() {
    let env = Env::new();
    let tpl = env.root.join("example").join("_workroom");
    fs::create_dir_all(&tpl).unwrap();
    fs::write(tpl.join("AGENTS.md"), "# 案件のルール\n").unwrap();
    let (out, lines) = env.run_cwd(&["new", "report", "-p", "example", "--date", "2026-11-01", "-x"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let room = env.root.join("example").join("2026-11-01-report");
    assert_eq!(lines, vec![room.display().to_string(), "codex".to_string()]);
    let agents = fs::read_to_string(room.join("AGENTS.md")).unwrap();
    assert!(agents.contains("# 作業ディレクトリのルール") && agents.ends_with("# 案件のルール\n"));

    let names = env.run(&["names"]);
    assert_eq!(stdout(&names), "example/2026-11-01-report\n");
}

#[test]
fn new_rejects_invalid_input_without_writing_cwd_file() {
    let env = Env::new();
    for args in [
        &["new", "a:b"][..],
        &["new", r"a\b"],
        &["new", "末尾."],
        &["new", "ok", "-p", "_Bad"],
        &["new", "ok", "--date", "11/02"],
        &["new", "a/b/c"],
        &["new", "/x"],
        &["new", "a/"],
        &["new", "a/b", "-p", "a"],
    ] {
        let (out, lines) = env.run_cwd(args);
        assert!(!out.status.success(), "{args:?}");
        assert!(lines.is_empty(), "{args:?}");
        assert!(!env.root.exists(), "{args:?}");
    }
    let out = env.run(&["new", "a", "-c", "-x"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn done_and_undone() {
    let env = Env::new();
    env.run(&["new", "task", "--date", "2026-11-02"]);
    let room = env.root.join("2026-11-02-task");

    // 名前を省くとカレントの部屋（サブディレクトリの中でもよい）
    fs::create_dir(room.join("docs")).unwrap();
    let out = env.cmd().current_dir(room.join("docs")).arg("done").output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(room.join(".done").is_file());

    let out = env.run(&["undone", "task"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!room.join(".done").exists());

    // 部屋の外で名前を省くとエラー
    let out = env.run(&["done"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("作業部屋の中にいません"));
}

#[test]
fn ls_hides_done_rooms_unless_all() {
    let env = Env::new();
    let today = env.run(&["new", "active", "進行中の作業"]);
    assert!(today.status.success());
    env.run(&["new", "finished"]);
    env.run(&["done", "finished"]);
    env.run(&["new", "old", "--date", "2000-01-01"]);

    let out = stdout(&env.run(&["ls"]));
    assert!(out.contains("-active/  進行中の作業"), "{out}");
    assert!(!out.contains("finished") && !out.contains("old"), "{out}");

    let out = stdout(&env.run(&["ls", "-a"]));
    assert!(out.contains("finished/ [done]") && out.contains("2000-01-01-old/"), "{out}");

    let out = stdout(&env.run(&["ls", "進行中"]));
    assert!(out.contains("active") && !out.contains("finished"), "{out}");
}

#[test]
fn ls_old_shows_only_old_unfinished_rooms() {
    let env = Env::new();
    env.run(&["new", "recent"]);
    env.run(&["new", "old-open", "古い未完了", "--date", "2000-01-01"]);
    env.run(&["new", "old-done", "--date", "2000-01-02"]);
    env.run(&["done", "old-done"]);

    let out = env.run(&["ls", "-o"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let out = stdout(&out);
    assert!(out.contains("直近14日より前・未完了 1件"), "{out}");
    assert!(out.contains("2000-01-01-old-open/  古い未完了"), "{out}");
    assert!(!out.contains("recent") && !out.contains("old-done"), "{out}");

    // 境目は --days で変えられる（0 日なら今日より前）
    let out = stdout(&env.run(&["ls", "--old", "--days", "0"]));
    assert!(out.contains("old-open") && !out.contains("recent"), "{out}");

    assert!(!env.run(&["ls", "-a", "-o"]).status.success());
}

#[test]
fn picker_requires_terminal() {
    let env = Env::new();
    env.run(&["new", "alpha", "--date", "2026-11-01"]);
    // テストでは標準入出力が端末ではないので、ピッカーは開かずに案内を出す
    for args in [&[][..], &["alpha"], &["alpha", "ls"]] {
        let (out, lines) = env.run_cwd(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(stderr(&out).contains("ピッカーは端末でのみ使えます"), "{args:?}");
        assert!(lines.is_empty(), "{args:?}");
    }
}

#[test]
fn new_without_name_requires_project_or_terminal_and_does_not_create_files() {
    let env = Env::new();
    for args in [
        &["new"][..],
        &["new", "-p", "example"],
        &["new", "-p", "example", "--date", "2026-11-03", "-c"],
        &["new", "-p", "example", "-x"],
    ] {
        let (out, lines) = env.run_cwd(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", stderr(&out));
        let expected = if args.len() == 1 {
            "部屋の名前か、`-p <案件>` を指定してください"
        } else {
            "ピッカーは端末でのみ使えます"
        };
        assert!(stderr(&out).contains(expected), "{}", stderr(&out));
        assert!(lines.is_empty());
        assert!(!env.root.exists());
        assert!(!env.base.join("state").exists());
        assert!(!env.base.join("claude").exists());
    }
}

#[test]
fn new_accepts_japanese_underscores_case_and_normalized_spaces() {
    let env = Env::new();
    for (input, name) in [("調査", "調査"), ("Bad_Slug", "Bad_Slug"), ("　Fix  Bug　 ", "Fix-Bug")] {
        let (out, lines) = env.run_cwd(&["new", input, "--date", "2026-11-03"]);
        assert!(out.status.success(), "{}", stderr(&out));
        let room = env.root.join(format!("2026-11-03-{name}"));
        assert_eq!(lines, [room.display().to_string()]);
        assert!(fs::read_to_string(room.join("README.md")).unwrap().starts_with(&format!("# {name}\n")));
    }
    let (out, _) = env.run_cwd(&["new", "x", "-p", "Bad", "--date", "2026-11-03"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(env.root.join("Bad").join("2026-11-03-x").is_dir());
}

#[test]
fn new_project_slash_and_uppercase_configuration() {
    let env = Env::new();
    let (out, lines) = env.run_cwd(&["new", "EXAMPLE/調査", "--date", "2026-11-03"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines, [env.root.join("EXAMPLE").join("2026-11-03-調査").display().to_string()]);
    env.write_config("project_uppercase = true\n");
    for (args, project, name) in [
        (vec!["new", "x", "-p", "example"], "EXAMPLE", "x"),
        (vec!["new", "example営業/Fix_Bug"], "EXAMPLE営業", "Fix_Bug"),
        (vec!["new", "y", "-p", " example　営業 "], "EXAMPLE-営業", "y"),
    ] {
        let mut args = args;
        args.extend(["--date", "2026-11-03"]);
        let (out, lines) = env.run_cwd(&args);
        assert!(out.status.success(), "{}", stderr(&out));
        assert_eq!(lines, [env.root.join(project).join(format!("2026-11-03-{name}")).display().to_string()]);
    }
}

#[test]
fn invalid_project_never_creates_root_or_template() {
    let env = Env::new();
    for project in ["_x", ".x", "2026-11-03-x", "CON", "nul.txt", "a:b", "a/b", r"a\b"] {
        for args in [vec!["new", "x", "-p", project], vec!["new", "-p", project]] {
            let (out, lines) = env.run_cwd(&args);
            assert!(!out.status.success(), "{project}");
            assert!(stderr(&out).contains("案件名"), "{}", stderr(&out));
            assert!(lines.is_empty());
            assert!(!env.root.exists());
            assert!(!env.base.join("state").exists());
        }
    }
}

#[test]
fn new_rejects_claude_history_collisions_and_can_reenter_existing_room() {
    let env = Env::new();
    let (out, _) = env.run_cwd(&["new", "調査", "--date", "2026-11-03"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let (out, lines) = env.run_cwd(&["new", "分析", "--date", "2026-11-03"]);
    assert!(!out.status.success());
    let error = stderr(&out);
    assert!(
        error.contains("2026-11-03-分析")
            && error.contains("2026-11-03-調査")
            && error.contains("名前に英数字を加えてください"),
        "{error}"
    );
    assert!(lines.is_empty());
    assert!(!env.root.join("2026-11-03-分析").exists());
    let (out, _) = env.run_cwd(&["new", "分析中", "--date", "2026-11-03"]);
    assert!(out.status.success(), "{}", stderr(&out));
    fs::create_dir(env.root.join("2026-11-03-報告")).unwrap();
    let (out, lines) = env.run_cwd(&["new", "調査", "--date", "2026-11-03"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out).contains("既にあるため移動"));
    assert_eq!(lines, [env.root.join("2026-11-03-調査").display().to_string()]);
}

#[test]
fn new_warns_about_orphaned_claude_history_after_creating_room() {
    let env = Env::new();
    let dest = env.root.join("2026-11-03-調査");
    let history = claude_history_dir(&env, &dest);
    fs::create_dir_all(&history).unwrap();
    fs::write(history.join("keep.jsonl"), "履歴の代わり\n").unwrap();
    let (out, lines) = env.run_cwd(&["new", "調査", "--date", "2026-11-03"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(dest.join("README.md").is_file());
    assert_eq!(lines, [dest.display().to_string()]);
    assert!(stderr(&out).contains("削除した部屋の履歴が残っている可能性"), "{}", stderr(&out));
    assert_eq!(fs::read_to_string(history.join("keep.jsonl")).unwrap(), "履歴の代わり\n");
}

#[test]
fn new_with_relative_root_warns_about_history_using_absolute_path() {
    let env = Env::new();
    let dest = env.base.join("rooms").join("2026-11-03-調査");
    let history = claude_history_dir(&env, &dest);
    fs::create_dir_all(&history).unwrap();
    let out = env.cmd().env("WORKROOM_ROOT", "rooms").args(["new", "調査", "--date", "2026-11-03"]).output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(dest.join("README.md").is_file());
    assert!(stderr(&out).contains("削除した部屋の履歴が残っている可能性"), "{}", stderr(&out));
}

#[test]
fn drive_sync_temp_dirs_are_not_projects() {
    let env = Env::new();
    env.run(&["new", "real", "--date", "2026-11-02"]);
    for hidden in [".tmp.drivedownload", ".tmp.driveupload"] {
        fs::create_dir_all(env.root.join(hidden).join("2026-11-02-ghost")).unwrap();
    }
    assert_eq!(stdout(&env.run(&["names"])), "2026-11-02-real\n");
    let out = stdout(&env.run(&["ls", "-a"]));
    assert!(!out.contains(".tmp.drive") && !out.contains("ghost"), "{out}");
    let out = env.run(&["done", "ghost"]);
    assert!(stderr(&out).contains("見つかりません"), "{}", stderr(&out));
}

#[test]
fn root_creates_root_and_template() {
    let env = Env::new();
    let (out, lines) = env.run_cwd(&["root"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines, vec![env.root.display().to_string()]);
    assert!(env.root.join("_workroom").join("AGENTS.md").is_file());
}

#[test]
fn legacy_root_agents_md_is_warned_but_kept() {
    let env = Env::new();
    fs::create_dir_all(&env.root).unwrap();
    fs::write(env.root.join("AGENTS.md"), "# 古いルール\n").unwrap();
    let out = env.run(&["new", "x"]);
    assert!(out.status.success());
    assert!(stderr(&out).contains("作業ルートに AGENTS.md があります"));
    assert_eq!(fs::read_to_string(env.root.join("AGENTS.md")).unwrap(), "# 古いルール\n");
}

#[test]
fn config_shows_paths_and_init_writes_sample() {
    let env = Env::new();
    let out = stdout(&env.run(&["config"]));
    assert!(out.contains("設定ファイルはありません"), "{out}");
    assert!(out.contains(&env.root.display().to_string()), "{out}");

    let out = env.run(&["config", "--init"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let path = env.base.join("config").join("config.toml");
    assert!(fs::read_to_string(&path).unwrap().contains("day_start_hour = 0"));
    assert!(fs::read_to_string(&path).unwrap().contains("project_uppercase = false"));
    assert!(!env.run(&["config", "--init"]).status.success());

    // 雛形はそのまま読み込める
    let out = stdout(&env.run(&["config"]));
    assert!(out.contains("recent_days = 14"), "{out}");
    assert!(out.contains("hub_port = 47100"), "{out}");
}

#[test]
fn init_prints_wrapper_for_each_shell() {
    let env = Env::new();
    for shell in ["pwsh", "nu", "bash", "zsh"] {
        let out = env.run(&["init", shell, "--cmd", "wr"]);
        assert!(out.status.success());
        let s = stdout(&out);
        assert!(s.contains("wr") && !s.contains("__CMD__"), "{shell}");
        assert!(s.contains("--cwd-file"), "{shell}");
        assert!(s.is_ascii(), "{shell}");
        if shell == "pwsh" {
            assert!(s.contains("'rules'"));
        }
    }
    assert!(!env.run(&["init", "pwsh", "--cmd", "a;b"]).status.success());
    // 設定ファイルが壊れていても init は動く
    fs::create_dir_all(env.base.join("config")).unwrap();
    fs::write(env.base.join("config").join("config.toml"), "root = [").unwrap();
    assert!(env.run(&["init", "bash"]).status.success());
    assert!(!env.run(&["ls"]).status.success());
}

#[test]
fn help_is_japanese() {
    let env = Env::new();
    let out = stdout(&env.run(&["--help"]));
    assert!(out.contains("使い方") && out.contains("コマンド:") && out.contains("ヘルプを表示する"), "{out}");
    let out = stdout(&env.run(&["new", "--help"]));
    assert!(out.contains("引数:") && out.contains("オプション:"), "{out}");
    for args in [&["rules", "--help"][..], &["rules", "sync", "--help"]] {
        let result = env.run(args);
        assert!(result.status.success(), "{}", stderr(&result));
        let out = stdout(&result);
        assert!(out.contains("使い方:") && out.contains("オプション:") && out.contains("ヘルプを表示する"), "{out}");
        assert!(!out.contains("Usage:") && !out.contains("Options:"), "{out}");
    }
    let out = stdout(&env.run(&["rules", "--help"]));
    assert!(out.contains("コマンド:") && out.contains("sync"), "{out}");
    let out = stdout(&env.run(&["rules", "sync", "--help"]));
    assert!(out.contains("--dry-run") && out.contains("ファイルを書かずに"), "{out}");
}

#[test]
fn rules_sync_updates_only_replaceable_rooms_and_can_preview() {
    let env = Env::new();
    let template = env.root.join("_workroom").join("AGENTS.md");
    fs::create_dir_all(template.parent().unwrap()).unwrap();
    fs::write(&template, "# 全体の旧ルール\n").unwrap();
    for slug in ["generated", "edited"] {
        let out = env.run(&["new", slug, "--date", "2026-11-02"]);
        assert!(out.status.success(), "{}", stderr(&out));
    }
    let generated = env.root.join("2026-11-02-generated");
    fs::write(generated.join(".done"), "完了\n").unwrap();
    let edited = env.root.join("2026-11-02-edited").join("AGENTS.md");
    let edited_text = format!("{}- 手で追記\n", fs::read_to_string(&edited).unwrap());
    fs::write(&edited, &edited_text).unwrap();

    let project_template = env.root.join("example").join("_workroom").join("AGENTS.md");
    fs::create_dir_all(project_template.parent().unwrap()).unwrap();
    fs::write(&project_template, "# 案件の旧ルール\n").unwrap();
    let out = env.run(&["new", "report", "-p", "example", "--date", "2026-11-01"]);
    assert!(out.status.success(), "{}", stderr(&out));
    for slug in ["foreign", "missing", "manual"] {
        fs::create_dir(env.root.join(format!("2026-11-02-{slug}"))).unwrap();
    }
    let foreign = env.root.join("2026-11-02-foreign").join("AGENTS.md");
    // ほかのツールが作ったコメント付きの AGENTS.md は、手で書かれたものとして残す
    fs::write(&foreign, "\u{feff}<!-- wk: コピー -->\r\n# 古いルール\r\n").unwrap();
    let manual = env.root.join("2026-11-02-manual").join("AGENTS.md");
    fs::write(&manual, "# 手で作成したルール\n").unwrap();
    let missing = env.root.join("2026-11-02-missing").join("AGENTS.md");

    fs::write(&template, "\u{feff}# 全体の新ルール\r\n\r\n").unwrap();
    fs::write(&project_template, "\u{feff}# 案件の新ルール\r\n").unwrap();
    let out = env.run(&["new", "unchanged", "--date", "2026-11-02"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let unchanged = env.root.join("2026-11-02-unchanged").join("AGENTS.md");
    let report = env.root.join("example").join("2026-11-01-report").join("AGENTS.md");
    let before: Vec<_> = [
        generated.join("AGENTS.md"),
        generated.join(".done"),
        edited.clone(),
        foreign.clone(),
        manual.clone(),
        missing.clone(),
        unchanged.clone(),
        report.clone(),
        template.clone(),
        project_template.clone(),
    ]
    .into_iter()
    .map(|path| {
        let bytes = fs::read(&path).ok();
        (path, bytes)
    })
    .collect();

    let (out, cwd) = env.run_cwd(&["rules", "sync", "--dry-run"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(out.stdout.is_empty() && cwd.is_empty());
    let summary = stderr(&out);
    assert!(
        summary.contains("更新する予定 2件 / 作成する予定 1件 / 変更なし 1件 / 手の変更のため対象外 3件"),
        "{summary}"
    );
    for label in ["2026-11-02-generated", "example/2026-11-01-report"] {
        assert!(summary.contains(&format!("更新する予定: {label}")), "{summary}");
    }
    assert!(summary.contains("作成する予定: 2026-11-02-missing"), "{summary}");
    for (path, bytes) in &before {
        assert_eq!(&fs::read(path).ok(), bytes, "{}", path.display());
    }

    let (out, cwd) = env.run_cwd(&["rules", "sync"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(out.stdout.is_empty() && cwd.is_empty());
    let summary = stderr(&out);
    assert!(summary.contains("更新 2件 / 作成 1件 / 変更なし 1件 / 手の変更のため対象外 3件"), "{summary}");
    for label in ["2026-11-02-generated", "example/2026-11-01-report"] {
        assert!(summary.contains(&format!("更新: {label}")), "{summary}");
    }
    assert!(summary.contains("作成: 2026-11-02-missing"), "{summary}");
    for label in ["2026-11-02-manual", "2026-11-02-edited", "2026-11-02-foreign"] {
        assert!(summary.contains(&format!("手の変更のため対象外: {label}")), "{summary}");
    }
    let expected = fs::read(&unchanged).unwrap();
    for path in [generated.join("AGENTS.md"), missing] {
        assert_eq!(fs::read(path).unwrap(), expected);
    }
    let project_rules = fs::read_to_string(report).unwrap();
    assert!(project_rules.starts_with("<!-- workroom:rules sha256="));
    assert!(project_rules.ends_with("# 全体の新ルール\n\n# 案件の新ルール\n"));
    assert!(!project_rules.contains('\r') && !project_rules.contains('\u{feff}'));
    assert_eq!(fs::read_to_string(edited).unwrap(), edited_text);
    assert_eq!(fs::read_to_string(manual).unwrap(), "# 手で作成したルール\n");
    assert_eq!(fs::read_to_string(foreign).unwrap(), "\u{feff}<!-- wk: コピー -->\r\n# 古いルール\r\n");
    assert_eq!(fs::read_to_string(generated.join(".done")).unwrap(), "完了\n");
    assert_eq!(fs::read_to_string(template).unwrap(), "\u{feff}# 全体の新ルール\r\n\r\n");
    assert_eq!(fs::read_to_string(project_template).unwrap(), "\u{feff}# 案件の新ルール\r\n");

    let out = env.run(&["rules", "sync"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out).contains("更新 0件 / 作成 0件 / 変更なし 4件 / 手の変更のため対象外 3件"), "{}", stderr(&out));
}

#[test]
fn rules_sync_requires_template_and_guides_legacy_root_migration() {
    let env = Env::new();
    let room = env.root.join("2026-11-02-legacy");
    fs::create_dir_all(&room).unwrap();
    fs::write(env.root.join("AGENTS.md"), "# ルートのルール\n").unwrap();
    let agents = room.join("AGENTS.md");
    let legacy = "# 部屋のルール\n";
    fs::write(&agents, legacy).unwrap();
    for args in [&["rules", "sync", "--dry-run"][..], &["rules", "sync"]] {
        let (out, cwd) = env.run_cwd(args);
        assert!(!out.status.success());
        assert!(out.stdout.is_empty() && cwd.is_empty());
        let error = stderr(&out);
        assert!(error.contains("テンプレートを読めません"), "{error}");
        assert!(error.contains(&env.root.join("AGENTS.md").display().to_string()), "{error}");
        assert!(error.contains("_workroom/AGENTS.md へ移してから実行してください"), "{error}");
        assert!(!env.root.join("_workroom").exists());
        assert_eq!(fs::read_to_string(&agents).unwrap(), legacy);
        assert_eq!(fs::read_to_string(env.root.join("AGENTS.md")).unwrap(), "# ルートのルール\n");
    }
}

#[test]
fn rules_sync_does_not_create_missing_root() {
    let env = Env::new();
    for args in [&["rules", "sync", "--dry-run"][..], &["rules", "sync"]] {
        let out = env.run(args);
        assert!(!out.status.success());
        assert!(stderr(&out).contains("テンプレートを読めません"), "{}", stderr(&out));
        assert!(!env.root.exists());
        assert!(!env.base.join("state").exists());
    }
}

#[test]
fn rules_sync_uses_configured_template_and_requires_it_to_exist() {
    let env = Env::new();
    let custom = env.base.join("custom-rules.md");
    env.write_config(&format!("agent_rules = {}\n", serde_json::to_string(&custom).unwrap()));
    let room = env.root.join("2026-11-02-room");
    fs::create_dir_all(&room).unwrap();
    let default = env.root.join("_workroom").join("AGENTS.md");
    fs::create_dir(default.parent().unwrap()).unwrap();
    fs::write(&default, "# 既定の場所のルール\n").unwrap();
    for args in [&["rules", "sync", "--dry-run"][..], &["rules", "sync"]] {
        let out = env.run(args);
        assert!(!out.status.success());
        assert!(stderr(&out).contains(&custom.display().to_string()), "{}", stderr(&out));
        assert!(!custom.exists() && !room.join("AGENTS.md").exists());
    }
    fs::write(&custom, "# 設定で指定したルール\n").unwrap();
    let out = env.run(&["rules", "sync"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = fs::read_to_string(room.join("AGENTS.md")).unwrap();
    assert!(text.starts_with("<!-- workroom:rules sha256="));
    assert!(text.ends_with("# 設定で指定したルール\n"));
    assert!(!text.contains("既定の場所"));
    assert_eq!(fs::read_to_string(default).unwrap(), "# 既定の場所のルール\n");
}

#[test]
fn rules_sync_reports_failed_room_and_continues() {
    let env = Env::new();
    let template = env.root.join("_workroom").join("AGENTS.md");
    fs::create_dir_all(template.parent().unwrap()).unwrap();
    fs::write(template, "# ルール\n").unwrap();
    // AGENTS.md がディレクトリなので、権限設定に依存せず読み取りに失敗する
    let failed = env.root.join("2026-11-02-failed");
    fs::create_dir_all(failed.join("AGENTS.md")).unwrap();
    let next = env.root.join("2026-11-01-next");
    fs::create_dir(&next).unwrap();
    let (out, cwd) = env.run_cwd(&["rules", "sync"]);
    assert!(!out.status.success());
    assert!(out.stdout.is_empty() && cwd.is_empty());
    assert!(stderr(&out).contains("失敗: 2026-11-02-failed:"), "{}", stderr(&out));
    assert!(stderr(&out).contains("更新 0件 / 作成 1件 / 変更なし 0件 / 手の変更のため対象外 0件"), "{}", stderr(&out));
    assert!(failed.join("AGENTS.md").is_dir());
    assert!(fs::read_to_string(next.join("AGENTS.md")).unwrap().starts_with("<!-- workroom:rules sha256="));
}

fn check_grep(use_builtin: bool) {
    let env = Env::new();
    env.write_config("exclude = [\"node_modules\", \"test-cache\"]\n");
    let room = env.root.join("2026-11-02-search");
    fs::create_dir_all(room.join("node_modules")).unwrap();
    fs::create_dir(room.join("test-cache")).unwrap();
    fs::create_dir(room.join("docs")).unwrap();
    fs::write(room.join("docs").join("note.md"), "先頭の行\nneedle がある行\n").unwrap();
    fs::write(room.join("node_modules").join("excluded.js"), "needle excluded_only\n").unwrap();
    fs::write(room.join("test-cache").join("cached.md"), "needle excluded_only\n").unwrap();
    fs::write(room.join("AGENTS.md"), "needle rules_only\n").unwrap();
    let project_room = env.root.join("example").join("2026-11-01-report");
    fs::create_dir_all(&project_room).unwrap();
    fs::write(project_room.join("report.md"), "needle 案件の行\n").unwrap();
    let run = |pattern: &str| {
        let mut cmd = env.cmd();
        cmd.env_remove("RIPGREP_CONFIG_PATH");
        if use_builtin {
            cmd.env("PATH", "");
        }
        cmd.args(["grep", pattern, "--line-number"]).output().unwrap()
    };
    let out = run("needle");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stderr(&out).contains("rg が見つからない"), use_builtin, "{}", stderr(&out));
    let text = stdout(&out);
    assert_eq!(text.lines().count(), 2, "{text}");
    assert!(text.lines().any(|line| line.contains("note.md") && line.contains("needle がある行")), "{text}");
    assert!(text.lines().any(|line| line.contains("report.md") && line.contains("needle 案件の行")), "{text}");
    assert!(!text.contains("excluded.js") && !text.contains("cached.md") && !text.contains("AGENTS.md"), "{text}");
    for pattern in ["excluded_only", "rules_only", "no_such_match"] {
        let out = run(pattern);
        assert_eq!(out.status.code(), Some(1), "{pattern}: {}", stderr(&out));
        assert!(out.stdout.is_empty(), "{pattern}: {}", stdout(&out));
    }
}

#[test]
fn grep_with_builtin_search() {
    check_grep(true);
}

#[test]
fn grep_with_rg_when_available() {
    match Command::new("rg").arg("--version").output() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("rg が PATH にないため、rg を使う統合テストは飛ばします");
            return;
        }
        Err(e) => panic!("rg を起動できません: {e}"),
        Ok(out) => assert!(out.status.success(), "{}", stderr(&out)),
    }
    check_grep(false);
}

/// import.rs と同じく、canonicalize で付く Windows の接頭辞を除く
fn canonical_import_path(path: &Path) -> PathBuf {
    let path = fs::canonicalize(path).unwrap();
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => path,
    }
}

fn claude_history_dir(env: &Env, path: &Path) -> PathBuf {
    let path = if path.exists() { canonical_import_path(path) } else { path.to_path_buf() };
    let key: String = path.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    env.base.join("claude").join("projects").join(key)
}

fn import_source(env: &Env) -> PathBuf {
    let source = env.base.join("移行 元");
    fs::create_dir_all(source.join("docs").join("empty")).unwrap();
    fs::write(source.join("note.txt"), "移行する内容\n").unwrap();
    fs::write(source.join("docs").join("data.bin"), [0, 1, 0xff, 2]).unwrap();
    source
}

#[test]
fn import_moves_contents_and_copies_claude_history() {
    let env = Env::new();
    let source = import_source(&env);
    let source_canonical = canonical_import_path(&source);
    let old_history = claude_history_dir(&env, &source);
    fs::create_dir_all(&old_history).unwrap();
    let original_line = serde_json::json!({"cwd": source_canonical, "text": "テスト用の履歴"});
    let history = format!("{original_line}\n");
    fs::write(old_history.join("x.jsonl"), &history).unwrap();

    let (out, cwd) =
        env.run_cwd(&["import", source.to_str().unwrap(), "migrated", "取り込んだ作業", "--date", "2026-11-02"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let dest = env.root.join("2026-11-02-migrated");
    assert_eq!(cwd, vec![dest.display().to_string()]);
    assert!(!source.exists());
    assert!(dest.join("docs").join("empty").is_dir());
    assert_eq!(fs::read_to_string(dest.join("note.txt")).unwrap(), "移行する内容\n");
    assert_eq!(fs::read(dest.join("docs").join("data.bin")).unwrap(), [0, 1, 0xff, 2]);
    let readme = fs::read_to_string(dest.join("README.md")).unwrap();
    assert!(readme.starts_with("# 取り込んだ作業\n"));
    assert!(readme.contains("- imported:"));
    let agents = fs::read_to_string(dest.join("AGENTS.md")).unwrap();
    assert!(agents.lines().next().unwrap().starts_with("<!-- workroom:rules sha256="));
    assert!(agents.contains("# 作業ディレクトリのルール"));

    let new_history = claude_history_dir(&env, &dest);
    assert_ne!(old_history, new_history);
    let copied = fs::read_to_string(new_history.join("x.jsonl")).unwrap();
    let copied_line: serde_json::Value = serde_json::from_str(copied.trim_end()).unwrap();
    assert_eq!(copied_line, serde_json::json!({"cwd": canonical_import_path(&dest), "text": "テスト用の履歴"}));
    assert_eq!(fs::read_to_string(old_history.join("x.jsonl")).unwrap(), history);
}

#[test]
fn import_with_relative_root_uses_absolute_history_key_and_cwd() {
    let env = Env::new();
    let source = import_source(&env);
    let old_history = claude_history_dir(&env, &source);
    fs::create_dir_all(&old_history).unwrap();
    let line = serde_json::json!({ "cwd": canonical_import_path(&source) });
    fs::write(old_history.join("x.jsonl"), format!("{line}\n")).unwrap();
    let out = env
        .cmd()
        .env("WORKROOM_ROOT", "rooms")
        .args(["import", source.to_str().unwrap(), "migrated", "--date", "2026-11-03"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let dest = env.base.join("rooms").join("2026-11-03-migrated");
    let copied = fs::read_to_string(claude_history_dir(&env, &dest).join("x.jsonl")).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&copied).unwrap(),
        serde_json::json!({ "cwd": canonical_import_path(&dest) })
    );
    assert!(!source.exists());
    assert!(old_history.join("x.jsonl").is_file());
}

#[test]
fn import_rejects_existing_destination_and_keeps_source() {
    let env = Env::new();
    let source = import_source(&env);
    let dest = env.root.join("2026-11-02-migrated");
    fs::create_dir_all(&dest).unwrap();
    fs::write(dest.join("existing.md"), "既存の内容\n").unwrap();
    let (out, cwd) = env.run_cwd(&["import", source.to_str().unwrap(), "migrated", "--date", "2026-11-02"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("移行先が既にあります"), "{}", stderr(&out));
    assert!(cwd.is_empty());
    assert_eq!(fs::read_to_string(source.join("note.txt")).unwrap(), "移行する内容\n");
    assert_eq!(fs::read(source.join("docs").join("data.bin")).unwrap(), [0, 1, 0xff, 2]);
    assert!(source.join("docs").join("empty").is_dir());
    assert_eq!(fs::read_to_string(dest.join("existing.md")).unwrap(), "既存の内容\n");
    assert_eq!(fs::read_dir(dest).unwrap().count(), 1);
}

#[test]
fn import_rejects_cwd_inside_source_and_keeps_source() {
    for subdir in ["", "docs"] {
        let env = Env::new();
        let source = import_source(&env);
        let (out, cwd) = env.run_cwd_in(
            &source.join(subdir),
            &["import", source.to_str().unwrap(), "migrated", "--date", "2026-11-02"],
        );
        assert!(!out.status.success());
        assert!(stderr(&out).contains("移行元の中にいるため"), "{}", stderr(&out));
        assert!(cwd.is_empty());
        assert_eq!(fs::read_to_string(source.join("note.txt")).unwrap(), "移行する内容\n");
        assert_eq!(fs::read(source.join("docs").join("data.bin")).unwrap(), [0, 1, 0xff, 2]);
        assert!(source.join("docs").join("empty").is_dir());
        assert!(!env.root.exists());
    }
}

#[test]
fn import_default_name_keeps_underscores_case_and_normalizes_spaces() {
    let env = Env::new();
    for (source_name, expected) in
        [("2026-11-01-novel_review", "novel_review"), ("　My  Task　", "My-Task"), ("日本語の作業", "日本語の作業")]
    {
        let source = env.base.join(source_name);
        fs::create_dir(&source).unwrap();
        fs::write(source.join("keep.txt"), "移行する内容").unwrap();
        let (out, lines) = env.run_cwd(&["import", source.to_str().unwrap(), "--date", "2026-11-03"]);
        assert!(out.status.success(), "{}", stderr(&out));
        let dest = env.root.join(format!("2026-11-03-{expected}"));
        assert_eq!(lines, [dest.display().to_string()]);
        assert_eq!(fs::read_to_string(dest.join("keep.txt")).unwrap(), "移行する内容");
        assert!(!source.exists());
    }
}

#[test]
fn import_invalid_default_name_or_project_keeps_source_and_does_not_create_root() {
    let env = Env::new();
    let source = env.base.join(".invalid");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("keep.txt"), "残す内容").unwrap();
    for args in [vec!["import", source.to_str().unwrap()], vec!["import", source.to_str().unwrap(), "ok", "-p", "_bad"]]
    {
        let (out, lines) = env.run_cwd(&args);
        assert!(!out.status.success());
        assert!(stderr(&out).contains("部屋の名前を指定してください"), "{}", stderr(&out));
        assert!(lines.is_empty());
        assert_eq!(fs::read_to_string(source.join("keep.txt")).unwrap(), "残す内容");
        assert!(!env.root.exists());
    }
}

#[test]
fn import_rejects_history_collision_before_copying_and_keeps_source() {
    let env = Env::new();
    let source = import_source(&env);
    let existing = env.root.join("2026-11-03-調査");
    fs::create_dir_all(&existing).unwrap();
    let old_history = claude_history_dir(&env, &source);
    fs::create_dir_all(&old_history).unwrap();
    fs::write(old_history.join("keep.jsonl"), "残す履歴").unwrap();
    let (out, lines) = env.run_cwd(&["import", source.to_str().unwrap(), "分析", "--date", "2026-11-03"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("Claude Code の履歴の名前"), "{}", stderr(&out));
    assert!(lines.is_empty());
    assert!(!env.root.join("2026-11-03-分析").exists());
    assert!(!env.root.join("_workroom").exists());
    assert_eq!(fs::read_to_string(source.join("note.txt")).unwrap(), "移行する内容\n");
    assert_eq!(fs::read_to_string(old_history.join("keep.jsonl")).unwrap(), "残す履歴");
}

#[test]
fn import_normalizes_explicit_names_and_project_uppercase() {
    let env = Env::new();
    env.write_config("project_uppercase = true\n");
    let source = import_source(&env);
    let (out, lines) =
        env.run_cwd(&["import", source.to_str().unwrap(), " Fix Bug ", "-p", "example営業", "--date", "2026-11-03"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let dest = env.root.join("EXAMPLE営業").join("2026-11-03-Fix-Bug");
    assert_eq!(lines, [dest.display().to_string()]);
    assert!(dest.join("note.txt").is_file());
}

#[test]
fn pwsh_completion_reads_japanese_names_as_utf8_and_restores_console_encoding() {
    match Command::new("pwsh").args(["-NoProfile", "-NonInteractive", "-Command", "exit 0"]).output() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("pwsh が PATH にないため、PowerShell の補完テストは飛ばします");
            return;
        }
        Err(e) => panic!("pwsh を起動できません: {e}"),
        Ok(out) => assert!(out.status.success(), "{}", stderr(&out)),
    }
    let env = Env::new();
    let out = env.run(&["new", "調査", "--date", "2026-11-03"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let script = env.base.join("completion.ps1");
    fs::write(
        &script,
        r#"param([string]$Executable)
$ErrorActionPreference = 'Stop'
$env:PATH = (Split-Path -LiteralPath $Executable) + [IO.Path]::PathSeparator + $env:PATH
$original = [Console]::OutputEncoding
try {
    [Console]::OutputEncoding = [Text.Encoding]::GetEncoding(932)
    & $Executable init pwsh --cmd wr | Out-String | Invoke-Expression
    $line = 'wr done 調'
    $completions = (TabExpansion2 $line $line.Length).CompletionMatches.CompletionText
    if ('2026-11-03-調査' -notin $completions) { throw '日本語の補完候補がありません' }
    if ([Console]::OutputEncoding.CodePage -ne 932) { throw 'コンソールの文字コードが戻っていません' }
} finally {
    [Console]::OutputEncoding = $original
}
"#,
    )
    .unwrap();
    let out = env
        .command("pwsh")
        .args(["-NoProfile", "-NonInteractive", "-File"])
        .arg(&script)
        .arg(env!("CARGO_BIN_EXE_workroom"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
}
