//! TUI ピッカー（`wk` / `wk <検索語>`）。
//!
//! - `state`：検索・絞り込み・並び替え・選択（描画から分けて単体テストする）
//! - `view`：ratatui での描画（`TestBackend` でテストする）
//! - このファイル：端末の準備と後片付け、キー入力の対応付け、ファイルの変更
//!
//! 画面を表示している間は標準エラーに何も書かない（画面が崩れるため）。
//! 部屋の作成・移動のメッセージは、端末を元に戻してから呼び出し元が出す。

pub mod state;
mod view;

use crate::config::Settings;
use crate::history::History;
use crate::{fsinfo, room, shell};
use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::ListState;
use state::{Action, Effect, Item, State};
use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::PathBuf;

/// ピッカーの結果
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Quit,
    Enter(PathBuf),
    Create(String),
}

/// TUI を表示できる端末か（標準入力と標準出力の両方が端末）
pub fn has_terminal() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// キー入力を操作に対応付ける。Windows ではキーを離したときのイベントも届くので、押したときだけを扱う
pub fn action_for(key: KeyEvent) -> Option<Action> {
    if key.kind != KeyEventKind::Press {
        return None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let action = match key.code {
        KeyCode::Esc => Action::Quit,
        KeyCode::Enter => Action::Enter,
        KeyCode::Up => Action::Up,
        KeyCode::Down => Action::Down,
        KeyCode::Tab => Action::ToggleShowDone,
        KeyCode::Backspace => Action::Backspace,
        // AltGr は Ctrl+Alt として届くので、文字として扱う
        KeyCode::Char(c) if ctrl && alt => Action::Input(c),
        KeyCode::Char(c) if ctrl => match c.to_ascii_lowercase() {
            'c' => Action::Quit,
            'u' => Action::Clear,
            'p' | 'k' => Action::Up,
            'n' | 'j' => Action::Down,
            'd' => Action::ToggleDone,
            'o' => Action::Open,
            's' => Action::CycleSort,
            'h' => Action::Backspace,
            _ => return None,
        },
        KeyCode::Char(c) if alt => match c.to_ascii_lowercase() {
            'p' => Action::TogglePreview,
            _ => return None,
        },
        KeyCode::Char(c) => Action::Input(c),
        _ => return None,
    };
    Some(action)
}

fn load_items(settings: &Settings) -> Result<Vec<Item>> {
    let history = History::load(&settings.state_dir);
    Ok(room::scan(&settings.root, None)?
        .into_iter()
        .map(|r| {
            let last_used = history.last_used(&r.path);
            Item { room: r, last_used }
        })
        .collect())
}

/// ピッカーを表示する。呼び出す前に `has_terminal()` を確かめること
pub fn run(settings: &Settings, query: &str) -> Result<Outcome> {
    let mut state = State::new(load_items(settings)?, query);
    // ratatui::try_init はパニック時にも端末を元に戻すフックを入れる
    let mut terminal = ratatui::try_init()?;
    let result = event_loop(&mut terminal, &mut state, settings);
    ratatui::try_restore()?;
    result
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, state: &mut State, settings: &Settings) -> Result<Outcome> {
    let root = settings.root.display().to_string();
    let mut list_state = ListState::default();
    let mut previews: HashMap<PathBuf, Vec<String>> = HashMap::new();
    loop {
        let preview = state.selected_item().map(|item| {
            previews.entry(item.room.path.clone()).or_insert_with(|| fsinfo::readme_head(&item.room.path, 30)).clone()
        });
        let ctx = view::ViewContext { root: &root, icons: settings.config.icons, preview: preview.as_deref() };
        terminal.draw(|f| view::render(f, state, &ctx, &mut list_state))?;

        let Event::Key(key) = event::read()? else { continue };
        let Some(action) = action_for(key) else { continue };
        match state.apply(action) {
            Effect::None => {}
            Effect::Quit => return Ok(Outcome::Quit),
            Effect::Enter(path) => return Ok(Outcome::Enter(path)),
            Effect::Create(slug) => return Ok(Outcome::Create(slug)),
            Effect::SetDone(path, done) => match room::set_done_at(&path, done) {
                Ok(()) => state.mark_done(&path, done),
                Err(e) => state.set_status(format!("完了の切り替えに失敗しました: {e:#}")),
            },
            Effect::Open(path) => {
                if let Err(e) = shell::open_in_filer(&path) {
                    state.set_status(format!("{e:#}"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyEventState;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn only_press_events_are_handled() {
        let mut release = key(KeyCode::Char('a'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        assert_eq!(action_for(release), None);
        let mut repeat = key(KeyCode::Down, KeyModifiers::NONE);
        repeat.kind = KeyEventKind::Repeat;
        assert_eq!(action_for(repeat), None);
        let press = KeyEvent {
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
            ..key(KeyCode::Down, KeyModifiers::NONE)
        };
        assert_eq!(action_for(press), Some(Action::Down));
    }

    #[test]
    fn key_table_matches_design() {
        let none = KeyModifiers::NONE;
        let ctrl = KeyModifiers::CONTROL;
        let cases = [
            (key(KeyCode::Char('a'), none), Some(Action::Input('a'))),
            (key(KeyCode::Char('A'), KeyModifiers::SHIFT), Some(Action::Input('A'))),
            (key(KeyCode::Char('日'), none), Some(Action::Input('日'))),
            (key(KeyCode::Backspace, none), Some(Action::Backspace)),
            (key(KeyCode::Char('u'), ctrl), Some(Action::Clear)),
            (key(KeyCode::Up, none), Some(Action::Up)),
            (key(KeyCode::Down, none), Some(Action::Down)),
            (key(KeyCode::Char('p'), ctrl), Some(Action::Up)),
            (key(KeyCode::Char('n'), ctrl), Some(Action::Down)),
            (key(KeyCode::Char('k'), ctrl), Some(Action::Up)),
            (key(KeyCode::Char('j'), ctrl), Some(Action::Down)),
            (key(KeyCode::Enter, none), Some(Action::Enter)),
            (key(KeyCode::Char('d'), ctrl), Some(Action::ToggleDone)),
            (key(KeyCode::Char('o'), ctrl), Some(Action::Open)),
            (key(KeyCode::Char('s'), ctrl), Some(Action::CycleSort)),
            (key(KeyCode::Tab, none), Some(Action::ToggleShowDone)),
            (key(KeyCode::Char('p'), KeyModifiers::ALT), Some(Action::TogglePreview)),
            (key(KeyCode::Esc, none), Some(Action::Quit)),
            (key(KeyCode::Char('c'), ctrl), Some(Action::Quit)),
            (key(KeyCode::Char('@'), ctrl | KeyModifiers::ALT), Some(Action::Input('@'))),
            (key(KeyCode::Char('z'), ctrl), None),
            (key(KeyCode::F(1), none), None),
        ];
        for (k, expected) in cases {
            assert_eq!(action_for(k), expected, "{k:?}");
        }
    }
}
