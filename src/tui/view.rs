//! TUI ピッカーの描画。状態（`State`）を読むだけで、変更はしない。

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
    /// 選択中の部屋のプレビュー（README の先頭）
    pub preview: Option<&'a [String]>,
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
    let block = Block::bordered().title(Line::from(vec![" wk ".bold(), format!(" {} ", ctx.root).dark_gray()]));
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
            "部屋がありません。検索語を入力して Enter で作成します"
        } else {
            "一致する部屋がありません"
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
        Row::Create(Some(slug)) => Line::from(vec!["+ 新規作成: ".green(), Span::raw(slug.to_string()).green().bold()]),
        Row::Create(None) => Line::from("+ 新規作成（この検索語は slug にできません）").dark_gray(),
    }
}

fn render_preview(frame: &mut Frame, ctx: &ViewContext, area: Rect) {
    let block = Block::bordered().title(" プレビュー ");
    let lines: Vec<Line> = match ctx.preview {
        Some(lines) => lines
            .iter()
            .enumerate()
            .map(|(i, l)| if i == 0 { Line::from(l.clone()).bold() } else { Line::from(l.clone()) })
            .collect(),
        None => Vec::new(),
    };
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
        )
    }

    #[test]
    fn snapshot_list_with_preview() {
        let state = sample_state();
        let preview = vec!["# 不具合調査".to_string(), String::new(), "- created: 2026-11-02 19:33".to_string()];
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
                "│                                              ││                              │",
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
        let mut state = State::new(Vec::new(), "");
        let ctx = ViewContext { root: "~/work", icons: false, preview: None };
        let lines = draw(&state, &ctx, 80, 6);
        assert!(lines[2].contains("部屋がありません"), "{lines:#?}");

        for c in "日本".chars() {
            state.apply(Action::Input(c));
        }
        state.apply(Action::Enter);
        let lines = draw(&state, &ctx, 80, 6);
        assert!(lines[1].starts_with("│> 日本"), "{lines:#?}");
        assert!(lines[2].contains("+ 新規作成（この検索語は slug にできません）"), "{lines:#?}");
        assert!(lines[5].contains("slug にできません（英小文字"), "{lines:#?}");
    }

    #[test]
    fn cursor_follows_wide_characters() {
        let mut state = State::new(Vec::new(), "");
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
}
