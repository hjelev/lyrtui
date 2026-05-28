use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use std::time::Duration;
use anyhow::Result;

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
            Event::Key(key) if key.kind != crossterm::event::KeyEventKind::Release
                => return Ok(InputEvent::Key(key)),
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
        (KeyCode::Char('1'), _) => Action::NavToSidebar(0),
        (KeyCode::Char('2'), _) => Action::NavToSidebar(1),
        (KeyCode::Char('3'), _) => Action::NavToSidebar(2),
        (KeyCode::Char('4'), _) => Action::NavToSidebar(3),
        (KeyCode::Char('5'), _) => Action::NavToSidebar(4),
        (KeyCode::Char('6'), _) => Action::NavToSidebar(5),
        (KeyCode::Char('7'), _) => Action::NavToSidebar(6),
        (KeyCode::Char('8'), _) => Action::NavToSidebar(7),
        _ => Action::None,
    }
}
