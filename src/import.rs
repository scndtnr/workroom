//! `import`: 既存のディレクトリを作業ルートへ移行する。
//!
//! Claude Code の会話履歴・メモリ（`<claude_dir>/projects/<パス由来の名前>/`）も移行先用にコピーし、
//! 履歴内の `"cwd"` を書き換えて、移行先でも resume できるようにする。
//! これは Claude Code の非公式な内部形式に依存しているため、形式が変わると動かなくなる可能性がある。

use crate::config::Settings;
use crate::room::{self, AGENTS, README};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Local, NaiveDate};
use regex::Regex;
use std::path::{Path, PathBuf};

pub struct ImportArgs<'a> {
    pub source: &'a Path,
    pub slug: Option<&'a str>,
    pub description: &'a str,
    pub date: Option<NaiveDate>,
    pub project: Option<&'a str>,
}

pub fn run(settings: &Settings, args: &ImportArgs) -> Result<PathBuf> {
    let src = std::fs::canonicalize(args.source)
        .with_context(|| format!("ディレクトリがありません: {}", args.source.display()))?;
    let src = strip_verbatim(src);
    if !src.is_dir() {
        bail!("ディレクトリではありません: {}", src.display());
    }
    // 移行元と同じく正規化して比べる（8.3 形式の短いパスなどで判定をすり抜けないように）
    let cwd = std::env::current_dir()?;
    let cwd = strip_verbatim(std::fs::canonicalize(&cwd).unwrap_or(cwd));
    if cwd.starts_with(&src) {
        bail!("移行元の中にいるため移動できません。別のディレクトリに移動してから実行してください");
    }

    let leaf = src.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let (name_date, rest) = match room::parse_room_name(&leaf) {
        Some((d, s)) => (Some(d), s),
        None => (None, leaf.clone()),
    };
    let date = match args.date.or(name_date) {
        Some(d) => d,
        None => {
            let created = std::fs::metadata(&src)?.created().or_else(|_| std::fs::metadata(&src)?.modified())?;
            DateTime::<Local>::from(created).date_naive()
        }
    };
    let target = room::parse_new_name(args.slug.unwrap_or(&rest), args.project, settings.config.project_uppercase)
        .with_context(|| {
            format!("部屋の名前を指定してください: workroom import \"{}\" <名前> [説明]", src.display())
        })?;
    let slug = &target.slug;
    let dest = target.path(&settings.root, date);
    if dest.exists() {
        bail!("移行先が既にあります: {}", dest.display());
    }
    room::check_claude_collision(settings, &dest)?;
    room::ensure_root(settings)?;
    if let Some(p) = &target.project {
        room::ensure_project(settings, p)?;
    }

    eprintln!("移行: {}", src.display());
    eprintln!("  → {}", dest.display());
    copy_dir(&src, &dest)?;
    let (a, b) = (count_entries(&src), count_entries(&dest));
    if a != b {
        bail!("コピー後のファイル数が一致しません（{a} → {b}）。移行元は残しています");
    }

    if !dest.join(README).exists() {
        let title = if args.description.is_empty() { slug.as_str() } else { args.description };
        room::write_text(
            &dest.join(README),
            &format!(
                "# {title}\n\n- created: {date}\n- imported: {} from `{}`\n",
                Local::now().format("%Y-%m-%d %H:%M"),
                src.display()
            ),
        )?;
    }
    if dest.join(AGENTS).exists() {
        eprintln!("警告: AGENTS.md が既にあるため、ルールのテンプレートはコピーしていません");
    } else {
        crate::rules::write_room_rules(settings, &dest, target.project.as_deref())?;
    }

    room::warn_existing_claude_history(settings, &dest)?;
    copy_claude_project(settings, &src, &dest)?;
    std::fs::remove_dir_all(&src).with_context(|| format!("移行元を削除できません: {}", src.display()))?;
    eprintln!("完了しました: {}", dest.display());
    Ok(dest)
}

/// Windows の `\\?\` 付きパスを通常のパスに戻す
fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => p,
    }
}

fn copy_dir(src: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir(dest).with_context(|| format!("作れません: {}", dest.display()))?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let to = dest.join(entry.file_name());
        let ft = entry.file_type()?;
        if ft.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else if ft.is_file() {
            std::fs::copy(entry.path(), &to)
                .with_context(|| format!("コピーできません: {}", entry.path().display()))?;
        } else {
            bail!("シンボリックリンクなどは移行できません: {}", entry.path().display());
        }
    }
    Ok(())
}

fn count_entries(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok()).map(|e| if e.path().is_dir() { 1 + count_entries(&e.path()) } else { 1 }).sum()
        })
        .unwrap_or(0)
}

fn copy_claude_project(settings: &Settings, from: &Path, to: &Path) -> Result<()> {
    let projects = settings.claude_dir.join("projects");
    let (old_key, new_key) = (room::claude_history_key(from)?, room::claude_history_key(to)?);
    let old_dir = projects.join(&old_key);
    let new_dir = projects.join(&new_key);
    if !old_dir.is_dir() {
        eprintln!("  Claude の履歴なし");
        return Ok(());
    }
    if old_key.len() > 200 || new_key.len() > 200 {
        eprintln!("警告: パスが長く Claude 側の名前規則が変わるため、履歴は移行しません: {old_key}");
        return Ok(());
    }
    if new_dir.exists() {
        eprintln!("警告: 移行先の Claude 履歴が既にあるため移行しません: {}", new_dir.display());
        return Ok(());
    }
    copy_dir(&old_dir, &new_dir)?;
    let to = std::path::absolute(to)?;
    let count = rewrite_cwd(&new_dir, from, &to)?;
    eprintln!("  Claude の履歴をコピーしました（cwd 書き換え: {count} ファイル）: {new_key}");
    eprintln!("  元の履歴は残しています: {}", old_dir.display());
    Ok(())
}

/// jsonl 内の `"cwd":"<from>..."` を `<to>` に書き換える。Windows 形式（`\\` エスケープ）と `/c/...` 形式の両方
fn rewrite_cwd(dir: &Path, from: &Path, to: &Path) -> Result<usize> {
    let pairs = [
        (json_escape(&from.to_string_lossy()), json_escape(&to.to_string_lossy())),
        (posix_style(from), posix_style(to)),
    ];
    let res: Vec<(Regex, String)> = pairs
        .iter()
        .filter(|(a, _)| !a.is_empty())
        .map(|(a, b)| {
            // 後ろが " \ / のものだけ（似た名前の別ディレクトリは対象外）
            let re = Regex::new(&format!(r#""cwd":"{}(["\\/])"#, regex::escape(a))).expect("escaped regex");
            (re, format!(r#""cwd":"{}"#, b.replace('$', "$$")))
        })
        .collect();

    let mut count = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "jsonl") {
                continue;
            }
            let text = std::fs::read_to_string(&path)?;
            let mut updated = text.clone();
            for (re, rep) in &res {
                updated = re.replace_all(&updated, format!("{rep}${{1}}")).into_owned();
            }
            if updated != text {
                std::fs::write(&path, updated)?;
                count += 1;
            }
        }
    }
    Ok(count)
}

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
}

/// `C:\Users\x` → `/c/Users/x`（Windows 以外ではそのまま）
fn posix_style(p: &Path) -> String {
    let s = p.to_string_lossy();
    let b = s.as_bytes();
    if b.len() >= 2 && b[1] == b':' {
        format!("/{}{}", s[..1].to_lowercase(), s[2..].replace('\\', "/"))
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_key() {
        assert_eq!(room::claude_project_key(Path::new(r"D:\作業場\work")), "D------work");
    }

    #[test]
    fn cwd_rewrite() {
        let d = tempfile::tempdir().unwrap();
        let from = Path::new(r"C:\tmp\old_dir");
        let to = Path::new(r"G:\work\2026-01-01-new");
        let lines = [
            r#"{"cwd":"C:\\tmp\\old_dir","text":"C:\\tmp\\old_dir stays"}"#,
            r#"{"cwd":"C:\\tmp\\old_dir\\sub"}"#,
            r#"{"cwd":"/c/tmp/old_dir"}"#,
            r#"{"cwd":"C:\\tmp\\old_dir_other"}"#,
        ];
        std::fs::create_dir(d.path().join("s")).unwrap();
        std::fs::write(d.path().join("a.jsonl"), lines.join("\n")).unwrap();
        std::fs::write(d.path().join("s").join("b.jsonl"), lines[0]).unwrap();
        std::fs::write(d.path().join("memo.md"), lines[0]).unwrap();
        assert_eq!(rewrite_cwd(d.path(), from, to).unwrap(), 2);
        let a = std::fs::read_to_string(d.path().join("a.jsonl")).unwrap();
        let got: Vec<&str> = a.lines().collect();
        assert_eq!(got[0], r#"{"cwd":"G:\\work\\2026-01-01-new","text":"C:\\tmp\\old_dir stays"}"#);
        assert_eq!(got[1], r#"{"cwd":"G:\\work\\2026-01-01-new\\sub"}"#);
        assert_eq!(got[2], r#"{"cwd":"/g/work/2026-01-01-new"}"#);
        assert_eq!(got[3], lines[3]);
        assert_eq!(std::fs::read_to_string(d.path().join("memo.md")).unwrap(), lines[0]);
    }
}
