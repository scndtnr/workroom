//! `grep`: 全部屋の中身を横断検索する。rg があれば rg に任せ、なければ内蔵の簡易検索を使う。

use crate::config::Settings;
use anyhow::{Context, Result};
use regex::RegexBuilder;
use std::io::Write;
use std::path::Path;
use std::process::Command;

pub fn run(settings: &Settings, pattern: &str, extra: &[String]) -> Result<i32> {
    let root = &settings.root;
    if let Some(code) = run_rg(settings, pattern, extra)? {
        return Ok(code);
    }
    if !extra.is_empty() {
        eprintln!("警告: rg が見つからないため、追加のオプションは無視します");
    }
    let found = builtin(root, pattern, settings)?;
    Ok(if found { 0 } else { 1 })
}

/// rg が無ければ None
fn run_rg(settings: &Settings, pattern: &str, extra: &[String]) -> Result<Option<i32>> {
    let mut cmd = Command::new("rg");
    cmd.current_dir(&settings.root).arg("--smart-case");
    for ex in &settings.config.exclude {
        cmd.arg("--glob").arg(format!("!{ex}"));
    }
    cmd.args(["--glob", "!AGENTS.md"]).args(extra).arg("--").arg(pattern).arg(".");
    match cmd.status() {
        Ok(status) => Ok(Some(status.code().unwrap_or(2))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).context("rg を起動できません"),
    }
}

fn builtin(root: &Path, pattern: &str, settings: &Settings) -> Result<bool> {
    let smart_case = !pattern.chars().any(|c| c.is_uppercase());
    let re = RegexBuilder::new(pattern)
        .case_insensitive(smart_case)
        .build()
        .with_context(|| format!("正規表現が不正です: {pattern}"))?;
    let mut out = std::io::stdout().lock();
    let mut found = false;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = read.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || settings.is_excluded(&name) || name == crate::room::AGENTS {
                continue;
            }
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                stack.push(path);
                continue;
            }
            // 大きいファイル・バイナリは飛ばす
            if entry.metadata().map(|m| m.len() > 8 * 1024 * 1024).unwrap_or(true) {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else { continue };
            if bytes.contains(&0) {
                continue;
            }
            let text = String::from_utf8_lossy(&bytes);
            let rel = path.strip_prefix(root).unwrap_or(&path).display().to_string();
            for (i, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    found = true;
                    writeln!(out, "{rel}:{}:{line}", i + 1)?;
                }
            }
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Settings};

    #[test]
    fn builtin_search_skips_excluded() {
        let d = tempfile::tempdir().unwrap();
        let room = d.path().join("2026-01-01-a");
        std::fs::create_dir_all(room.join("node_modules")).unwrap();
        std::fs::write(room.join("note.md"), "hello Needle\n").unwrap();
        std::fs::write(room.join("node_modules").join("x.js"), "needle").unwrap();
        let settings = Settings {
            config: Config::default(),
            config_path: d.path().join("c.toml"),
            root: d.path().to_path_buf(),
            state_dir: d.path().join("state"),
            claude_dir: d.path().join("claude"),
        };
        assert!(builtin(d.path(), "needle", &settings).unwrap());
        assert!(!builtin(d.path(), "Needlex", &settings).unwrap());
    }
}
