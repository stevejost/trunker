//! Monitor state tracking for the P25 TUI.
//!
//! Maintains system information, active grants, identifier tables,
//! and activity history from parsed decoder JSON output.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::monitor::parse::{self, ParsedMessage};

/// Maximum number of activity events to retain.
const MAX_ACTIVITY_EVENTS: usize = 1000;

/// Maximum number of raw log lines to retain.
const MAX_RAW_LOG: usize = 500;

/// Top-level monitor state, updated from decoder JSON output.
pub struct MonitorState {
    /// System identity information from status broadcasts.
    pub system_info: SystemInfo,
    /// Currently active voice grants, keyed by channel number.
    pub active_grants: HashMap<u16, ActiveGrant>,
    /// Frequency band table (4-bit identifier, 16 slots).
    pub identifiers: [Option<IdentifierEntry>; 16],
    /// All channel numbers ever seen, sorted.
    pub known_channels: Vec<u16>,
    /// Recent activity events (capped at 1000).
    pub activity_events: VecDeque<ActivityEvent>,
    /// Recent raw JSON lines (capped at 500).
    pub raw_log: VecDeque<String>,
    /// Total messages processed.
    pub message_count: u64,
    /// Duration after which grants are considered expired.
    pub grant_timeout: Duration,
}

/// System identity information from network/RFSS status broadcasts.
pub struct SystemInfo {
    /// Wide Area Communications Network ID.
    pub wacn: Option<String>,
    /// System ID.
    pub system_id: Option<String>,
    /// RFSS identifier.
    pub rfss_id: Option<u8>,
    /// Site identifier.
    pub site_id: Option<u8>,
    /// Control channel number.
    pub control_channel: Option<u16>,
    /// Control channel frequency in MHz.
    pub control_frequency: Option<f64>,
}

/// An active voice channel grant.
pub struct ActiveGrant {
    /// Channel number.
    pub channel: u16,
    /// Frequency in MHz if resolved.
    pub frequency: Option<f64>,
    /// Talkgroup using this channel.
    pub talkgroup: u16,
    /// Source unit ID, if known.
    pub source: Option<u32>,
    /// When this grant was last refreshed.
    pub last_seen: Instant,
}

/// Frequency band parameters from an identifier update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdentifierEntry {
    /// Base frequency in hertz.
    pub base_frequency: u64,
    /// Channel spacing in hertz.
    pub channel_spacing: u32,
    /// Transmit offset in hertz.
    pub transmit_offset: i64,
}

/// A logged activity event for display in the TUI.
pub struct ActivityEvent {
    /// When this event occurred.
    pub timestamp: Instant,
    /// Human-readable description of the event.
    pub description: String,
    /// Whether this is an emergency event.
    pub is_emergency: bool,
}

/// A channel entry for display, combining channel info with active state.
pub struct ChannelDisplayEntry {
    /// Channel number.
    pub channel: u16,
    /// Frequency in MHz if resolvable.
    pub frequency: Option<f64>,
    /// Talkgroup currently using this channel, if any.
    pub active_talkgroup: Option<u16>,
}

impl MonitorState {
    /// Create a new monitor state with the specified grant timeout.
    pub fn new(grant_timeout: Duration) -> Self {
        Self {
            system_info: SystemInfo::new(),
            active_grants: HashMap::new(),
            identifiers: [None; 16],
            known_channels: Vec::new(),
            activity_events: VecDeque::new(),
            raw_log: VecDeque::new(),
            message_count: 0,
            grant_timeout,
        }
    }

    /// Process a JSON line from the decoder, updating all state.
    ///
    /// Parses the line, updates state, logs the raw line, and
    /// generates activity events. Parse errors are silently ignored
    /// (the raw line is still logged).
    pub fn process_message(&mut self, line: &str) {
        self.append_raw_log(line);
        self.message_count += 1;

        let message = match parse::parse_json_line(line) {
            Ok(msg) => msg,
            Err(_) => return,
        };

        self.apply_message(&message);
    }

    /// Remove grants that have not been refreshed within the timeout.
    pub fn expire_grants(&mut self) {
        let cutoff = Instant::now() - self.grant_timeout;
        self.active_grants
            .retain(|_, grant| grant.last_seen > cutoff);
    }

    /// Resolve a channel number to a frequency in MHz using the identifier table.
    ///
    /// Channel format: upper 4 bits = identifier, lower 12 bits = channel index.
    /// Frequency = base_frequency + channel_spacing * channel_index, converted to MHz.
    pub fn resolve_frequency(&self, channel: u16) -> Option<f64> {
        let identifier = (channel >> 12) as usize;
        let channel_index = channel & 0x0FFF;
        let entry = self.identifiers.get(identifier)?.as_ref()?;
        let hz = entry.base_frequency + entry.channel_spacing as u64 * channel_index as u64;
        Some(hz as f64 / 1_000_000.0)
    }

    /// Build a sorted list of all known channels with their current state.
    ///
    /// Sorted by frequency if resolvable, otherwise by channel number.
    pub fn channel_list(&self) -> Vec<ChannelDisplayEntry> {
        let mut entries: Vec<ChannelDisplayEntry> = self
            .known_channels
            .iter()
            .map(|&channel| {
                let frequency = self.resolve_frequency(channel);
                let active_talkgroup = self
                    .active_grants
                    .get(&channel)
                    .map(|grant| grant.talkgroup);
                ChannelDisplayEntry {
                    channel,
                    frequency,
                    active_talkgroup,
                }
            })
            .collect();

        entries.sort_by(|a, b| match (a.frequency, b.frequency) {
            (Some(fa), Some(fb)) => fa.partial_cmp(&fb).unwrap_or(std::cmp::Ordering::Equal),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.channel.cmp(&b.channel),
        });

        entries
    }

    /// Apply a parsed message to update internal state.
    fn apply_message(&mut self, message: &ParsedMessage) {
        match message {
            ParsedMessage::GroupVoiceGrant {
                channel,
                frequency,
                talkgroup,
                source,
            } => self.apply_grant(*channel, *frequency, *talkgroup, *source),
            ParsedMessage::GroupVoiceGrantUpdate {
                channel_a,
                frequency_a,
                talkgroup_a,
                channel_b,
                frequency_b,
                talkgroup_b,
            } => self.apply_grant_update(
                *channel_a,
                *frequency_a,
                *talkgroup_a,
                *channel_b,
                *frequency_b,
                *talkgroup_b,
            ),
            ParsedMessage::GroupVoiceGrantUpdateExplicit {
                receive_channel,
                receive_frequency,
                talkgroup,
            } => self.apply_grant_explicit(*receive_channel, *receive_frequency, *talkgroup),
            ParsedMessage::IdentifierUpdate {
                identifier,
                base_frequency,
                channel_spacing,
                transmit_offset,
            }
            | ParsedMessage::IdentifierUpdateTdma {
                identifier,
                base_frequency,
                channel_spacing,
                transmit_offset,
            } => self.update_identifier(
                *identifier,
                *base_frequency,
                *channel_spacing,
                *transmit_offset,
            ),
            ParsedMessage::NetworkStatusBroadcast {
                wacn,
                system_id,
                channel,
                frequency,
            } => self.apply_network_status(wacn, system_id, *channel, *frequency),
            ParsedMessage::RfssStatusBroadcast {
                system_id,
                rfss_id,
                site_id,
                channel,
                frequency,
            } => self.apply_rfss_status(system_id, *rfss_id, *site_id, *channel, *frequency),
            ParsedMessage::EmergencyAlarm { target, source } => {
                self.add_activity(format!("EMERGENCY: src {source} target {target}"), true);
            }
            _ => {}
        }
    }

    /// Apply a group voice channel grant.
    fn apply_grant(&mut self, channel: u16, frequency: Option<f64>, talkgroup: u16, source: u32) {
        self.upsert_grant(channel, frequency, talkgroup, Some(source));
        self.add_activity(
            format!("Grant: TG {talkgroup} on CH {channel} (src {source})"),
            false,
        );
    }

    /// Apply a group voice channel grant update (two channels).
    fn apply_grant_update(
        &mut self,
        channel_a: u16,
        frequency_a: Option<f64>,
        talkgroup_a: u16,
        channel_b: u16,
        frequency_b: Option<f64>,
        talkgroup_b: u16,
    ) {
        self.upsert_grant(channel_a, frequency_a, talkgroup_a, None);
        self.upsert_grant(channel_b, frequency_b, talkgroup_b, None);
        self.add_activity(
            format!("Update: TG {talkgroup_a} CH {channel_a}, TG {talkgroup_b} CH {channel_b}"),
            false,
        );
    }

    /// Apply an explicit grant update.
    fn apply_grant_explicit(&mut self, channel: u16, frequency: Option<f64>, talkgroup: u16) {
        self.upsert_grant(channel, frequency, talkgroup, None);
        self.add_activity(
            format!("Explicit update: TG {talkgroup} on CH {channel}"),
            false,
        );
    }

    /// Apply a network status broadcast.
    fn apply_network_status(
        &mut self,
        wacn: &str,
        system_id: &str,
        channel: u16,
        frequency: Option<f64>,
    ) {
        self.system_info.wacn = Some(wacn.to_string());
        self.system_info.system_id = Some(system_id.to_string());
        self.system_info.control_channel = Some(channel);
        self.system_info.control_frequency = frequency.or(self.resolve_frequency(channel));
    }

    /// Apply an RFSS status broadcast.
    fn apply_rfss_status(
        &mut self,
        system_id: &str,
        rfss_id: u8,
        site_id: u8,
        channel: u16,
        frequency: Option<f64>,
    ) {
        self.system_info.system_id = Some(system_id.to_string());
        self.system_info.rfss_id = Some(rfss_id);
        self.system_info.site_id = Some(site_id);
        self.system_info.control_channel = Some(channel);
        self.system_info.control_frequency = frequency.or(self.resolve_frequency(channel));
    }

    /// Insert or update a grant, tracking the channel in known_channels.
    fn upsert_grant(
        &mut self,
        channel: u16,
        frequency: Option<f64>,
        talkgroup: u16,
        source: Option<u32>,
    ) {
        let resolved_frequency = frequency.or_else(|| self.resolve_frequency(channel));
        self.track_channel(channel);

        let grant = self.active_grants.entry(channel).or_insert(ActiveGrant {
            channel,
            frequency: resolved_frequency,
            talkgroup,
            source,
            last_seen: Instant::now(),
        });
        grant.talkgroup = talkgroup;
        grant.last_seen = Instant::now();
        grant.frequency = resolved_frequency;
        if source.is_some() {
            grant.source = source;
        }
    }

    /// Add a channel to known_channels if not already present (maintains sorted order).
    fn track_channel(&mut self, channel: u16) {
        if let Err(pos) = self.known_channels.binary_search(&channel) {
            self.known_channels.insert(pos, channel);
        }
    }

    /// Update the identifier table entry.
    fn update_identifier(
        &mut self,
        identifier: u8,
        base_frequency: u64,
        channel_spacing: u32,
        transmit_offset: i64,
    ) {
        if (identifier as usize) < self.identifiers.len() {
            self.identifiers[identifier as usize] = Some(IdentifierEntry {
                base_frequency,
                channel_spacing,
                transmit_offset,
            });
        }
    }

    /// Add an activity event, enforcing the maximum capacity.
    fn add_activity(&mut self, description: String, is_emergency: bool) {
        if self.activity_events.len() >= MAX_ACTIVITY_EVENTS {
            self.activity_events.pop_front();
        }
        self.activity_events.push_back(ActivityEvent {
            timestamp: Instant::now(),
            description,
            is_emergency,
        });
    }

    /// Append a raw log line, enforcing the maximum capacity.
    fn append_raw_log(&mut self, line: &str) {
        if self.raw_log.len() >= MAX_RAW_LOG {
            self.raw_log.pop_front();
        }
        self.raw_log.push_back(line.to_string());
    }
}

impl SystemInfo {
    /// Create empty system info with all fields unset.
    fn new() -> Self {
        Self {
            wacn: None,
            system_id: None,
            rfss_id: None,
            site_id: None,
            control_channel: None,
            control_frequency: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_state() -> MonitorState {
        MonitorState::new(Duration::from_secs(3))
    }

    #[test]
    fn grant_appears_after_group_voice_grant() {
        let mut state = default_state();
        let json = r#"{"name":"GRP_V_CH_GRANT","channel":3851,"frequency":null,"talkgroup":100,"source":12345}"#;
        state.process_message(json);

        assert!(state.active_grants.contains_key(&3851));
        let grant = &state.active_grants[&3851];
        assert_eq!(grant.talkgroup, 100);
        assert_eq!(grant.source, Some(12345));
    }

    #[test]
    fn grant_update_refreshes_both_channels() {
        let mut state = default_state();
        let json = r#"{"name":"GRP_V_CH_GRANT_UPDT","channel_a":433,"frequency_a":null,"talkgroup_a":2821,"channel_b":135,"frequency_b":null,"talkgroup_b":3769}"#;
        state.process_message(json);

        assert!(state.active_grants.contains_key(&433));
        assert!(state.active_grants.contains_key(&135));
        assert_eq!(state.active_grants[&433].talkgroup, 2821);
        assert_eq!(state.active_grants[&135].talkgroup, 3769);
    }

    #[test]
    fn system_info_populated_from_net_sts_bcst() {
        let mut state = default_state();
        let json = r#"{"name":"NET_STS_BCST","wacn":"0x0C018","system_id":"0x704","channel":459,"frequency":null}"#;
        state.process_message(json);

        assert_eq!(state.system_info.wacn.as_deref(), Some("0x0C018"));
        assert_eq!(state.system_info.system_id.as_deref(), Some("0x704"));
        assert_eq!(state.system_info.control_channel, Some(459));
    }

    #[test]
    fn system_info_populated_from_rfss_sts_bcst() {
        let mut state = default_state();
        let json = r#"{"name":"RFSS_STS_BCST","system_id":"0x5F2","rfss_id":1,"site_id":12,"channel":215,"frequency":null}"#;
        state.process_message(json);

        assert_eq!(state.system_info.system_id.as_deref(), Some("0x5F2"));
        assert_eq!(state.system_info.rfss_id, Some(1));
        assert_eq!(state.system_info.site_id, Some(12));
    }

    #[test]
    fn identifier_table_populated_from_ident_up() {
        let mut state = default_state();
        let json = r#"{"name":"IDENT_UP","identifier":6,"bandwidth":12500,"transmit_offset":-45000000,"channel_spacing":6250,"base_frequency":851006250}"#;
        state.process_message(json);

        let entry = state.identifiers[6].unwrap();
        assert_eq!(entry.base_frequency, 851006250);
        assert_eq!(entry.channel_spacing, 6250);
        assert_eq!(entry.transmit_offset, -45000000);
    }

    #[test]
    fn identifier_table_populated_from_iden_up_tdma() {
        let mut state = default_state();
        let json = r#"{"name":"IDEN_UP_TDMA","identifier":4,"bandwidth":12500,"transmit_offset":-39000000,"channel_spacing":12500,"base_frequency":935012500}"#;
        state.process_message(json);

        let entry = state.identifiers[4].unwrap();
        assert_eq!(entry.base_frequency, 935012500);
        assert_eq!(entry.channel_spacing, 12500);
    }

    #[test]
    fn grant_expiry_removes_stale_grants() {
        let mut state = MonitorState::new(Duration::from_millis(50));
        let json = r#"{"name":"GRP_V_CH_GRANT","channel":100,"frequency":null,"talkgroup":200,"source":300}"#;
        state.process_message(json);
        assert_eq!(state.active_grants.len(), 1);

        // Wait for the grant to expire
        std::thread::sleep(Duration::from_millis(100));
        state.expire_grants();
        assert!(state.active_grants.is_empty());
    }

    #[test]
    fn fresh_grant_not_expired() {
        let mut state = MonitorState::new(Duration::from_secs(10));
        let json = r#"{"name":"GRP_V_CH_GRANT","channel":100,"frequency":null,"talkgroup":200,"source":300}"#;
        state.process_message(json);

        state.expire_grants();
        assert_eq!(state.active_grants.len(), 1);
    }

    #[test]
    fn resolve_frequency_with_ident_table() {
        let mut state = default_state();
        // Set up identifier 6 with known parameters
        let json = r#"{"name":"IDENT_UP","identifier":6,"bandwidth":12500,"transmit_offset":-45000000,"channel_spacing":6250,"base_frequency":851006250}"#;
        state.process_message(json);

        // Channel 0x6009: identifier=6, index=9
        let freq = state.resolve_frequency(0x6009);
        assert!(freq.is_some());
        let mhz = freq.unwrap();
        // 851006250 + 6250 * 9 = 851062500 Hz = 851.0625 MHz
        assert!((mhz - 851.0625).abs() < 0.0001);
    }

    #[test]
    fn resolve_frequency_returns_none_without_ident() {
        let state = default_state();
        assert!(state.resolve_frequency(0x6009).is_none());
    }

    #[test]
    fn channel_list_sorted_by_frequency() {
        let mut state = default_state();

        // Add ident so channels can be resolved
        state.identifiers[6] = Some(IdentifierEntry {
            base_frequency: 851_000_000,
            channel_spacing: 12_500,
            transmit_offset: 0,
        });

        // Add grants on different channels (higher channel index = higher freq)
        let grant_high = r#"{"name":"GRP_V_CH_GRANT","channel":24588,"frequency":null,"talkgroup":100,"source":1}"#;
        let grant_low = r#"{"name":"GRP_V_CH_GRANT","channel":24578,"frequency":null,"talkgroup":200,"source":2}"#;
        state.process_message(grant_high); // 0x600C = ident 6, index 12
        state.process_message(grant_low); // 0x6002 = ident 6, index 2

        let list = state.channel_list();
        assert_eq!(list.len(), 2);
        // Lower frequency should come first
        assert!(list[0].frequency.unwrap() < list[1].frequency.unwrap());
        assert_eq!(list[0].channel, 24578); // index 2
        assert_eq!(list[1].channel, 24588); // index 12
    }

    #[test]
    fn channel_list_sorted_by_channel_number_without_frequency() {
        let mut state = default_state();

        let grant1 = r#"{"name":"GRP_V_CH_GRANT","channel":500,"frequency":null,"talkgroup":100,"source":1}"#;
        let grant2 = r#"{"name":"GRP_V_CH_GRANT","channel":200,"frequency":null,"talkgroup":200,"source":2}"#;
        state.process_message(grant1);
        state.process_message(grant2);

        let list = state.channel_list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].channel, 200);
        assert_eq!(list[1].channel, 500);
    }

    #[test]
    fn known_channels_tracked_and_sorted() {
        let mut state = default_state();

        let grant1 = r#"{"name":"GRP_V_CH_GRANT","channel":500,"frequency":null,"talkgroup":100,"source":1}"#;
        let grant2 = r#"{"name":"GRP_V_CH_GRANT","channel":200,"frequency":null,"talkgroup":200,"source":2}"#;
        let grant3 = r#"{"name":"GRP_V_CH_GRANT","channel":500,"frequency":null,"talkgroup":300,"source":3}"#;
        state.process_message(grant1);
        state.process_message(grant2);
        state.process_message(grant3); // duplicate channel

        assert_eq!(state.known_channels, vec![200, 500]);
    }

    #[test]
    fn message_count_incremented() {
        let mut state = default_state();
        assert_eq!(state.message_count, 0);

        state.process_message(r#"{"name":"NET_STS_BCST","wacn":"0x0C018","system_id":"0x704","channel":459,"frequency":null}"#);
        assert_eq!(state.message_count, 1);

        state.process_message(r#"{"name":"UNKNOWN","data":"FF"}"#);
        assert_eq!(state.message_count, 2);
    }

    #[test]
    fn raw_log_capped_at_max() {
        let mut state = default_state();
        let json = r#"{"name":"NET_STS_BCST","wacn":"0x0C018","system_id":"0x704","channel":459,"frequency":null}"#;

        for _ in 0..600 {
            state.process_message(json);
        }

        assert_eq!(state.raw_log.len(), MAX_RAW_LOG);
    }

    #[test]
    fn activity_events_capped_at_max() {
        let mut state = default_state();

        // Each grant produces an activity event
        for i in 0..1100u16 {
            let json = format!(
                r#"{{"name":"GRP_V_CH_GRANT","channel":{i},"frequency":null,"talkgroup":100,"source":1}}"#
            );
            state.process_message(&json);
        }

        assert_eq!(state.activity_events.len(), MAX_ACTIVITY_EVENTS);
    }

    #[test]
    fn emergency_alarm_produces_emergency_event() {
        let mut state = default_state();
        let json = r#"{"name":"EMERGENCY_ALRM","target":5000,"source":12345}"#;
        state.process_message(json);

        assert_eq!(state.activity_events.len(), 1);
        assert!(state.activity_events[0].is_emergency);
        assert!(state.activity_events[0].description.contains("EMERGENCY"));
    }

    #[test]
    fn malformed_json_still_logged() {
        let mut state = default_state();
        state.process_message("not json at all");

        assert_eq!(state.message_count, 1);
        assert_eq!(state.raw_log.len(), 1);
        assert_eq!(state.raw_log[0], "not json at all");
        assert!(state.active_grants.is_empty());
    }

    #[test]
    fn grant_update_explicit_creates_grant() {
        let mut state = default_state();
        let json = r#"{"name":"GRP_V_CH_GRANT_UPDT_EXP","receive_channel":500,"receive_frequency":852.5,"talkgroup":100}"#;
        state.process_message(json);

        assert!(state.active_grants.contains_key(&500));
        assert_eq!(state.active_grants[&500].talkgroup, 100);
        assert!((state.active_grants[&500].frequency.unwrap() - 852.5).abs() < 0.001);
    }

    #[test]
    fn grant_frequency_resolved_retroactively() {
        let mut state = default_state();

        // First, create a grant without frequency resolution
        let grant = r#"{"name":"GRP_V_CH_GRANT","channel":24585,"frequency":null,"talkgroup":100,"source":1}"#;
        state.process_message(grant);
        assert!(state.active_grants[&24585].frequency.is_none());

        // Now add the identifier
        let ident = r#"{"name":"IDENT_UP","identifier":6,"bandwidth":12500,"transmit_offset":0,"channel_spacing":12500,"base_frequency":851000000}"#;
        state.process_message(ident);

        // Refresh the grant — now frequency should resolve
        let grant2 = r#"{"name":"GRP_V_CH_GRANT","channel":24585,"frequency":null,"talkgroup":100,"source":1}"#;
        state.process_message(grant2);
        // 0x6009 = ident 6, index 9 => 851000000 + 12500*9 = 851112500 Hz = 851.1125 MHz
        let freq = state.active_grants[&24585].frequency.unwrap();
        assert!((freq - 851.1125).abs() < 0.0001);
    }
}
