//! Event loop for the P25 monitor TUI.
//!
//! Manages terminal setup/teardown, stdin reading on a background thread,
//! crossterm key events, and periodic tick-based rendering.

use std::io::{self, BufRead, BufReader};
use std::sync::mpsc;
use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyModifiers};
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;

use super::state::MonitorState;
use super::ui;
use crate::monitor::error::MonitorError;

/// Events that drive the monitor loop.
pub enum MonitorEvent {
    /// A line was read from stdin.
    JsonLine(String),
    /// Stdin reached EOF.
    InputClosed,
}

/// Tick interval for grant expiry checks and screen refresh.
const TICK_INTERVAL: Duration = Duration::from_millis(250);

/// Run the monitor event loop, consuming stdin and rendering to the terminal.
///
/// Sets up the terminal in raw/alternate-screen mode, spawns a stdin reader
/// thread, and loops until the user presses 'q' or Ctrl-C. Terminal is
/// always restored on exit, including on panic.
pub fn run_event_loop(state: &mut MonitorState) -> Result<(), MonitorError> {
    install_panic_hook();
    let mut terminal = setup_terminal()?;
    let receiver = spawn_stdin_reader();

    let result = main_loop(&mut terminal, state, &receiver);

    restore_terminal(&mut terminal)?;
    result
}

/// Install a panic hook that restores the terminal before printing the panic.
fn install_panic_hook() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(io::stdout(), LeaveAlternateScreen);
        original_hook(info);
    }));
}

/// Set up the terminal for TUI rendering.
fn setup_terminal() -> Result<ratatui::Terminal<CrosstermBackend<io::Stdout>>, MonitorError> {
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = ratatui::Terminal::new(backend)?;
    Ok(terminal)
}

/// Restore the terminal to its original state.
fn restore_terminal(
    terminal: &mut ratatui::Terminal<CrosstermBackend<io::Stdout>>,
) -> Result<(), MonitorError> {
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

/// Spawn a thread that reads stdin line-by-line and sends events.
///
/// The thread terminates when stdin reaches EOF (sending `InputClosed`)
/// or when the receiver is dropped.
fn spawn_stdin_reader() -> mpsc::Receiver<MonitorEvent> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let reader = BufReader::new(io::stdin());
        for line in reader.lines() {
            match line {
                Ok(text) => {
                    if sender.send(MonitorEvent::JsonLine(text)).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = sender.send(MonitorEvent::InputClosed);
    });
    receiver
}

/// Core event loop: process stdin, handle keys, expire grants, render.
fn main_loop(
    terminal: &mut ratatui::Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut MonitorState,
    receiver: &mpsc::Receiver<MonitorEvent>,
) -> Result<(), MonitorError> {
    loop {
        if drain_stdin_events(state, receiver) || should_quit()? {
            break;
        }

        state.expire_grants();
        terminal.draw(|frame| ui::render(frame, state))?;
    }
    Ok(())
}

/// Process all pending stdin events without blocking.
///
/// Returns `true` if stdin reached EOF.
fn drain_stdin_events(state: &mut MonitorState, receiver: &mpsc::Receiver<MonitorEvent>) -> bool {
    let mut eof = false;
    while let Ok(event) = receiver.try_recv() {
        match event {
            MonitorEvent::JsonLine(line) => state.process_message(&line),
            MonitorEvent::InputClosed => eof = true,
        }
    }
    eof
}

/// Poll for crossterm key events and return true if the user wants to quit.
fn should_quit() -> Result<bool, MonitorError> {
    if crossterm::event::poll(TICK_INTERVAL)?
        && let Event::Key(key) = crossterm::event::read()?
    {
        let ctrl_c =
            key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
        if key.code == KeyCode::Char('q') || ctrl_c {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monitor_event_json_line_variant() {
        let event = MonitorEvent::JsonLine("test".to_string());
        assert!(matches!(event, MonitorEvent::JsonLine(_)));
    }

    #[test]
    fn monitor_event_input_closed_variant() {
        let event = MonitorEvent::InputClosed;
        assert!(matches!(event, MonitorEvent::InputClosed));
    }

    #[test]
    fn drain_stdin_events_processes_messages() {
        let (sender, receiver) = mpsc::channel();
        let mut state = MonitorState::new(Duration::from_secs(3));

        sender
            .send(MonitorEvent::JsonLine(
                r#"{"name":"NET_STS_BCST","wacn":"0x0C018","system_id":"0x704","channel":459,"frequency":null}"#.to_string(),
            ))
            .unwrap();
        sender
            .send(MonitorEvent::JsonLine(
                r#"{"name":"GRP_V_CH_GRANT","channel":100,"frequency":null,"talkgroup":200,"source":300}"#.to_string(),
            ))
            .unwrap();
        drop(sender);

        let eof = drain_stdin_events(&mut state, &receiver);

        assert!(!eof);
        assert_eq!(state.message_count, 2);
        assert_eq!(state.system_info.wacn.as_deref(), Some("0x0C018"));
        assert!(state.active_grants.contains_key(&100));
    }

    #[test]
    fn drain_stdin_events_handles_eof() {
        let (sender, receiver) = mpsc::channel();
        let mut state = MonitorState::new(Duration::from_secs(3));

        sender.send(MonitorEvent::InputClosed).unwrap();
        drop(sender);

        let eof = drain_stdin_events(&mut state, &receiver);
        assert!(eof);
        assert_eq!(state.message_count, 0);
    }

    #[test]
    fn drain_stdin_events_handles_empty_channel() {
        let (_sender, receiver) = mpsc::channel();
        let mut state = MonitorState::new(Duration::from_secs(3));

        let eof = drain_stdin_events(&mut state, &receiver);
        assert!(!eof);
        assert_eq!(state.message_count, 0);
    }

    #[test]
    fn spawn_stdin_reader_returns_receiver() {
        // Just verify it doesn't panic. The thread will block on stdin
        // but will be cleaned up when the receiver is dropped.
        let _receiver = spawn_stdin_reader();
    }
}
