//! Ratatui-based UI rendering for the P25 monitor TUI.
//!
//! Contains pure rendering functions that take [`MonitorState`] references
//! and produce ratatui widgets. No side effects or state mutation.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};

use super::state::{ActivityEvent, ChannelDisplayEntry, MonitorState};

/// Render the full monitor UI into the given frame.
///
/// Three-panel layout: header (system info), middle (channels + activity),
/// and bottom (raw log).
pub fn render(frame: &mut Frame, state: &MonitorState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Percentage(20),
        ])
        .split(frame.area());

    render_header(frame, chunks[0], state);
    render_middle(frame, chunks[1], state);
    render_log(frame, chunks[2], state);
}

/// Render the header bar showing system identity information.
fn render_header(frame: &mut Frame, area: Rect, state: &MonitorState) {
    let info = &state.system_info;
    let wacn = info.wacn.as_deref().unwrap_or("---");
    let system_id = info.system_id.as_deref().unwrap_or("---");
    let rfss = info.rfss_id.map_or("---".to_string(), |v| v.to_string());
    let site = info.site_id.map_or("---".to_string(), |v| v.to_string());
    let cc_freq = info
        .control_frequency
        .map_or("---".to_string(), |f| format!("{f:.4}"));
    let count = state.message_count;

    let text = format!(
        "WACN {wacn}  System {system_id}  RFSS {rfss}  Site {site}  CC {cc_freq} MHz  [{count} msgs]"
    );
    let paragraph = Paragraph::new(text)
        .style(Style::default().fg(Color::Cyan))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" P25 Monitor "),
        );
    frame.render_widget(paragraph, area);
}

/// Render the middle section: channels (left) and activity feed (right).
fn render_middle(frame: &mut Frame, area: Rect, state: &MonitorState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(area);

    render_channels(frame, chunks[0], state);
    render_activity(frame, chunks[1], state);
}

/// Render the channel list panel.
///
/// Shows the control channel at the top, active grants with a bullet
/// marker, and inactive channels dimmed.
fn render_channels(frame: &mut Frame, area: Rect, state: &MonitorState) {
    let entries = state.channel_list();
    let items: Vec<ListItem> = entries
        .iter()
        .map(|e| channel_list_item(e, state))
        .collect();

    let list = List::new(items).block(Block::default().borders(Borders::ALL).title(" Channels "));
    frame.render_widget(list, area);
}

/// Build a single channel list item with appropriate styling.
fn channel_list_item<'a>(entry: &ChannelDisplayEntry, state: &MonitorState) -> ListItem<'a> {
    let freq_str = entry
        .frequency
        .map_or("???".to_string(), |f| format!("{f:.4}"));

    let is_control = state.system_info.control_channel == Some(entry.channel);

    let line = if is_control {
        format!("CC {freq_str} MHz")
    } else if let Some(talkgroup) = entry.active_talkgroup {
        format!("  * {freq_str}  TG {talkgroup}")
    } else {
        format!("    {freq_str}")
    };

    let style = if is_control {
        Style::default().fg(Color::Cyan).bold()
    } else if entry.active_talkgroup.is_some() {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    ListItem::new(Line::from(Span::styled(line, style)))
}

/// Render the activity feed panel.
///
/// Shows timestamped events with color coding: green for grants,
/// red for emergencies, white for other events. Newest at bottom.
fn render_activity(frame: &mut Frame, area: Rect, state: &MonitorState) {
    let visible_height = area.height.saturating_sub(2) as usize;
    let events = &state.activity_events;
    let skip = events.len().saturating_sub(visible_height);

    let items: Vec<ListItem> = events.iter().skip(skip).map(activity_list_item).collect();

    let list = List::new(items).block(Block::default().borders(Borders::ALL).title(" Activity "));
    frame.render_widget(list, area);
}

/// Build a single activity list item with color coding.
fn activity_list_item(event: &ActivityEvent) -> ListItem<'_> {
    let elapsed = event.timestamp.elapsed().as_secs();
    let prefix = format_elapsed(elapsed);
    let text = format!("{prefix} {}", event.description);

    let style = if event.is_emergency {
        Style::default().fg(Color::Red).bold()
    } else if event.description.starts_with("Grant:") || event.description.starts_with("Update:") {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::White)
    };

    ListItem::new(Line::from(Span::styled(text, style)))
}

/// Format elapsed seconds as a relative timestamp (e.g., "2s ago", "5m ago").
fn format_elapsed(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds:>3}s")
    } else if seconds < 3600 {
        format!("{:>3}m", seconds / 60)
    } else {
        format!("{:>3}h", seconds / 3600)
    }
}

/// Render the raw log panel at the bottom.
///
/// Shows raw JSON lines scrolling, newest at bottom.
fn render_log(frame: &mut Frame, area: Rect, state: &MonitorState) {
    let visible_height = area.height.saturating_sub(2) as usize;
    let log = &state.raw_log;
    let skip = log.len().saturating_sub(visible_height);

    let text: String = log
        .iter()
        .skip(skip)
        .map(|line| line.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    let paragraph = Paragraph::new(text)
        .style(Style::default().fg(Color::DarkGray))
        .block(Block::default().borders(Borders::ALL).title(" Log "));
    frame.render_widget(paragraph, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn default_state() -> MonitorState {
        MonitorState::new(Duration::from_secs(3))
    }

    #[test]
    fn format_elapsed_seconds() {
        assert_eq!(format_elapsed(0), "  0s");
        assert_eq!(format_elapsed(5), "  5s");
        assert_eq!(format_elapsed(59), " 59s");
    }

    #[test]
    fn format_elapsed_minutes() {
        assert_eq!(format_elapsed(60), "  1m");
        assert_eq!(format_elapsed(120), "  2m");
        assert_eq!(format_elapsed(3599), " 59m");
    }

    #[test]
    fn format_elapsed_hours() {
        assert_eq!(format_elapsed(3600), "  1h");
        assert_eq!(format_elapsed(7200), "  2h");
    }

    #[test]
    fn channel_list_item_control_channel() {
        let mut state = default_state();
        state.system_info.control_channel = Some(100);

        let entry = ChannelDisplayEntry {
            channel: 100,
            frequency: Some(852.35),
            active_talkgroup: None,
        };

        let item = channel_list_item(&entry, &state);
        let text = format!("{:?}", item);
        assert!(text.contains("CC"));
        assert!(text.contains("852.35"));
    }

    #[test]
    fn channel_list_item_active_grant() {
        let state = default_state();
        let entry = ChannelDisplayEntry {
            channel: 200,
            frequency: Some(851.0625),
            active_talkgroup: Some(1234),
        };

        let item = channel_list_item(&entry, &state);
        let text = format!("{:?}", item);
        assert!(text.contains("TG 1234"));
    }

    #[test]
    fn channel_list_item_inactive() {
        let state = default_state();
        let entry = ChannelDisplayEntry {
            channel: 300,
            frequency: Some(851.125),
            active_talkgroup: None,
        };

        let item = channel_list_item(&entry, &state);
        let text = format!("{:?}", item);
        assert!(text.contains("851.125"));
    }

    #[test]
    fn channel_list_item_unknown_frequency() {
        let state = default_state();
        let entry = ChannelDisplayEntry {
            channel: 400,
            frequency: None,
            active_talkgroup: None,
        };

        let item = channel_list_item(&entry, &state);
        let text = format!("{:?}", item);
        assert!(text.contains("???"));
    }

    #[test]
    fn activity_item_emergency_styling() {
        let event = ActivityEvent {
            timestamp: std::time::Instant::now(),
            description: "EMERGENCY: src 12345 target 5000".to_string(),
            is_emergency: true,
        };

        let item = activity_list_item(&event);
        let text = format!("{:?}", item);
        assert!(text.contains("EMERGENCY"));
    }

    #[test]
    fn activity_item_grant_styling() {
        let event = ActivityEvent {
            timestamp: std::time::Instant::now(),
            description: "Grant: TG 100 on CH 200 (src 300)".to_string(),
            is_emergency: false,
        };

        let item = activity_list_item(&event);
        let text = format!("{:?}", item);
        assert!(text.contains("Grant"));
    }

    #[test]
    fn render_does_not_panic_on_empty_state() {
        let state = default_state();
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &state)).unwrap();
    }

    #[test]
    fn render_does_not_panic_with_data() {
        let mut state = default_state();
        state.process_message(
            r#"{"name":"NET_STS_BCST","wacn":"0x0C018","system_id":"0x704","channel":459,"frequency":null}"#,
        );
        state.process_message(
            r#"{"name":"RFSS_STS_BCST","system_id":"0x704","rfss_id":1,"site_id":12,"channel":459,"frequency":null}"#,
        );
        state.process_message(
            r#"{"name":"GRP_V_CH_GRANT","channel":100,"frequency":851.0625,"talkgroup":1234,"source":56789}"#,
        );

        let backend = ratatui::backend::TestBackend::new(120, 40);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &state)).unwrap();
    }
}
