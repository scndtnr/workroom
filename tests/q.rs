//! 実データに触れず、確認事項の CLI を子プロセスで確認する。

use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

struct Env {
    _dir: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    room: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(dir.path()).unwrap();
        let root = base.join("作業 ルート");
        let room = root.join("EXAMPLE").join("2026-11-03-調査");
        fs::create_dir_all(&room).unwrap();
        fs::write(room.join("README.md"), "# 調査\n").unwrap();
        Self { _dir: dir, base, root, room }
    }

    fn cmd(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_workroom"));
        command
            .env("WORKROOM_CONFIG", self.base.join("config.toml"))
            .env("WORKROOM_ROOT", &self.root)
            .env("WORKROOM_STATE_DIR", self.base.join("state"))
            .env("WORKROOM_CLAUDE_DIR", self.base.join("claude"))
            .env_remove("CLAUDECODE")
            .env("NO_COLOR", "1")
            .current_dir(&self.room);
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cmd().arg("q").args(args).output().unwrap()
    }

    fn ask(&self, text: &str) -> String {
        let output = self.run(&["ask", text, "--by", "テストエージェント"]);
        success(&output);
        stdout(&output).trim().into()
    }

    fn answer(&self, id: &str, kind: &str, text: &str) {
        let events = self.events();
        let ask = events.iter().find(|v| v["id"] == id && v["type"] == "ask").unwrap();
        let old = fs::read_to_string(self.room.join("ANSWERS.jsonl")).unwrap_or_default();
        let n = old.lines().map(|s| serde_json::from_str::<Value>(s).unwrap()).filter(|v| v["id"] == id).count() + 1;
        let record = json!({"v":1,"type":"answer","id":id,"n":n,"at":"2026-11-06T14:05:12+09:00",
            "kind":kind,"text":text,"question":{"question":ask["question"],"background":ask["background"],
            "choices":ask.get("choices").cloned().unwrap_or(json!([])),"recommend":ask["recommend"],
            "by":ask["by"],"asked_at":ask["at"],"origin":ask["origin"]}});
        writeln!(
            OpenOptions::new().create(true).append(true).open(self.room.join("ANSWERS.jsonl")).unwrap(),
            "{record}"
        )
        .unwrap();
    }

    fn events(&self) -> Vec<Value> {
        fs::read_to_string(self.room.join("QUESTIONS.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}
fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}
fn success(output: &Output) {
    assert_eq!(output.status.code(), Some(0), "{}", stderr(output));
}
fn parsed(output: &Output) -> Value {
    success(output);
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn ask_outputs_only_id_and_stores_fields_with_local_timestamp() {
    let env = Env::new();
    let output = env.run(&[
        "ask",
        "  出力形式はどうしますか  ",
        "--background",
        "背景\n2行目",
        "--choice",
        "表",
        "--choice",
        "文章",
        "--recommend",
        "1",
        "--by",
        "テストエージェント",
    ]);
    success(&output);
    assert!(regex::Regex::new(r"^Q-\d{8}-\d{2,}\n$").unwrap().is_match(&stdout(&output)));
    assert!(stderr(&output).contains("起票しました:"));
    let event = &env.events()[0];
    assert_eq!(event["question"], "出力形式はどうしますか");
    assert_eq!(event["background"], "背景\n2行目");
    assert_eq!(event["choices"], json!(["表", "文章"]));
    assert_eq!(event["recommend"], 1);
    assert_eq!(event["by"], "テストエージェント");
    chrono::DateTime::parse_from_rfc3339(event["at"].as_str().unwrap()).unwrap();
    let bytes = fs::read(env.room.join("QUESTIONS.jsonl")).unwrap();
    assert!(!bytes.starts_with(&[0xef, 0xbb, 0xbf]));
    assert!(!bytes.contains(&b'\r'));
    assert!(!String::from_utf8(bytes).unwrap().contains("\\u"));
    assert!(!env.base.join("state").exists());
    assert!(!env.base.join("claude").exists());
    assert!(!env.room.join("ANSWERS.jsonl").exists());
}

#[test]
fn parallel_ask_processes_get_distinct_consecutive_ids() {
    let env = Env::new();
    let children: Vec<_> = (0..8)
        .map(|_| {
            env.cmd().args(["q", "ask", "並行の質問"]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap()
        })
        .collect();
    let mut ids: Vec<_> = children
        .into_iter()
        .map(|child| {
            let out = child.wait_with_output().unwrap();
            success(&out);
            stdout(&out).trim().to_string()
        })
        .collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 8);
    let mut dates = std::collections::BTreeMap::new();
    for id in &ids {
        let (prefix, suffix) = id.rsplit_once('-').unwrap();
        let expected = dates.entry(prefix).or_insert(0usize);
        *expected += 1;
        assert_eq!(suffix.parse::<usize>().unwrap(), *expected);
    }
    assert_eq!(env.events().len(), 8);
    assert!(!env.room.join(".QUESTIONS.jsonl.lock").exists());
}

#[test]
fn pending_is_one_line_when_empty_or_unanswered_and_does_not_show_question_text() {
    let env = Env::new();
    let out = env.run(&["pending"]);
    success(&out);
    assert_eq!(stdout(&out), "確認事項はありません。\n");
    let id = env.ask("未回答の質問の本文");
    let out = env.run(&["pending"]);
    success(&out);
    assert_eq!(stdout(&out), format!("確認事項：対応が要るものはありません（未回答 1 件: {id}）。\n"));
    let data = parsed(&env.run(&["pending", "--json"]));
    assert_eq!(data["unanswered"], json!([id]));
    assert_eq!(data["pending"], json!([]));
}

#[test]
fn pending_preserves_new_answers_between_reading_and_done() {
    let env = Env::new();
    let id = env.ask("対象期間はどうしますか");
    env.answer(&id, "answer", "最初の回答\n# 見出し\n- 項目");
    let out = env.run(&["pending"]);
    success(&out);
    assert!(stdout(&out).contains("# 確認事項：反映が要る回答（EXAMPLE/2026-11-03-調査）"));
    assert!(stdout(&out).contains("  # 見出し\n  - 項目"));
    assert!(stdout(&out).contains(&format!("workroom q done {id} --upto 1 --to \"<反映先>\"")));
    env.answer(&id, "answer", "後から届いた回答");
    let before = fs::read(env.room.join("ANSWERS.jsonl")).unwrap();
    success(&env.run(&["done", &id, "--upto", "1", "--to", "README"]));
    let out = env.run(&["pending"]);
    success(&out);
    assert!(stdout(&out).contains("後から届いた回答"));
    assert!(!stdout(&out).contains("最初の回答"));
    assert!(stdout(&out).contains("--upto 2"));
    assert!(stdout(&out).contains("前に反映済みの回答 1 件"));
    let data = parsed(&env.run(&["pending", "--json"]));
    assert_eq!(data["pending"][0]["reflected_upto"], 1);
    assert_eq!(data["pending"][0]["answers"][0]["n"], 2);
    assert_eq!(data["pending"][0]["answers"].as_array().unwrap().len(), 1);
    let out = env.run(&["done", &id, "--to", "docs/方針.md"]);
    success(&out);
    assert!(stdout(&out).contains("後から届いた回答"));
    assert_eq!(before, fs::read(env.room.join("ANSWERS.jsonl")).unwrap());
    assert_eq!(parsed(&env.run(&["pending", "--json"]))["pending"], json!([]));
}

#[test]
fn done_requires_answer_or_chat_and_is_idempotent() {
    let env = Env::new();
    let id = env.ask("会話で答える質問");
    for args in
        [vec!["done", id.as_str(), "--to", "README"], vec!["done", id.as_str(), "--upto", "0", "--to", "README"]]
    {
        let out = env.run(&args);
        assert_eq!(out.status.code(), Some(1));
        assert!(stderr(&out).contains("反映する回答がありません"));
    }
    assert_eq!(env.events().len(), 1);
    assert_eq!(env.run(&["done", &id, "--chat", "表でよい"]).status.code(), Some(1));
    success(&env.run(&["done", &id, "--chat", "表でよい", "--to", "README"]));
    assert_eq!(env.events()[1]["upto"], 0);
    let before = fs::read(env.room.join("QUESTIONS.jsonl")).unwrap();
    success(&env.run(&["done", &id]));
    assert_eq!(before, fs::read(env.room.join("QUESTIONS.jsonl")).unwrap());
    let out = env.run(&["show", "--all"]);
    success(&out);
    assert!(stdout(&out).contains("解決済み"));
    assert!(stdout(&out).contains("表でよい"));
    assert!(!stdout(&env.run(&["show"])).contains(&id));
    env.answer(&id, "answer", "解決後の追加回答");
    assert_eq!(parsed(&env.run(&["pending", "--json"]))["pending"][0]["id"], id);
}

#[test]
fn explicit_upto_is_clamped_and_repeating_it_does_not_consume_new_answers() {
    let env = Env::new();
    let id = env.ask("質問");
    env.answer(&id, "answer", "回答1");
    success(&env.run(&["done", &id, "--upto", "100", "--to", "README"]));
    assert_eq!(env.events()[1]["upto"], 1);
    env.answer(&id, "answer", "回答2");
    success(&env.run(&["done", &id, "--upto", "1"]));
    assert_eq!(env.events().len(), 2);
    assert_eq!(parsed(&env.run(&["pending", "--json"]))["pending"][0]["answers"][0]["text"], "回答2");
}

#[test]
fn not_needed_can_be_done_without_to() {
    let env = Env::new();
    let id = env.ask("回答不要になる質問");
    env.answer(&id, "not_needed", "");
    let out = env.run(&["pending"]);
    success(&out);
    assert!(stdout(&out).contains("ユーザーは回答不要とした（理由なし）"));
    assert!(stdout(&out).contains(&format!("workroom q done {id} --upto 1\n")));
    success(&env.run(&["done", &id, "--upto", "1"]));
    assert_eq!(env.events()[1]["to"], "回答不要");
    assert_eq!(env.events()[1]["upto"], 1);
}

#[test]
fn withdraw_reports_reflection_locations_and_remaining_answers_and_is_final() {
    let env = Env::new();
    let id = env.ask("古い質問");
    let next = env.run(&["ask", "聞き直し", "--origin", &id]);
    success(&next);
    let successor = stdout(&next).trim().to_string();
    env.answer(&id, "answer", "最初の回答");
    success(&env.run(&["done", &id, "--upto", "1", "--to", "README の条件"]));
    env.answer(&id, "answer", "未反映の回答");
    let out = env.run(&["withdraw", &id, "--reason", "前提の誤り", "--successor", &successor]);
    success(&out);
    assert!(stdout(&out).contains("README の条件"));
    assert!(stdout(&out).contains("未反映の回答が 1 件あります"));
    let before = fs::read(env.room.join("QUESTIONS.jsonl")).unwrap();
    success(&env.run(&["withdraw", &id, "--reason", "再実行"]));
    assert_eq!(before, fs::read(env.room.join("QUESTIONS.jsonl")).unwrap());
    assert_eq!(env.run(&["done", &id, "--chat", "回答", "--to", "README"]).status.code(), Some(1));
    env.answer(&id, "answer", "取り下げた後の回答");
    assert_eq!(parsed(&env.run(&["pending", "--json"]))["pending"], json!([]));
    let out = env.run(&["show", "--all"]);
    success(&out);
    assert!(stdout(&out).contains("前提の誤り"));
    assert!(stdout(&out).contains(&format!("後継: {successor}")));
    assert!(stdout(&out).contains("取り下げた項目への回答"));
}

#[test]
fn room_argument_resolves_from_outside_and_cwd_ignores_nested_date_directories() {
    let env = Env::new();
    let out = env.cmd().current_dir(&env.base).args(["q", "pending"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("部屋の中で実行するか、--room を指定してください"));
    for args in [vec!["q", "--room", "調査", "ask", "外からの質問"], vec!["q", "ask", "外からの質問", "--room", "調査"]]
    {
        success(&env.cmd().current_dir(&env.base).args(args).output().unwrap());
    }
    let nested = env.room.join("outputs").join("2026-11-05-報告");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("README.md"), "# 成果物\n").unwrap();
    let out = env.cmd().current_dir(&nested).args(["q", "ask", "入れ子から"]).output().unwrap();
    success(&out);
    assert!(!nested.join("QUESTIONS.jsonl").exists());
    assert_eq!(env.events().len(), 3);
    // 他の部屋に重複する検索語があれば、既存 resolve と同じくエラー。
    fs::create_dir_all(env.root.join("2026-11-04-調査追加")).unwrap();
    let out = env.run(&["pending", "--room", "調査"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("候補が複数"));
}

#[test]
fn unreadable_settings_use_outermost_cwd_but_valid_other_root_is_rejected() {
    let env = Env::new();
    fs::write(env.base.join("config.toml"), "[壊れた設定").unwrap();
    let nested = env.room.join("outputs").join("2026-11-05-報告");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("AGENTS.md"), "ルール").unwrap();
    let out = env
        .cmd()
        .env("WORKROOM_ROOT", env.base.join("別のルート"))
        .current_dir(&nested)
        .args(["q", "ask", "設定なしの質問"])
        .output()
        .unwrap();
    success(&out);
    assert!(stderr(&out).contains("設定を読めないため"));
    assert_eq!(env.events().len(), 1);
    fs::write(env.base.join("config.toml"), "").unwrap();
    let out = env.cmd().env("WORKROOM_ROOT", env.base.join("別のルート")).args(["q", "pending"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn invalid_values_and_missing_ids_are_runtime_errors_but_argument_errors_are_two() {
    let env = Env::new();
    for args in [
        vec!["ask", "改行\nあり"],
        vec!["ask", "\n質問\n"],
        vec!["ask", " "],
        vec!["ask", "質問", "--recommend", "1"],
        vec!["ask", "質問", "--choice", "選択肢", "--recommend", "0"],
        vec!["ask", "質問", "--origin", "Q-20261106-99"],
        vec!["done", "Q-20261106-99", "--to", "README"],
        vec!["withdraw", "Q-20261106-99", "--reason", "理由"],
        vec!["pending", "--room", "不在"],
    ] {
        assert_eq!(env.run(&args).status.code(), Some(1), "{args:?}");
    }
    assert_eq!(env.run(&["ask", &"あ".repeat(1001)]).status.code(), Some(1));
    for args in [
        vec!["ask"],
        vec!["withdraw", "Q-20261106-01"],
        vec!["done", "Q-20261106-01", "--upto", "bad"],
        vec!["unknown"],
        vec![],
    ] {
        assert_eq!(env.run(&args).status.code(), Some(2), "{args:?}");
    }
    let id = env.ask("質問");
    assert_eq!(env.run(&["withdraw", &id, "--reason", "理由", "--successor", "Q-20261106-99"]).status.code(), Some(1));
}

#[test]
fn warnings_are_on_stderr_and_in_markdown_or_json_without_changing_logs() {
    let env = Env::new();
    env.ask("質問");
    writeln!(OpenOptions::new().append(true).open(env.room.join("QUESTIONS.jsonl")).unwrap(), "broken").unwrap();
    let before = fs::read(env.room.join("QUESTIONS.jsonl")).unwrap();
    for command in ["pending", "show"] {
        let out = env.run(&[command]);
        success(&out);
        assert!(stdout(&out).contains("警告: QUESTIONS.jsonl の 2 行目"));
        assert!(stderr(&out).contains("警告: QUESTIONS.jsonl の 2 行目"));
        let out = env.run(&[command, "--json"]);
        let data = parsed(&out);
        assert_eq!(data["warnings"][0]["line"], 2);
        assert_eq!(data["warnings"][0]["file"], "QUESTIONS.jsonl");
    }
    assert_eq!(before, fs::read(env.room.join("QUESTIONS.jsonl")).unwrap());
}

#[test]
fn claude_environment_default_and_explicit_by_and_cwd_file_are_preserved() {
    let env = Env::new();
    let file = env.base.join("cwd.txt");
    fs::write(&file, "変更しない\n").unwrap();
    let out =
        env.cmd().env("CLAUDECODE", "1").arg("--cwd-file").arg(&file).args(["q", "ask", "質問"]).output().unwrap();
    success(&out);
    assert_eq!(env.events()[0]["by"], "Claude Code");
    let out = env.cmd().env("CLAUDECODE", "1").args(["q", "ask", "質問", "--by", "明示した名前"]).output().unwrap();
    success(&out);
    assert_eq!(env.events()[1]["by"], "明示した名前");
    assert_eq!(fs::read_to_string(file).unwrap(), "変更しない\n");
    let out = env.cmd().args(["q", "--help"]).output().unwrap();
    success(&out);
    for text in ["ask", "pending", "done", "withdraw", "show", "--room", "確認"] {
        assert!(stdout(&out).contains(text));
    }
}

#[test]
fn numbering_uses_configured_day_start_hour() {
    let env = Env::new();
    fs::write(env.base.join("config.toml"), "day_start_hour = 23\n").unwrap();
    let before = (chrono::Local::now() - chrono::TimeDelta::hours(23)).date_naive();
    let id = env.ask("日付の質問");
    let after = (chrono::Local::now() - chrono::TimeDelta::hours(23)).date_naive();
    assert!([before, after].iter().any(|date| id.starts_with(&format!("Q-{}-", date.format("%Y%m%d")))));
}

#[test]
fn lock_timeout_is_runtime_error_and_a_stale_lock_can_be_recovered() {
    let env = Env::new();
    let path = env.room.join(".QUESTIONS.jsonl.lock");
    fs::write(&path, "pid=停止したテストプロセス\n").unwrap();
    let start = std::time::Instant::now();
    let out = env.run(&["ask", "ロック中の質問"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("ロックを取得できません"));
    assert!(start.elapsed() >= std::time::Duration::from_secs(5));
    assert!(!env.room.join("QUESTIONS.jsonl").exists());
    let file = OpenOptions::new().write(true).open(&path).unwrap();
    file.set_times(
        fs::FileTimes::new().set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(31)),
    )
    .unwrap();
    drop(file);
    env.ask("古いロックを回収する質問");
    assert!(!path.exists());
    assert_eq!(env.events().len(), 1);
}

#[test]
fn orphan_answers_are_readable_and_never_offer_an_ineffective_done_command() {
    let env = Env::new();
    let id = env.ask("写しで残る質問");
    env.answer(&id, "answer", "残っている回答");
    fs::remove_file(env.room.join("QUESTIONS.jsonl")).unwrap();
    let out = env.run(&["pending"]);
    success(&out);
    assert!(stdout(&out).contains("質問なし（写しから表示）"));
    assert!(stdout(&out).contains("残っている回答"));
    assert!(!stdout(&out).contains("workroom q done"));
    let out = env.run(&["show", "--all"]);
    success(&out);
    assert!(stdout(&out).contains("起票（写し）:"));
    assert_eq!(parsed(&env.run(&["show", "--all", "--json"]))["questions"][0]["ask"], Value::Null);
    assert_eq!(env.run(&["done", &id, "--to", "README"]).status.code(), Some(1));
    assert_eq!(env.run(&["withdraw", &id, "--reason", "理由"]).status.code(), Some(1));
    assert!(!env.room.join("QUESTIONS.jsonl").exists());
}

#[test]
fn pending_from_outside_includes_room_in_the_executable_example() {
    let env = Env::new();
    let id = env.ask("外から読む質問");
    env.answer(&id, "not_needed", "会話で決まった");
    let out = env.cmd().current_dir(&env.base).args(["q", "pending", "--room", "調査"]).output().unwrap();
    success(&out);
    assert!(stdout(&out).contains(&format!("workroom q done {id} --upto 1 --room 'EXAMPLE/2026-11-03-調査'")));
    success(
        &env.cmd()
            .current_dir(&env.base)
            .args(["q", "done", &id, "--upto", "1", "--room", "EXAMPLE/2026-11-03-調査"])
            .output()
            .unwrap(),
    );
    assert_eq!(env.events()[1]["upto"], 1);
}
