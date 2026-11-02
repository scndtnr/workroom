//! プレビューの段階的な読み込みと、部屋のパスごとのキャッシュ。

use crate::fsinfo::{self, DiskUsage, TreeLine, TreeOptions};
use std::collections::HashMap;
use std::panic::{self, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;

const WORKER_NAME: &str = "workroom-preview";

/// プレビューに出す README の行数（多いと tree とサイズが欄からはみ出す）
const README_LINES: usize = 10;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Preview {
    pub readme: Option<Vec<String>>,
    pub tree: Option<Vec<TreeLine>>,
    pub usage: Option<DiskUsage>,
}

impl Preview {
    fn is_complete(&self) -> bool {
        self.readme.is_some() && self.tree.is_some() && self.usage.is_some()
    }

    fn fill_missing(&mut self) {
        self.readme.get_or_insert_with(Vec::new);
        self.tree.get_or_insert_with(Vec::new);
        self.usage.get_or_insert_default();
    }

    fn apply(&mut self, part: Part) {
        match part {
            Part::Readme(lines) => self.readme = Some(lines),
            Part::Tree(lines) => self.tree = Some(lines),
            Part::Usage(usage) => self.usage = Some(usage),
        }
    }
}

struct Request {
    path: PathBuf,
    generation: u64,
}

#[derive(Debug, PartialEq, Eq)]
enum Part {
    Readme(Vec<String>),
    Tree(Vec<TreeLine>),
    Usage(DiskUsage),
}

struct Update {
    path: PathBuf,
    part: Part,
}

pub struct Previews {
    cache: HashMap<PathBuf, Preview>,
    selected: Option<PathBuf>,
    requests: Option<Sender<Request>>,
    updates: Receiver<Update>,
    generation: Arc<AtomicU64>,
}

impl Previews {
    pub fn new(exclude: Vec<String>) -> Self {
        let (requests, receiver) = mpsc::channel();
        let (sender, updates) = mpsc::channel();
        let generation = Arc::new(AtomicU64::new(0));
        let worker_generation = Arc::clone(&generation);
        // 終了時には待たない。ファイルシステムの応答が遅くても TUI を閉じられる。
        let worker = thread::Builder::new()
            .name(WORKER_NAME.into())
            .spawn(move || worker_loop(receiver, sender, &worker_generation, &exclude));
        Self { cache: HashMap::new(), selected: None, requests: worker.ok().map(|_| requests), updates, generation }
    }

    /// 選択変更で古い計算を中断する。未完了の部屋は、選び直すたびに再要求できる。
    pub fn select(&mut self, path: Option<&Path>) {
        if self.selected.as_deref() == path {
            return;
        }
        self.selected = path.map(Path::to_path_buf);
        let generation = self.generation.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let Some(path) = path else { return };
        let preview = self.cache.entry(path.to_path_buf()).or_default();
        if preview.is_complete() {
            return;
        }
        if let Some(sender) = &self.requests
            && sender.send(Request { path: path.to_path_buf(), generation }).is_ok()
        {
            return;
        }
        // スレッドを起動できない・停止した場合も、UI は空のプレビューで動き続ける。
        preview.fill_missing();
    }

    pub fn get(&self, path: &Path) -> Option<&Preview> {
        self.cache.get(path)
    }

    /// 届いている結果だけを反映し、待たずに戻る。変更があれば再描画する。
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        loop {
            match self.updates.try_recv() {
                Ok(update) => {
                    self.cache.entry(update.path).or_default().apply(update.part);
                    changed = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if self.requests.take().is_some() {
                        for preview in self.cache.values_mut() {
                            preview.fill_missing();
                        }
                        changed = true;
                    }
                    break;
                }
            }
        }
        changed
    }
}

impl Drop for Previews {
    fn drop(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }
}

/// ratatui の初期化後に設定する。捕捉するパニックで端末を復元したり、標準エラーへ書いたりしない。
pub fn install_panic_hook() {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        if thread::current().name() != Some(WORKER_NAME) {
            previous(info);
        }
    }));
}

fn worker_loop(requests: Receiver<Request>, updates: Sender<Update>, generation: &AtomicU64, exclude: &[String]) {
    while let Ok(mut request) = requests.recv() {
        // 読み込み中に溜まった要求を捨てて、最新の選択から処理する。
        for newer in requests.try_iter() {
            request = newer;
        }
        load_preview(&request, &updates, generation, exclude);
    }
}

fn load_preview(request: &Request, updates: &Sender<Update>, generation: &AtomicU64, exclude: &[String]) {
    let current = || generation.load(Ordering::Relaxed) == request.generation;
    let send = |part| current() && updates.send(Update { path: request.path.clone(), part }).is_ok();
    if !current() {
        return;
    }
    let readme = catch_unwind(|| fsinfo::readme_head(&request.path, README_LINES)).unwrap_or_default();
    if !send(Part::Readme(readme)) {
        return;
    }
    let tree = catch_unwind(|| {
        fsinfo::tree(&request.path, &TreeOptions { depth: 2, max_items: 10, exclude, hide_room_files: true })
    })
    .unwrap_or_default();
    if !send(Part::Tree(tree)) {
        return;
    }
    let usage = catch_unwind(|| fsinfo::disk_usage_cancellable(&request.path, exclude, || !current()))
        .unwrap_or(Some(DiskUsage::default()));
    if let Some(usage) = usage {
        send(Part::Usage(usage));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use std::time::Duration;

    fn receive(updates: &Receiver<Update>) -> Update {
        // タイムアウトは失敗時のハング防止用。処理速度や sleep による順序付けは使わない。
        updates.recv_timeout(Duration::from_secs(10)).unwrap()
    }

    fn channels() -> (Previews, Receiver<Request>, Sender<Update>) {
        let (requests, receiver) = mpsc::channel();
        let (sender, updates) = mpsc::channel();
        (
            Previews {
                cache: HashMap::new(),
                selected: None,
                requests: Some(requests),
                updates,
                generation: Arc::new(AtomicU64::new(0)),
            },
            receiver,
            sender,
        )
    }

    #[test]
    fn worker_delivers_readme_tree_and_usage_in_order_with_exclusions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        let readme = (0..35).map(|i| format!("行{i}\n")).collect::<String>();
        fs::write(path.join("README.md"), &readme).unwrap();
        fs::write(path.join("AGENTS.md"), "ルール").unwrap();
        fs::create_dir(path.join("docs")).unwrap();
        fs::write(path.join("docs").join("a.txt"), "123").unwrap();
        fs::create_dir_all(path.join("docs").join("deep")).unwrap();
        fs::write(path.join("docs").join("deep").join("b.txt"), "45").unwrap();
        for base in [path.to_path_buf(), path.join("docs")] {
            fs::create_dir(base.join("node_modules")).unwrap();
            fs::write(base.join("node_modules").join("ignored.js"), "除外").unwrap();
        }
        let mut previews = Previews::new(vec!["node_modules".into()]);
        previews.select(Some(path));
        let mut parts = Vec::new();
        for _ in 0..3 {
            let update = receive(&previews.updates);
            assert_eq!(update.path, path);
            parts.push(update.part);
        }
        assert_eq!(parts[0], Part::Readme((0..README_LINES).map(|i| format!("行{i}")).collect()));
        let Part::Tree(lines) = &parts[1] else { panic!("tree が2番目に届いていません") };
        assert_eq!(lines.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["docs", "deep", "a.txt"]);
        assert_eq!(parts[2], Part::Usage(DiskUsage { files: 4, bytes: (readme.len() + "ルール".len() + 5) as u64 }));
    }

    #[test]
    fn worker_skips_queued_requests_and_they_can_be_selected_again() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        for path in [&a, &b] {
            fs::create_dir(path).unwrap();
            fs::write(path.join("README.md"), "# テスト").unwrap();
        }
        let (mut previews, receiver, sender) = channels();
        // ワーカーの開始前に要求を詰めるので、スケジューリングには依存しない。
        previews.select(Some(&a));
        previews.select(Some(&b));
        let generation = Arc::clone(&previews.generation);
        let worker = thread::spawn(move || worker_loop(receiver, sender, &generation, &[]));
        for _ in 0..3 {
            let update = receive(&previews.updates);
            assert_eq!(update.path, b);
            previews.cache.entry(update.path).or_default().apply(update.part);
        }
        assert!(!previews.get(&a).unwrap().is_complete());
        previews.select(Some(&a));
        for _ in 0..3 {
            let update = receive(&previews.updates);
            assert_eq!(update.path, a);
        }
        drop(previews);
        worker.join().unwrap();
    }

    #[test]
    fn partial_cache_is_retried_but_complete_cache_is_reused() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        let (mut previews, requests, updates) = channels();
        previews.select(Some(&a));
        let original = requests.try_recv().unwrap();
        previews.select(Some(&a));
        assert!(requests.try_recv().is_err());
        updates.send(Update { path: a.clone(), part: Part::Readme(vec!["# テスト".into()]) }).unwrap();
        assert!(previews.poll());
        assert!(!previews.poll());
        assert_eq!(previews.get(&a).unwrap().readme.as_ref().unwrap(), &["# テスト"]);
        assert!(previews.get(&a).unwrap().usage.is_none());

        previews.select(Some(&b));
        requests.try_recv().unwrap();
        // 古い要求は結果を返さず、途中のサイズもキャッシュしない。
        load_preview(&original, &updates, &previews.generation, &[]);
        assert!(!previews.poll());
        previews.select(Some(&a));
        assert_ne!(requests.try_recv().unwrap().generation, original.generation);
        updates.send(Update { path: a.clone(), part: Part::Tree(Vec::new()) }).unwrap();
        updates.send(Update { path: a.clone(), part: Part::Usage(DiskUsage::default()) }).unwrap();
        assert!(previews.poll());
        previews.select(None);
        previews.select(Some(&a));
        assert!(requests.try_recv().is_err());
    }

    #[test]
    fn unreadable_room_delivers_empty_parts_and_worker_disconnect_keeps_partial_cache() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing");
        let mut previews = Previews::new(Vec::new());
        previews.select(Some(&path));
        assert_eq!(receive(&previews.updates).part, Part::Readme(Vec::new()));
        assert_eq!(receive(&previews.updates).part, Part::Tree(Vec::new()));
        assert_eq!(receive(&previews.updates).part, Part::Usage(DiskUsage::default()));

        let (mut previews, requests, updates) = channels();
        previews.select(Some(&path));
        updates.send(Update { path: path.clone(), part: Part::Readme(vec!["読めた行".into()]) }).unwrap();
        drop(requests);
        drop(updates);
        assert!(previews.poll());
        assert!(!previews.poll());
        let preview = previews.get(&path).unwrap();
        assert!(preview.is_complete());
        assert_eq!(preview.readme.as_ref().unwrap(), &["読めた行"]);
        let other = dir.path().join("other");
        previews.select(Some(&other));
        assert!(previews.get(&other).unwrap().is_complete());
    }

    #[test]
    fn worker_panics_do_not_restore_terminal_or_write_stderr() {
        const CHILD_ENV: &str = "WORKROOM_TEST_PREVIEW_PANIC";
        if std::env::var_os(CHILD_ENV).is_some() {
            // パニックフックはプロセス全体の状態なので、他のテストから隔離する。
            let calls = Arc::new(AtomicU64::new(0));
            let hook_calls = Arc::clone(&calls);
            panic::set_hook(Box::new(move |_| {
                hook_calls.fetch_add(1, Ordering::Relaxed);
                eprintln!("端末復元フックが呼ばれました");
            }));
            install_panic_hook();
            let dir = tempfile::tempdir().unwrap();
            let (mut previews, requests, updates) = channels();
            previews.select(Some(dir.path()));
            let worker = thread::Builder::new()
                .name(WORKER_NAME.into())
                .spawn(move || {
                    let _channels = (requests, updates);
                    let empty: Vec<String> = catch_unwind(|| panic!("読み込み時のパニック")).unwrap_or_default();
                    assert!(empty.is_empty());
                    panic!("ワーカー停止時のパニック");
                })
                .unwrap();
            assert!(worker.join().is_err());
            assert_eq!(calls.load(Ordering::Relaxed), 0);
            assert!(previews.poll());
            assert!(previews.get(dir.path()).unwrap().is_complete());
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tui::preview::tests::worker_panics_do_not_restore_terminal_or_write_stderr",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .env("WORKROOM_ROOT", dir.path().join("rooms"))
            .env("WORKROOM_CONFIG", dir.path().join("config.toml"))
            .env("WORKROOM_STATE_DIR", dir.path().join("state"))
            .env("WORKROOM_CLAUDE_DIR", dir.path().join("claude"))
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
        assert!(output.stderr.is_empty(), "{}", String::from_utf8_lossy(&output.stderr));
    }
}
