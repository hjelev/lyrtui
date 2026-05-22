use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use std::time::Duration;
use anyhow::Result;

pub enum InputEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Tick,
}

pub fn poll_event(tick_rate: Duration) -> Result<InputEvent> {
    if event::poll(tick_rate)? {
        match event::read()? {
            Event::Key(key) => return Ok(InputEvent::Key(key)),
            Event::Mouse(m) => return Ok(InputEvent::Mouse(m)),
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
    None,
}

pub fn key_to_action(key: KeyEvent) -> Action {
    match (key.code, key.modifiers) {
        (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => Action::Quit,
        (KeyCode::Up, _) | (KeyCode::Char('k'), _) => Action::NavUp,
        (KeyCode::Down, _) | (KeyCode::Char('j'), _) => Action::NavDown,
        (KeyCode::Enter, _) | (KeyCode::Char('l'), _) => Action::Select,
        (KeyCode::Esc, _) | (KeyCode::Char('h'), _) => Action::Back,
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
        _ => Action::None,
    }
}
