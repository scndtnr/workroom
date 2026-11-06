//! tree 表示・プレビュー・ディスク使用量の計算。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Directory,
    File,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// 一覧の起点からの相対パス（表示名と違い、OS の文字列を保つ）。
    pub path: PathBuf,
    pub name: String,
    pub kind: EntryKind,
    /// 通常ファイルのサイズ。ディレクトリや取得できなかった場合は None。
    pub size: Option<u64>,
    pub modified: Option<SystemTime>,
}

pub struct EntriesOptions<'a> {
    /// 起点の直下を 1 とする。0 なら走査しない。
    pub depth: usize,
    /// ディレクトリも含め、返す項目全体の上限。
    pub max_items: usize,
    pub exclude: &'a [String],
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Entries {
    pub items: Vec<Entry>,
    /// 深さの範囲内で、件数の上限により省いた項目数。
    pub omitted: usize,
}

/// 中身を読まずにメタデータの一覧を作る。ドットで始まる名前と exclude は各階層で除く。
/// ディレクトリを先に、大小文字を区別しない名前順の深さ優先で返す。
/// リンク先はたどらず、読めないディレクトリは空として扱う（tree と同じ）。
pub fn entries(path: &Path, opts: &EntriesOptions) -> Entries {
    let mut out = Entries::default();
    walk_entries(path, Path::new(""), opts.depth, opts, &mut out);
    out
}

fn walk_entries(root: &Path, relative: &Path, depth: usize, opts: &EntriesOptions, out: &mut Entries) {
    if depth == 0 {
        return;
    }
    let Ok(read) = std::fs::read_dir(root.join(relative)) else { return };
    let mut items: Vec<_> = read
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || opts.exclude.iter().any(|x| x == &name) {
                return None;
            }
            let kind = match entry.file_type() {
                Ok(t) if t.is_dir() => EntryKind::Directory,
                Ok(t) if t.is_file() => EntryKind::File,
                Ok(t) if t.is_symlink() => EntryKind::Symlink,
                _ => EntryKind::Other,
            };
            Some((entry, name, kind))
        })
        .collect();
    items.sort_by(|a, b| {
        (b.2 == EntryKind::Directory)
            .cmp(&(a.2 == EntryKind::Directory))
            .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
            .then_with(|| a.0.file_name().cmp(&b.0.file_name()))
    });
    for (entry, name, kind) in items {
        let path = relative.join(entry.file_name());
        if out.items.len() < opts.max_items {
            let metadata = std::fs::symlink_metadata(entry.path()).ok();
            out.items.push(Entry {
                path: path.clone(),
                name,
                kind,
                size: metadata.as_ref().filter(|m| m.is_file()).map(|m| m.len()),
                modified: metadata.and_then(|m| m.modified().ok()),
            });
        } else {
            out.omitted += 1;
        }
        if kind == EntryKind::Directory {
            // 上限を超えた後も深さの範囲内を数えるが、各項目のメタデータは取得しない。
            walk_entries(root, &path, depth - 1, opts, out);
        }
    }
}

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
    fn entries_return_relative_paths_and_metadata_and_exclude_names_at_every_depth() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("docs/sub")).unwrap();
        for (name, contents) in [
            ("README.md", "# 説明"),
            ("b.txt", "12345"),
            ("_visible", "残す"),
            ("docs/a.md", "本文"),
            ("docs/sub/deep.md", "abc"),
            (".done", ""),
            ("docs/.hidden", "除外"),
            ("skip.txt", "除外"),
            ("docs/skip.txt", "除外"),
        ] {
            fs::write(root.join(name), contents).unwrap();
        }
        for name in [".cache", "node_modules", "docs/node_modules"] {
            fs::create_dir_all(root.join(name)).unwrap();
            fs::write(root.join(name).join("ignored.txt"), "除外").unwrap();
        }
        let exclude = vec!["node_modules".into(), "skip.txt".into()];
        let listing = entries(root, &EntriesOptions { depth: 3, max_items: 100, exclude: &exclude });
        let paths: Vec<_> = listing.items.iter().map(|e| e.path.clone()).collect();
        assert_eq!(
            paths,
            ["docs", "docs/sub", "docs/sub/deep.md", "docs/a.md", "_visible", "b.txt", "README.md"].map(PathBuf::from)
        );
        assert_eq!(listing.omitted, 0);
        for entry in listing.items {
            let metadata = fs::symlink_metadata(root.join(&entry.path)).unwrap();
            assert_eq!(entry.name, entry.path.file_name().unwrap().to_string_lossy());
            assert_eq!(entry.modified, Some(metadata.modified().unwrap()));
            if metadata.is_dir() {
                assert_eq!(entry.kind, EntryKind::Directory);
                assert_eq!(entry.size, None);
            } else {
                assert_eq!(entry.kind, EntryKind::File);
                assert_eq!(entry.size, Some(metadata.len()));
            }
        }
    }

    #[test]
    fn entries_limit_depth_and_total_items_and_count_omissions_within_depth() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for name in ["a", "b"] {
            fs::create_dir(root.join(name)).unwrap();
        }
        for name in ["a/1.txt", "a/2.txt", "b/1.txt", "c.txt"] {
            fs::write(root.join(name), "").unwrap();
        }
        for (depth, max_items, expected, omitted) in [
            (0, 100, vec![], 0),
            (1, 100, vec!["a", "b", "c.txt"], 0),
            (1, 2, vec!["a", "b"], 1),
            (1, 0, vec![], 3),
            (2, 3, vec!["a", "a/1.txt", "a/2.txt"], 3),
            (2, 0, vec![], 6),
            (2, 100, vec!["a", "a/1.txt", "a/2.txt", "b", "b/1.txt", "c.txt"], 0),
        ] {
            let listing = entries(root, &EntriesOptions { depth, max_items, exclude: &[] });
            assert_eq!(
                listing.items.iter().map(|e| e.path.clone()).collect::<Vec<_>>(),
                expected.into_iter().map(PathBuf::from).collect::<Vec<_>>(),
                "depth={depth}, max_items={max_items}"
            );
            assert_eq!(listing.omitted, omitted, "depth={depth}, max_items={max_items}");
        }
    }

    #[test]
    fn entries_return_empty_for_missing_paths_and_files_without_creating_anything() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file");
        fs::write(&file, "").unwrap();
        let opts = EntriesOptions { depth: 4, max_items: 500, exclude: &[] };
        assert_eq!(entries(&dir.path().join("missing"), &opts), Entries::default());
        assert_eq!(entries(&file, &opts), Entries::default());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn entries_list_symlinks_without_following_them() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let target = dir.path().join("target");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&target).unwrap();
        fs::write(target.join("outside.txt"), "一覧の外").unwrap();
        let link = root.join("link");
        #[cfg(unix)]
        let result = std::os::unix::fs::symlink(&target, &link);
        #[cfg(windows)]
        let result = std::os::windows::fs::symlink_dir(&target, &link);
        if let Err(error) = result {
            #[cfg(windows)]
            if error.kind() == std::io::ErrorKind::PermissionDenied || error.raw_os_error() == Some(1314) {
                eprintln!("シンボリックリンクの作成権限がないためスキップ: {error}");
                return;
            }
            panic!("シンボリックリンクを作れません: {error}");
        }
        let listing = entries(&root, &EntriesOptions { depth: 4, max_items: 500, exclude: &[] });
        assert_eq!(listing.items.len(), 1);
        assert_eq!(listing.items[0].kind, EntryKind::Symlink);
        assert_eq!(listing.items[0].path, PathBuf::from("link"));
        assert_eq!(listing.items[0].size, None);
        assert_eq!(listing.omitted, 0);
    }

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
