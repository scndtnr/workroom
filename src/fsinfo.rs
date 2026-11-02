//! tree 表示・プレビュー・ディスク使用量の計算。

use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeLine {
    /// 罫線の部分（`│   ├── ` など）
    pub prefix: String,
    pub name: String,
    pub is_dir: bool,
    /// 「… 他 N 件」の行
    pub more: bool,
}

pub struct TreeOptions<'a> {
    pub depth: usize,
    pub max_items: usize,
    pub exclude: &'a [String],
    /// 部屋の直下では README.md / AGENTS.md を省く
    pub hide_room_files: bool,
}

/// ディレクトリの中身を tree 形式の行にする（ディレクトリが先、名前順）
pub fn tree(path: &Path, opts: &TreeOptions) -> Vec<TreeLine> {
    let mut out = Vec::new();
    walk_tree(path, "", opts.depth, true, opts, &mut out);
    out
}

fn walk_tree(path: &Path, prefix: &str, depth: usize, top: bool, opts: &TreeOptions, out: &mut Vec<TreeLine>) {
    if depth == 0 {
        return;
    }
    let Ok(read) = std::fs::read_dir(path) else { return };
    let mut items: Vec<(String, bool)> = read
        .filter_map(|e| e.ok())
        .map(|e| {
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            (e.file_name().to_string_lossy().into_owned(), is_dir)
        })
        .filter(|(name, _)| {
            !name.starts_with('.')
                && !opts.exclude.iter().any(|x| x == name)
                && !(top && opts.hide_room_files && (name == crate::room::README || name == crate::room::AGENTS))
        })
        .collect();
    items.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));

    let shown = items.len().min(opts.max_items);
    let rest = items.len() - shown;
    for (i, (name, is_dir)) in items.into_iter().take(shown).enumerate() {
        let last = i + 1 == shown && rest == 0;
        out.push(TreeLine {
            prefix: format!("{prefix}{}", if last { "└── " } else { "├── " }),
            name: name.clone(),
            is_dir,
            more: false,
        });
        if is_dir {
            let next = format!("{prefix}{}", if last { "    " } else { "│   " });
            walk_tree(&path.join(&name), &next, depth - 1, false, opts, out);
        }
    }
    if rest > 0 {
        out.push(TreeLine {
            prefix: format!("{prefix}└── "),
            name: format!("… 他 {rest} 件"),
            is_dir: false,
            more: true,
        });
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiskUsage {
    pub files: u64,
    pub bytes: u64,
}

/// ファイル数と合計サイズ。除外した名前は数えず、メタデータだけを見て、中身は読まない
pub fn disk_usage(path: &Path, exclude: &[String]) -> DiskUsage {
    disk_usage_cancellable(path, exclude, || false).unwrap_or_default()
}

/// 中断された計算は、途中の集計を返さない。
pub fn disk_usage_cancellable(
    path: &Path,
    exclude: &[String],
    mut cancelled: impl FnMut() -> bool,
) -> Option<DiskUsage> {
    let mut usage = DiskUsage::default();
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if cancelled() {
            return None;
        }
        let Ok(read) = std::fs::read_dir(&dir) else { continue };
        for entry in read.filter_map(|e| e.ok()) {
            if cancelled() {
                return None;
            }
            if exclude.iter().any(|name| entry.file_name() == name.as_str()) {
                continue;
            }
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                usage.files += 1;
                usage.bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    if cancelled() { None } else { Some(usage) }
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit + 1 < UNITS.len() {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} B") } else { format!("{v:.1} {}", UNITS[unit]) }
}

/// README.md の先頭 n 行
pub fn readme_head(room: &Path, n: usize) -> Vec<String> {
    std::fs::read_to_string(room.join(crate::room::README))
        .map(|t| t.trim_start_matches('\u{feff}').lines().take(n).map(str::to_string).collect())
        .unwrap_or_default()
}

/// Nerd Font のアイコン
pub fn icon_for(name: &str, is_dir: bool) -> &'static str {
    if is_dir {
        return "\u{f07b}";
    }
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "md" => "\u{f48a}",
        "py" => "\u{e606}",
        "rs" => "\u{e7a8}",
        "ps1" | "sh" | "bat" | "cmd" | "nu" => "\u{f489}",
        "json" | "toml" | "yaml" | "yml" => "\u{e60b}",
        "js" | "ts" => "\u{e74e}",
        "html" | "htm" => "\u{e736}",
        "csv" | "tsv" | "xlsx" => "\u{f1c3}",
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" => "\u{f1c5}",
        "pdf" => "\u{f1c1}",
        "txt" | "log" => "\u{f15c}",
        _ => "\u{f15b}",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn tree_and_usage() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        fs::write(p.join("README.md"), "# x").unwrap();
        fs::write(p.join("b.txt"), "12345").unwrap();
        fs::create_dir(p.join("docs")).unwrap();
        fs::write(p.join("docs").join("a.md"), "abc").unwrap();
        fs::create_dir(p.join("node_modules")).unwrap();
        fs::write(p.join("node_modules").join("ignored.js"), "除外するファイル").unwrap();
        fs::create_dir(p.join("docs").join("node_modules")).unwrap();
        fs::write(p.join("docs").join("node_modules").join("nested.js"), "これも除外").unwrap();
        fs::write(p.join(".done"), "").unwrap();
        let exclude = vec!["node_modules".to_string()];
        let lines = tree(p, &TreeOptions { depth: 2, max_items: 10, exclude: &exclude, hide_room_files: true });
        let names: Vec<_> = lines.iter().map(|l| format!("{}{}", l.prefix, l.name)).collect();
        assert_eq!(names, vec!["├── docs", "│   └── a.md", "└── b.txt"]);

        let u = disk_usage(p, &exclude);
        assert_eq!(u.files, 4);
        assert_eq!(u.bytes, 3 + 5 + 3);
    }

    #[test]
    fn usage_excludes_files_by_name_too() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("keep.txt"), "123").unwrap();
        fs::write(d.path().join("skip.txt"), "12345").unwrap();
        assert_eq!(disk_usage(d.path(), &["skip.txt".into()]), DiskUsage { files: 1, bytes: 3 });
    }

    #[test]
    fn cancelled_usage_does_not_return_partial_totals() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..3 {
            fs::write(d.path().join(format!("{i}.txt")), "123").unwrap();
        }
        let mut checks = 0;
        let usage = disk_usage_cancellable(d.path(), &[], || {
            checks += 1;
            checks == 3
        });
        assert_eq!(usage, None);
        assert_eq!(checks, 3);
        assert_eq!(disk_usage(d.path(), &[]), DiskUsage { files: 3, bytes: 9 });
    }

    #[test]
    fn tree_truncates() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..5 {
            fs::write(d.path().join(format!("f{i}.txt")), "").unwrap();
        }
        let lines = tree(d.path(), &TreeOptions { depth: 1, max_items: 3, exclude: &[], hide_room_files: false });
        assert_eq!(lines.len(), 4);
        assert!(lines[3].more);
        assert_eq!(lines[3].name, "… 他 2 件");
    }

    #[test]
    fn bytes_format() {
        assert_eq!(human_bytes(10), "10 B");
        assert_eq!(human_bytes(1536), "1.5 KB");
    }
}
