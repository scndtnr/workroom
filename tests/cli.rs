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
        let base = dir.path().to_path_buf();
        // 日本語を含むパスでも動くことを確かめる
        let root = base.join("作業 ルート");
        Self { _dir: dir, base, root }
    }

    fn cmd(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_workroom"));
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
    let tpl = env.root.join("acme").join("_workroom");
    fs::create_dir_all(&tpl).unwrap();
    fs::write(tpl.join("AGENTS.md"), "# 案件のルール\n").unwrap();
    let (out, lines) = env.run_cwd(&["new", "report", "-p", "acme", "--date", "2026-11-01", "-x"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let room = env.root.join("acme").join("2026-11-01-report");
    assert_eq!(lines, vec![room.display().to_string(), "codex".to_string()]);
    let agents = fs::read_to_string(room.join("AGENTS.md")).unwrap();
    assert!(agents.contains("# 作業ディレクトリのルール") && agents.ends_with("# 案件のルール\n"));

    let names = env.run(&["names"]);
    assert_eq!(stdout(&names), "acme/2026-11-01-report\n");
}

#[test]
fn new_rejects_invalid_input_without_writing_cwd_file() {
    let env = Env::new();
    for args in
        [&["new", "日本語"][..], &["new", "Bad_Slug"], &["new", "ok", "-p", "Bad"], &["new", "ok", "--date", "11/02"]]
    {
        let (out, lines) = env.run_cwd(args);
        assert!(!out.status.success(), "{args:?}");
        assert!(lines.is_empty(), "{args:?}");
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
fn query_without_subcommand_moves_to_unique_match() {
    let env = Env::new();
    env.run(&["new", "alpha", "--date", "2026-11-01"]);
    env.run(&["new", "alpine", "--date", "2026-11-02"]);

    let (out, lines) = env.run_cwd(&["alpha"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(lines, vec![env.root.join("2026-11-01-alpha").display().to_string()]);

    let (out, lines) = env.run_cwd(&["alp"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("候補が複数あります"));
    assert!(lines.is_empty());
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
    let (out, lines) = env.run_cwd(&["ghost"]);
    assert!(!out.status.success());
    assert!(lines.is_empty());
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
    assert!(!env.run(&["config", "--init"]).status.success());

    // 雛形はそのまま読み込める
    let out = stdout(&env.run(&["config"]));
    assert!(out.contains("recent_days = 14"), "{out}");
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
}
