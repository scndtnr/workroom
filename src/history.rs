//! 「最近使った順」のための使用履歴。
//!
//! 作業ルート（クラウド同期されていることがある）ではなく、ローカルの状態ディレクトリに保存する。

use anyhow::Result;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct History {
    file: PathBuf,
    /// パス → 最終使用日時（UNIX 秒）
    entries: BTreeMap<String, i64>,
}

impl History {
    pub fn load(state_dir: &Path) -> Self {
        let file = state_dir.join("history.json");
        let entries =
            std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Self { file, entries }
    }

    pub fn last_used(&self, path: &Path) -> Option<i64> {
        self.entries.get(&key(path)).copied()
    }

    pub fn touch(&mut self, path: &Path) {
        self.entries.insert(key(path), chrono::Local::now().timestamp());
    }

    /// 存在しなくなったパスを除いて保存する
    pub fn save(&mut self) -> Result<()> {
        self.entries.retain(|k, _| Path::new(k).exists());
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&self.file, serde_json::to_string_pretty(&self.entries)?)?;
        Ok(())
    }
}

/// Windows ではパスの大文字・小文字を区別しない
fn key(path: &Path) -> String {
    let s = path.to_string_lossy().trim_end_matches(['\\', '/']).to_string();
    if cfg!(windows) { s.to_lowercase() } else { s }
}

/// 使用履歴を記録する（失敗しても本来の処理は止めない）
pub fn record(state_dir: &Path, path: &Path) {
    let mut h = History::load(state_dir);
    h.touch(path);
    if let Err(e) = h.save() {
        eprintln!("警告: 使用履歴を保存できません: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touch_and_reload() {
        let state = tempfile::tempdir().unwrap();
        let room = tempfile::tempdir().unwrap();
        record(state.path(), room.path());
        let h = History::load(state.path());
        assert!(h.last_used(room.path()).is_some());
        assert!(h.last_used(&room.path().join("nope")).is_none());
    }

    #[test]
    fn missing_paths_are_pruned() {
        let state = tempfile::tempdir().unwrap();
        let gone = state.path().join("gone");
        std::fs::create_dir(&gone).unwrap();
        record(state.path(), &gone);
        std::fs::remove_dir(&gone).unwrap();
        record(state.path(), state.path());
        let h = History::load(state.path());
        assert!(h.last_used(&gone).is_none());
    }
}
