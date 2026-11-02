//! TUI ピッカーの状態（検索・絞り込み・並び替え・選択）。描画とは分けてあり、単体テストできる。

use crate::room::{self, Room};
use chrono::NaiveTime;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    Recent,
    Created,
    Name,
}

impl SortMode {
    fn next(self) -> Self {
        match self {
            Self::Recent => Self::Created,
            Self::Created => Self::Name,
            Self::Name => Self::Recent,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Recent => "最近使った順",
            Self::Created => "作成日順",
            Self::Name => "名前順",
        }
    }
}

/// 一覧の1件
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub room: Room,
    /// 最終使用日時（UNIX 秒）。履歴になければ None
    pub last_used: Option<i64>,
}

impl Item {
    /// 「最近使った順」の基準。履歴になければ作成日を使う
    fn recent_key(&self) -> i64 {
        self.last_used.unwrap_or_else(|| self.room.date.and_time(NaiveTime::MIN).and_utc().timestamp())
    }

    fn haystack(&self) -> String {
        format!("{} {}", self.room.label(), self.room.description)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Input(char),
    Backspace,
    Clear,
    Up,
    Down,
    Enter,
    ToggleDone,
    Open,
    CycleSort,
    ToggleShowDone,
    TogglePreview,
    Quit,
}

/// 状態の外で行う処理（ファイルの変更・終了など）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    None,
    Quit,
    Enter(PathBuf),
    Create(String),
    SetDone(PathBuf, bool),
    Open(PathBuf),
}

/// 表示する行
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row<'a> {
    Room(&'a Item),
    /// 「+ 新規作成」の行。slug にできない検索語なら None
    Create(Option<&'a str>),
}

pub struct State {
    items: Vec<Item>,
    query: String,
    sort: SortMode,
    show_done: bool,
    show_preview: bool,
    /// 表示する部屋（items の添字）
    visible: Vec<usize>,
    selected: usize,
    create_slug: Option<String>,
    status: Option<String>,
    matcher: Matcher,
}

impl State {
    pub fn new(items: Vec<Item>, query: &str) -> Self {
        let mut s = Self {
            items,
            query: query.to_string(),
            sort: SortMode::Recent,
            show_done: false,
            show_preview: true,
            visible: Vec::new(),
            selected: 0,
            create_slug: None,
            status: None,
            matcher: Matcher::new(Config::DEFAULT),
        };
        s.refresh(None);
        s
    }

    pub fn query(&self) -> &str {
        &self.query
    }
    pub fn sort(&self) -> SortMode {
        self.sort
    }
    pub fn show_done(&self) -> bool {
        self.show_done
    }
    pub fn show_preview(&self) -> bool {
        self.show_preview
    }
    pub fn selected(&self) -> usize {
        self.selected
    }
    pub fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }
    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status = Some(msg.into());
    }
    pub fn total(&self) -> usize {
        self.items.len()
    }
    /// 表示している部屋の数（「+ 新規作成」の行は含まない）
    pub fn visible_count(&self) -> usize {
        self.visible.len()
    }
    /// 完了済みのため隠している部屋の数
    pub fn hidden_done(&self) -> usize {
        if self.show_done { 0 } else { self.items.iter().filter(|i| i.room.done).count() }
    }

    fn has_create_row(&self) -> bool {
        !self.query.trim().is_empty()
    }

    pub fn row_count(&self) -> usize {
        self.visible.len() + usize::from(self.has_create_row())
    }

    pub fn rows(&self) -> impl Iterator<Item = Row<'_>> {
        let rooms = self.visible.iter().map(|&i| Row::Room(&self.items[i]));
        let create = self.has_create_row().then_some(Row::Create(self.create_slug.as_deref()));
        rooms.chain(create)
    }

    pub fn selected_item(&self) -> Option<&Item> {
        self.visible.get(self.selected).map(|&i| &self.items[i])
    }

    fn selected_path(&self) -> Option<PathBuf> {
        self.selected_item().map(|i| i.room.path.clone())
    }

    /// 絞り込みと並び替えをやり直す。`keep` の部屋が残っていれば、それを選んだままにする
    fn refresh(&mut self, keep: Option<PathBuf>) {
        let q = self.query.trim();
        self.create_slug = if q.is_empty() {
            None
        } else if room::is_valid_slug(q) {
            Some(q.to_string())
        } else {
            Some(room::derive_slug(q)).filter(|s| room::is_valid_slug(s))
        };

        let candidates = (0..self.items.len()).filter(|&i| self.show_done || !self.items[i].room.done);
        let mut scored: Vec<(usize, u32)> = if q.is_empty() {
            candidates.map(|i| (i, 0)).collect()
        } else {
            let pattern = Pattern::parse(q, CaseMatching::Smart, Normalization::Smart);
            let mut buf = Vec::new();
            candidates
                .filter_map(|i| {
                    let hay = self.items[i].haystack();
                    pattern.score(Utf32Str::new(&hay, &mut buf), &mut self.matcher).map(|s| (i, s))
                })
                .collect()
        };
        let sort = self.sort;
        let items = &self.items;
        scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| compare(sort, &items[a.0], &items[b.0])));
        self.visible = scored.into_iter().map(|(i, _)| i).collect();

        let kept = keep.and_then(|p| self.visible.iter().position(|&i| self.items[i].room.path == p));
        self.selected = kept.unwrap_or(self.selected).min(self.row_count().saturating_sub(1));
    }

    pub fn apply(&mut self, action: Action) -> Effect {
        self.status = None;
        match action {
            Action::Input(c) => {
                self.query.push(c);
                self.selected = 0;
                self.refresh(None);
            }
            Action::Backspace => {
                self.query.pop();
                self.selected = 0;
                self.refresh(None);
            }
            Action::Clear => {
                self.query.clear();
                self.selected = 0;
                self.refresh(None);
            }
            Action::Up => self.selected = self.selected.saturating_sub(1),
            Action::Down => self.selected = (self.selected + 1).min(self.row_count().saturating_sub(1)),
            Action::Enter => {
                if let Some(item) = self.selected_item() {
                    return Effect::Enter(item.room.path.clone());
                }
                if self.has_create_row() {
                    match &self.create_slug {
                        Some(slug) => return Effect::Create(slug.clone()),
                        None => self.set_status("この検索語は slug にできません（英小文字・数字・ハイフンで入力）"),
                    }
                }
            }
            Action::ToggleDone => {
                if let Some(item) = self.selected_item() {
                    return Effect::SetDone(item.room.path.clone(), !item.room.done);
                }
            }
            Action::Open => {
                if let Some(item) = self.selected_item() {
                    return Effect::Open(item.room.path.clone());
                }
            }
            Action::CycleSort => {
                self.sort = self.sort.next();
                let keep = self.selected_path();
                self.refresh(keep);
            }
            Action::ToggleShowDone => {
                self.show_done = !self.show_done;
                let keep = self.selected_path();
                self.refresh(keep);
            }
            Action::TogglePreview => self.show_preview = !self.show_preview,
            Action::Quit => return Effect::Quit,
        }
        Effect::None
    }

    /// 完了マーカーを付け外ししたあとに呼ぶ（隠れる場合は選択位置を詰める）
    pub fn mark_done(&mut self, path: &Path, done: bool) {
        if let Some(item) = self.items.iter_mut().find(|i| i.room.path == path) {
            item.room.done = done;
        }
        self.refresh(Some(path.to_path_buf()));
    }
}

fn compare(sort: SortMode, a: &Item, b: &Item) -> Ordering {
    match sort {
        SortMode::Recent => b.recent_key().cmp(&a.recent_key()).then_with(|| b.room.name.cmp(&a.room.name)),
        SortMode::Created => b.room.name.cmp(&a.room.name).then_with(|| a.room.project.cmp(&b.room.project)),
        SortMode::Name => a
            .room
            .slug
            .cmp(&b.room.slug)
            .then_with(|| a.room.project.cmp(&b.room.project))
            .then_with(|| b.room.date.cmp(&a.room.date)),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use chrono::NaiveDate;

    pub(crate) fn item(name: &str, project: Option<&str>, desc: &str, done: bool, last_used: Option<i64>) -> Item {
        let (date, slug) = room::parse_room_name(name).unwrap();
        let path = match project {
            Some(p) => PathBuf::from("/w").join(p).join(name),
            None => PathBuf::from("/w").join(name),
        };
        Item {
            room: Room {
                name: name.to_string(),
                path,
                project: project.map(str::to_string),
                date,
                slug,
                description: desc.to_string(),
                done,
            },
            last_used,
        }
    }

    fn ts(y: i32, m: u32, d: u32) -> i64 {
        NaiveDate::from_ymd_opt(y, m, d).unwrap().and_time(NaiveTime::MIN).and_utc().timestamp()
    }

    fn sample() -> Vec<Item> {
        vec![
            item("2026-11-02-fix-bug", None, "不具合調査", false, None),
            item("2026-11-01-report", Some("acme"), "月次レポート作成", false, Some(ts(2026, 11, 3))),
            item("2026-10-01-old-done", None, "", true, None),
            item("2026-06-14-novel-review", None, "小説の講評", false, Some(ts(2026, 11, 2) + 60)),
        ]
    }

    fn names(s: &State) -> Vec<String> {
        s.rows()
            .map(|r| match r {
                Row::Room(i) => i.room.label(),
                Row::Create(slug) => format!("+{}", slug.unwrap_or("?")),
            })
            .collect()
    }

    fn type_str(s: &mut State, text: &str) {
        for c in text.chars() {
            s.apply(Action::Input(c));
        }
    }

    #[test]
    fn recent_order_falls_back_to_room_date() {
        let s = State::new(sample(), "");
        // report(11/3 使用) > novel-review(11/2 00:01 使用) > fix-bug(作成 11/2 00:00)。完了済みは隠れる
        assert_eq!(names(&s), ["acme/2026-11-01-report", "2026-06-14-novel-review", "2026-11-02-fix-bug"]);
        assert_eq!(s.hidden_done(), 1);
    }

    #[test]
    fn sort_cycles_and_keeps_selection() {
        let mut s = State::new(sample(), "");
        s.apply(Action::Down); // novel-review
        s.apply(Action::CycleSort);
        assert_eq!(s.sort(), SortMode::Created);
        assert_eq!(names(&s), ["2026-11-02-fix-bug", "acme/2026-11-01-report", "2026-06-14-novel-review"]);
        assert_eq!(s.selected_item().unwrap().room.slug, "novel-review");
        s.apply(Action::CycleSort);
        assert_eq!(s.sort(), SortMode::Name);
        assert_eq!(names(&s), ["2026-11-02-fix-bug", "2026-06-14-novel-review", "acme/2026-11-01-report"]);
        s.apply(Action::CycleSort);
        assert_eq!(s.sort(), SortMode::Recent);
    }

    #[test]
    fn fuzzy_search_covers_slug_project_and_japanese_description() {
        let mut s = State::new(sample(), "");
        type_str(&mut s, "acme");
        assert_eq!(names(&s), ["acme/2026-11-01-report", "+acme"]);
        s.apply(Action::Clear);
        type_str(&mut s, "講評");
        assert_eq!(names(&s), ["2026-06-14-novel-review", "+?"]);
        s.apply(Action::Clear);
        type_str(&mut s, "fxbg");
        assert_eq!(names(&s)[0], "2026-11-02-fix-bug");
    }

    #[test]
    fn ties_follow_current_sort_order() {
        let items = vec![
            item("2026-11-01-task-a", None, "", false, Some(ts(2026, 11, 5))),
            item("2026-11-02-task-b", None, "", false, Some(ts(2026, 11, 4))),
        ];
        let mut s = State::new(items, "task");
        assert_eq!(names(&s), ["2026-11-01-task-a", "2026-11-02-task-b", "+task"]);
        s.apply(Action::CycleSort);
        assert_eq!(names(&s), ["2026-11-02-task-b", "2026-11-01-task-a", "+task"]);
    }

    #[test]
    fn create_row_derives_slug_or_refuses() {
        let mut s = State::new(sample(), "new-thing");
        assert_eq!(names(&s), ["+new-thing"]);
        assert_eq!(s.apply(Action::Enter), Effect::Create("new-thing".into()));

        let mut s = State::new(sample(), "New Thing");
        s.apply(Action::Down);
        assert_eq!(s.apply(Action::Enter), Effect::Create("new-thing".into()));

        let mut s = State::new(sample(), "新しい作業");
        assert_eq!(s.apply(Action::Enter), Effect::None);
        assert!(s.status().unwrap().contains("slug にできません"));
        // 次の操作でメッセージは消える
        s.apply(Action::Up);
        assert!(s.status().is_none());
    }

    #[test]
    fn enter_on_room_and_selection_bounds() {
        let mut s = State::new(sample(), "");
        s.apply(Action::Up);
        assert_eq!(s.selected(), 0);
        for _ in 0..10 {
            s.apply(Action::Down);
        }
        assert_eq!(s.selected(), 2);
        assert_eq!(s.apply(Action::Enter), Effect::Enter(PathBuf::from("/w/2026-11-02-fix-bug")));
        assert_eq!(s.apply(Action::Quit), Effect::Quit);

        let mut empty = State::new(Vec::new(), "");
        assert_eq!(empty.row_count(), 0);
        assert_eq!(empty.apply(Action::Down), Effect::None);
        assert_eq!(empty.apply(Action::Enter), Effect::None);
        assert_eq!(empty.apply(Action::ToggleDone), Effect::None);
    }

    #[test]
    fn show_done_toggle_keeps_selection_by_path() {
        let mut s = State::new(sample(), "");
        s.apply(Action::Down);
        s.apply(Action::Down); // fix-bug
        s.apply(Action::CycleSort); // 作成日順: fix-bug, report, novel-review
        s.apply(Action::ToggleShowDone);
        assert!(s.show_done());
        assert_eq!(names(&s).len(), 4);
        assert_eq!(s.selected_item().unwrap().room.slug, "fix-bug");
        assert_eq!(s.hidden_done(), 0);
    }

    #[test]
    fn toggling_done_hides_room_and_clamps_selection() {
        let mut s = State::new(sample(), "");
        s.apply(Action::Down);
        s.apply(Action::Down); // 最後の行（fix-bug）
        let path = PathBuf::from("/w/2026-11-02-fix-bug");
        assert_eq!(s.apply(Action::ToggleDone), Effect::SetDone(path.clone(), true));
        s.mark_done(&path, true);
        assert_eq!(names(&s).len(), 2);
        assert_eq!(s.selected(), 1);
        assert_eq!(s.hidden_done(), 2);

        // 完了済みを表示しているときは、行は残り、完了を外せる
        s.apply(Action::ToggleShowDone);
        let idx = names(&s).iter().position(|n| n == "2026-11-02-fix-bug").unwrap();
        while s.selected() < idx {
            s.apply(Action::Down);
        }
        assert_eq!(s.apply(Action::ToggleDone), Effect::SetDone(path, false));
    }

    #[test]
    fn open_and_preview_toggle() {
        let mut s = State::new(sample(), "");
        assert_eq!(s.apply(Action::Open), Effect::Open(PathBuf::from("/w/acme/2026-11-01-report")));
        assert!(s.show_preview());
        s.apply(Action::TogglePreview);
        assert!(!s.show_preview());
    }

    #[test]
    fn typing_resets_selection_and_backspace_restores() {
        let mut s = State::new(sample(), "");
        s.apply(Action::Down);
        type_str(&mut s, "zzzz");
        assert_eq!(s.selected(), 0);
        assert_eq!(s.visible_count(), 0);
        for _ in 0..4 {
            s.apply(Action::Backspace);
        }
        assert_eq!(s.query(), "");
        assert_eq!(s.visible_count(), 3);
    }
}
