use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use std::io::Write;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub enum InputEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize,
    Tick,
    /// The input stream is producing a continuous flood of events we drop (a half-closed/EOF
    /// stdin, a detached session, or garbled escape sequences). Signals the main loop to exit
    /// cleanly instead of spinning a CPU core at 100%.
    Disconnected,
}

/// Cap on same-direction scroll events drained per wheel tick, so a stuck terminal/mouse can't
/// spin the drain loop indefinitely.
const MAX_SCROLL_DRAIN: u32 = 64;

/// If a single `poll_event` call reads this many dropped (non-actionable) events without the
/// tick budget elapsing, the input stream is treated as broken. A real user cannot generate
/// this many dropped events inside one ~250 ms tick; a half-closed/EOF stdin produces them
/// instantly and unboundedly.
const MAX_DROPPED_EVENTS: u32 = 1024;

/// Whether `LYRTUI_DEBUG_EVENTS` is set. Evaluated once; the hot path is a single load when off.
fn debug_events_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("LYRTUI_DEBUG_EVENTS").is_some())
}

/// Append a diagnostic line to `<tmp>/lyrtui-events.log` when `LYRTUI_DEBUG_EVENTS` is set.
/// No-op (single bool check) otherwise. Used to pinpoint a flooding event kind if a hang recurs.
fn log_event_debug(msg: &str) {
    if !debug_events_enabled() {
        return;
    }
    let path = std::env::temp_dir().join("lyrtui-events.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{:?} {msg}", std::time::SystemTime::now());
    }
}

pub fn poll_event(tick_rate: Duration) -> Result<InputEvent> {
    // Treat `tick_rate` as a hard budget. Events we drop (key Release, focus change, bracketed
    // paste, partial/garbled escape sequences, EOF spam) return from `event::read` instantly and
    // do NOT consume the poll timeout. Returning `Tick` on each one would let a continuous stream
    // bypass the budget and spin the outer loop at 100% CPU, so instead we keep waiting within the
    // deadline and bail out as `Disconnected` if the stream floods without bound.
    let deadline = Instant::now() + tick_rate;
    let mut dropped: u32 = 0;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() || !event::poll(remaining)? {
            return Ok(InputEvent::Tick);
        }
        match event::read()? {
            // Filter Release events: on Windows, crossterm emits both Press and Release for each
            // keystroke. Processing Release would cause each key to fire twice (e.g., 'c' opens
            // the config modal on Press, then immediately closes it on Release).
            Event::Key(key) if key.kind != crossterm::event::KeyEventKind::Release => {
                return Ok(InputEvent::Key(key));
            }
            Event::Mouse(m) => {
                if matches!(m.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown) {
                    // Drain extra same-direction scroll events the OS sends per wheel tick.
                    // Bounded so a stuck terminal/mouse can't spin this at 100% CPU.
                    for _ in 0..MAX_SCROLL_DRAIN {
                        if !event::poll(Duration::ZERO)? {
                            break;
                        }
                        match event::read()? {
                            Event::Mouse(next) if next.kind == m.kind => {}
                            _ => break,
                        }
                    }
                }
                return Ok(InputEvent::Mouse(m));
            }
            Event::Resize(_, _) => return Ok(InputEvent::Resize),
            other => {
                dropped += 1;
                log_event_debug(&format!("dropped #{dropped}: {other:?}"));
                if dropped >= MAX_DROPPED_EVENTS {
                    log_event_debug("input stream flooded with dropped events; disconnecting");
                    return Ok(InputEvent::Disconnected);
                }
            }
        }
    }
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
