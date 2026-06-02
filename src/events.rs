use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use std::time::Duration;

pub enum InputEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize,
    Tick,
}

pub fn poll_event(tick_rate: Duration) -> Result<InputEvent> {
    if event::poll(tick_rate)? {
        match event::read()? {
            // Filter Release events: on Windows, crossterm emits both Press and Release for each
            // keystroke. Processing Release would cause each key to fire twice (e.g., 'c' opens
            // the config modal on Press, then immediately closes it on Release).
            Event::Key(key) if key.kind != crossterm::event::KeyEventKind::Release => {
                return Ok(InputEvent::Key(key));
            }
            Event::Mouse(m) => return Ok(InputEvent::Mouse(m)),
            Event::Resize(_, _) => return Ok(InputEvent::Resize),
            _ => {}
        }
    }
    Ok(InputEvent::Tick)
}

// Actions that map to app behavior
#[derive(Debug)]
pub enum Action {
    Quit,
    NavUp,
    NavDown,
    Select,
    Back,
    FocusSidebar,
    FocusMain,
    PlayPause,
    Next,
    Prev,
    VolumeUp,
    VolumeDown,
    ToggleMute,
    TogglePower,
    OpenConfig,
    AddToQueue,
    ClearQueue,
    DeleteQueueItem,
    Stop,
    ToggleShuffle,
    ToggleRepeat,
    ToggleFullArtMode,
    PageUp,
    PageDown,
    Home,
    End,
    ToggleFocus,
    ScopePrev,
    ScopeNext,
    NavToSidebar(usize),
    None,
}

/// Maps a key event to a context-free `Action`. Note: some keys are intercepted upstream in
/// `main.rs` before this runs and never reach here in that context — most notably `'s'`, which
/// opens the sync modal in the Players view but maps to `ToggleShuffle` everywhere else. Keep
/// that interception in mind when changing `'s'`/`ToggleShuffle`.
pub fn key_to_action(key: KeyEvent) -> Action {
    match (key.code, key.modifiers) {
        (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => Action::Quit,
        (KeyCode::Up, _) | (KeyCode::Char('k'), _) => Action::NavUp,
        (KeyCode::Down, _) | (KeyCode::Char('j'), _) => Action::NavDown,
        (KeyCode::Enter, _) | (KeyCode::Char('l'), _) => Action::Select,
        (KeyCode::Esc, _) | (KeyCode::Char('h'), _) | (KeyCode::Backspace, _) => Action::Back,
        (KeyCode::Left, _) => Action::FocusSidebar,
        (KeyCode::Right, _) => Action::FocusMain,
        (KeyCode::Char(' '), _) => Action::PlayPause,
        (KeyCode::Char('n'), _) => Action::Next,
        (KeyCode::Char('p'), _) => Action::Prev,
        (KeyCode::Char('+'), _) | (KeyCode::Char('='), _) => Action::VolumeUp,
        (KeyCode::Char('-'), _) => Action::VolumeDown,
        (KeyCode::Char('m'), _) => Action::ToggleMute,
        (KeyCode::Char('t'), _) => Action::TogglePower,
        (KeyCode::Char('c'), _) => Action::OpenConfig,
        (KeyCode::Char('a'), _) => Action::AddToQueue,
        (KeyCode::Char('x'), _) => Action::ClearQueue,
        (KeyCode::Char('d'), _) | (KeyCode::Delete, _) => Action::DeleteQueueItem,
        (KeyCode::Char('s'), _) => Action::ToggleShuffle,
        (KeyCode::Char('r'), _) => Action::ToggleRepeat,
        (KeyCode::Char('`'), _) => Action::ToggleFullArtMode,
        (KeyCode::PageUp, _) => Action::PageUp,
        (KeyCode::PageDown, _) => Action::PageDown,
        (KeyCode::Home, _) => Action::Home,
        (KeyCode::End, _) => Action::End,
        (KeyCode::Tab, _) | (KeyCode::BackTab, _) => Action::ToggleFocus,
        (KeyCode::Char('['), _) => Action::ScopePrev,
        (KeyCode::Char(']'), _) => Action::ScopeNext,
        (KeyCode::Char(c @ '1'..='8'), _) => Action::NavToSidebar(c as usize - '1' as usize),
        _ => Action::None,
    }
}
