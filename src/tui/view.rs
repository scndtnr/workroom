//! TUI ピッカーの描画。状態（`State`）を読むだけで、変更はしない。

use super::preview::Preview;
use super::state::{Row, State};
use crate::fsinfo;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph};

/// 描画に必要な、状態以外の情報
pub struct ViewContext<'a> {
    /// 枠のタイトルに出す作業ルート
    pub root: &'a str,
    pub icons: bool,
    /// 選択中の部屋のプレビュー（届いている部分から表示する）
    pub preview: Option<&'a Preview>,
}

const HELP: &str = "Enter 移動  ^D 完了  ^O 開く  ^S 並び順  Tab 完了済み  Alt+P 詳細  Esc 終了";

/// プレビューを出す最小の幅（これより狭い端末では一覧だけにする）
const MIN_WIDTH_FOR_PREVIEW: u16 = 70;

pub fn render(frame: &mut Frame, state: &State, ctx: &ViewContext, list_state: &mut ListState) {
    let [main, footer] = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(frame.area());
    let (list_area, preview_area) = if state.show_preview() && main.width >= MIN_WIDTH_FOR_PREVIEW {
        let [l, p] = Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(main);
        (l, Some(p))
    } else {
        (main, None)
    };

    render_list(frame, state, ctx, list_area, list_state);
    if let Some(area) = preview_area {
        render_preview(frame, ctx, area);
    }
    let footer_line = match state.status() {
        Some(msg) => Line::from(msg.to_string()).yellow(),
        None => Line::from(HELP).dark_gray(),
    };
    frame.render_widget(Paragraph::new(footer_line), footer);
}

fn render_list(frame: &mut Frame, state: &State, ctx: &ViewContext, area: Rect, list_state: &mut ListState) {
    let mut title = vec![" wk ".bold(), format!(" {} ", ctx.root).dark_gray()];
    if let Some(project) = state.project() {
        title.push(format!(" 案件: {project} ").blue());
    }
    let block = Block::bordered().title(Line::from(title));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [input_area, list_area] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);

    // 検索欄と件数・並び順
    let mut info = format!("{}/{}  {}", state.visible_count(), state.total(), state.sort().label());
    if state.hidden_done() > 0 {
        info.push_str(&format!("  完了 {}件非表示", state.hidden_done()));
    }
    let info_width = Span::raw(&info).width() as u16;
    let [query_area, info_area] =
        Layout::horizontal([Constraint::Min(8), Constraint::Length(info_width)]).areas(input_area);
    let prompt = Line::from(vec!["> ".cyan().bold(), Span::raw(state.query())]);
    let cursor_x = query_area.x + (prompt.width() as u16).min(query_area.width.saturating_sub(1));
    frame.render_widget(Paragraph::new(prompt), query_area);
    frame.render_widget(Paragraph::new(Line::from(info).dark_gray().right_aligned()), info_area);
    // 入力位置にカーソルを置く（IME の変換窓の位置にもなる）
    frame.set_cursor_position((cursor_x, query_area.y));

    let items: Vec<ListItem> = state.rows().map(|row| ListItem::new(row_line(row, ctx.icons))).collect();
    if items.is_empty() {
        let msg = if state.total() == 0 {
            match state.project() {
                Some(project) => format!("案件 {project} に部屋がありません。検索語を入力して Enter で作成します"),
                None => "部屋がありません。検索語を入力して Enter で作成します".to_string(),
            }
        } else {
            "一致する部屋がありません".to_string()
        };
        frame.render_widget(Paragraph::new(Line::from(msg).dark_gray()), list_area);
        return;
    }
    list_state.select(Some(state.selected()));
    let list = List::new(items)
        .highlight_symbol("> ")
        .highlight_style(Style::new().bg(Color::DarkGray).add_modifier(Modifier::BOLD));
    frame.render_stateful_widget(list, list_area, list_state);
}

fn row_line(row: Row, icons: bool) -> Line<'static> {
    match row {
        Row::Room(item) => {
            let r = &item.room;
            let mut spans = Vec::new();
            if icons {
                spans.push(Span::raw(format!("{} ", fsinfo::icon_for("", true))).blue());
            }
            spans.push(Span::raw(r.date.format("%y-%m-%d ").to_string()).dark_gray());
            if let Some(p) = &r.project {
                spans.push(Span::raw(format!("{p}/")).blue());
            }
            let slug = Span::raw(r.slug.clone());
            spans.push(if r.done { slug.dark_gray() } else { slug.cyan() });
            if !r.description.is_empty() {
                spans.push(Span::raw(format!("  {}", r.description)));
            }
            if r.done {
                spans.push(" ✓".green());
            }
            Line::from(spans)
        }
        Row::Create(Ok(target)) => Line::from(vec!["+ 新規作成: ".green(), Span::raw(target.label()).green().bold()]),
        Row::Create(Err(reason)) => Line::from(format!("+ 新規作成できません: {reason}")).dark_gray(),
    }
}

fn render_preview(frame: &mut Frame, ctx: &ViewContext, area: Rect) {
    let block = Block::bordered().title(" プレビュー ");
    let mut lines = Vec::new();
    if let Some(preview) = ctx.preview {
        match &preview.readme {
            Some(readme) => lines.extend(
                readme
                    .iter()
                    .enumerate()
                    .map(|(i, l)| if i == 0 { Line::from(l.as_str()).bold() } else { Line::from(l.as_str()) }),
            ),
            None => lines.push(Line::from("README を読み込み中…").dark_gray()),
        }
        lines.push(Line::default());
        match &preview.tree {
            Some(tree) => lines.extend(tree.iter().map(|entry| {
                let icon = if ctx.icons && !entry.more {
                    format!("{} ", fsinfo::icon_for(&entry.name, entry.is_dir))
                } else {
                    String::new()
                };
                let name = Span::raw(format!("{icon}{}", entry.name));
                let name = if entry.more {
                    name.dark_gray()
                } else if entry.is_dir {
                    name.blue()
                } else {
                    name
                };
                let line = Line::from(vec![Span::raw(&entry.prefix), name]);
                if entry.more { line.dark_gray() } else { line }
            })),
            None => lines.push(Line::from("ファイル構成を計算中…").dark_gray()),
        }
        lines.push(Line::default());
        lines.push(match preview.usage {
            Some(usage) => Line::from(format!("{} files · {}", usage.files, fsinfo::human_bytes(usage.bytes))),
            None => Line::from("サイズを計算中…").dark_gray(),
        });
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::state::Action;
    use crate::tui::state::tests::item;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    /// バッファを文字列の行にする（全角文字の後ろの空きセルは飛ばし、行末の空白は削る）
    fn buffer_lines(buf: &Buffer) -> Vec<String> {
        let area = buf.area;
        (0..area.height)
            .map(|y| {
                let mut line = String::new();
                let mut skip = 0;
                for x in 0..area.width {
                    if skip > 0 {
                        skip -= 1;
                        continue;
                    }
                    let sym = buf[(x, y)].symbol();
                    line.push_str(sym);
                    skip = Span::raw(sym).width().saturating_sub(1);
                }
                line.trim_end().to_string()
            })
            .collect()
    }

    fn draw(state: &State, ctx: &ViewContext, w: u16, h: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        let mut list_state = ListState::default();
        terminal.draw(|f| render(f, state, ctx, &mut list_state)).unwrap();
        buffer_lines(terminal.backend().buffer())
    }

    fn sample_state() -> State {
        State::new(
            vec![
                item("2026-11-02-fix-bug", None, "不具合調査", false, Some(100)),
                item("2026-11-01-report", Some("acme"), "月次レポート", false, Some(50)),
                item("2026-10-01-old", None, "", true, None),
            ],
            "",
            None,
            false,
        )
    }

    #[test]
    fn snapshot_list_with_preview() {
        let state = sample_state();
        let preview = Preview {
            readme: Some(vec!["# 不具合調査".to_string(), String::new(), "- created: 2026-11-02 19:33".to_string()]),
            ..Preview::default()
        };
        let ctx = ViewContext { root: "~/work", icons: false, preview: Some(&preview) };
        let lines = draw(&state, &ctx, 80, 8);
        assert_eq!(
            lines,
            vec![
                "┌ wk  ~/work ──────────────────────────────────┐┌ プレビュー ──────────────────┐",
                "│>            2/3  最近使った順  完了 1件非表示││# 不具合調査                  │",
                "│> 26-11-02 fix-bug  不具合調査                ││                              │",
                "│  26-11-01 acme/report  月次レポート          ││- created: 2026-11-02 19:33   │",
                "│                                              ││                              │",
                "│                                              ││ファイル構成を計算中…         │",
                "└──────────────────────────────────────────────┘└──────────────────────────────┘",
                "Enter 移動  ^D 完了  ^O 開く  ^S 並び順  Tab 完了済み  Alt+P 詳細  Esc 終了",
            ]
        );
    }

    #[test]
    fn snapshot_without_preview_shows_done_and_create_row() {
        let mut state = sample_state();
        state.apply(Action::ToggleShowDone);
        state.apply(Action::TogglePreview);
        for c in "o".chars() {
            state.apply(Action::Input(c));
        }
        let ctx = ViewContext { root: "~/work", icons: false, preview: None };
        let lines = draw(&state, &ctx, 60, 7);
        assert!(lines[0].starts_with("┌ wk  ~/work ─") && lines[0].ends_with('┐'), "{lines:#?}");
        assert!(!lines.iter().any(|l| l.contains("プレビュー")), "{lines:#?}");
        assert!(lines[1].starts_with("│> o"), "{lines:#?}");
        assert!(lines.iter().any(|l| l.contains("26-10-01 old ✓")), "{lines:#?}");
        assert!(lines.iter().any(|l| l.contains("+ 新規作成: o")), "{lines:#?}");
    }

    #[test]
    fn snapshot_empty_root_and_status() {
        let mut state = State::new(Vec::new(), "", None, false);
        let ctx = ViewContext { root: "~/work", icons: false, preview: None };
        let lines = draw(&state, &ctx, 80, 6);
        assert!(lines[2].contains("部屋がありません"), "{lines:#?}");

        for c in "a:b".chars() {
            state.apply(Action::Input(c));
        }
        state.apply(Action::TogglePreview);
        state.apply(Action::Enter);
        let lines = draw(&state, &ctx, 100, 6);
        assert!(lines[1].starts_with("│> a:b"), "{lines:#?}");
        assert!(lines[2].contains("+ 新規作成できません: 部屋の名前に `:` は使えません"), "{lines:#?}");
        assert!(lines[5].contains("部屋の名前に `:` は使えません"), "{lines:#?}");
    }

    #[test]
    fn snapshot_project_title_empty_message_and_create_rows() {
        let ctx = ViewContext { root: "~/work", icons: false, preview: None };
        let mut state = State::new(Vec::new(), "", Some("ACME"), true);
        state.apply(Action::TogglePreview);
        let lines = draw(&state, &ctx, 100, 6);
        assert!(lines[0].starts_with("┌ wk  ~/work  案件: ACME ─"), "{lines:#?}");
        assert!(lines[2].contains("案件 ACME に部屋がありません。検索語を入力して Enter で作成します"), "{lines:#?}");
        for c in "調査".chars() {
            state.apply(Action::Input(c));
        }
        let lines = draw(&state, &ctx, 100, 6);
        assert!(lines[2].contains("+ 新規作成: ACME/調査"), "{lines:#?}");
        state.apply(Action::Clear);
        for c in "other/x".chars() {
            state.apply(Action::Input(c));
        }
        let lines = draw(&state, &ctx, 100, 6);
        assert!(lines[2].contains("+ 新規作成: OTHER/x"), "{lines:#?}");

        let state = State::new(Vec::new(), "調査", None, false);
        let lines = draw(&state, &ctx, 60, 6);
        assert!(lines[2].contains("+ 新規作成: 調査"), "{lines:#?}");
        assert!(!lines[0].contains("案件:"), "{lines:#?}");
    }

    #[test]
    fn cursor_follows_wide_characters() {
        let mut state = State::new(Vec::new(), "", None, false);
        for c in "日本a".chars() {
            state.apply(Action::Input(c));
        }
        let ctx = ViewContext { root: "~/work", icons: false, preview: None };
        let mut terminal = Terminal::new(TestBackend::new(60, 6)).unwrap();
        let mut list_state = ListState::default();
        terminal.draw(|f| render(f, &state, &ctx, &mut list_state)).unwrap();
        // 枠(1) + "> "(2) + 日本(4) + a(1)
        terminal.backend_mut().assert_cursor_position((8, 1));
    }

    #[test]
    fn icons_are_drawn_when_enabled() {
        let state = sample_state();
        let ctx = ViewContext { root: "~/work", icons: true, preview: None };
        let lines = draw(&state, &ctx, 80, 6);
        assert!(lines[2].contains(&format!("{} 26-11-02 fix-bug", fsinfo::icon_for("", true))), "{lines:#?}");
    }

    fn full_preview() -> Preview {
        Preview {
            readme: Some(vec!["# 不具合調査".into(), "概要".into()]),
            tree: Some(vec![
                fsinfo::TreeLine { prefix: "├── ".into(), name: "docs".into(), is_dir: true, more: false },
                fsinfo::TreeLine { prefix: "│   └── ".into(), name: "a.md".into(), is_dir: false, more: false },
                fsinfo::TreeLine {
                    prefix: "└── ".into(), name: "… 他 2 件".into(), is_dir: false, more: true
                },
            ]),
            usage: Some(fsinfo::DiskUsage { files: 3, bytes: 12698 }),
        }
    }

    fn draw_preview(preview: &Preview, icons: bool) -> Buffer {
        let ctx = ViewContext { root: "~/work", icons, preview: Some(preview) };
        let mut terminal = Terminal::new(TestBackend::new(32, 11)).unwrap();
        terminal.draw(|f| render_preview(f, &ctx, f.area())).unwrap();
        terminal.backend().buffer().clone()
    }

    #[test]
    fn snapshot_partial_preview() {
        let preview = Preview { readme: Some(vec!["# 不具合調査".into()]), ..Preview::default() };
        let buffer = draw_preview(&preview, false);
        assert_eq!(
            buffer_lines(&buffer),
            vec![
                "┌ プレビュー ──────────────────┐",
                "│# 不具合調査                  │",
                "│                              │",
                "│ファイル構成を計算中…         │",
                "│                              │",
                "│サイズを計算中…               │",
                "│                              │",
                "│                              │",
                "│                              │",
                "│                              │",
                "└──────────────────────────────┘",
            ]
        );
        assert!(buffer[(1, 1)].modifier.contains(Modifier::BOLD));
        assert_eq!(buffer[(1, 3)].fg, Color::DarkGray);
        assert_eq!(buffer[(1, 5)].fg, Color::DarkGray);
        let pending = draw_preview(&Preview::default(), false);
        assert!(buffer_lines(&pending)[1].contains("README を読み込み中…"));
        assert_eq!(pending[(1, 1)].fg, Color::DarkGray);
    }

    #[test]
    fn snapshot_complete_preview() {
        let buffer = draw_preview(&full_preview(), false);
        assert_eq!(
            buffer_lines(&buffer),
            vec![
                "┌ プレビュー ──────────────────┐",
                "│# 不具合調査                  │",
                "│概要                          │",
                "│                              │",
                "│├── docs                      │",
                "││   └── a.md                  │",
                "│└── … 他 2 件                 │",
                "│                              │",
                "│3 files · 12.4 KB             │",
                "│                              │",
                "└──────────────────────────────┘",
            ]
        );
        assert!(buffer[(1, 1)].modifier.contains(Modifier::BOLD));
        assert!(!buffer[(1, 2)].modifier.contains(Modifier::BOLD));
        assert_eq!(buffer[(5, 4)].fg, Color::Blue);
        assert_eq!(buffer[(1, 6)].fg, Color::DarkGray);
    }

    #[test]
    fn preview_tree_icons_follow_configuration() {
        let preview = full_preview();
        let buffer = draw_preview(&preview, true);
        let lines = buffer_lines(&buffer);
        assert!(lines[4].contains(&format!("├── {} docs", fsinfo::icon_for("docs", true))), "{lines:#?}");
        assert!(lines[5].contains(&format!("│   └── {} a.md", fsinfo::icon_for("a.md", false))), "{lines:#?}");
        assert_eq!(lines[6], "│└── … 他 2 件                 │");
        assert_eq!(buffer[(5, 4)].fg, Color::Blue);
        let plain = buffer_lines(&draw_preview(&preview, false)).join("\n");
        assert!(!plain.contains(fsinfo::icon_for("docs", true)));
        assert!(!plain.contains(fsinfo::icon_for("a.md", false)));
    }
}
