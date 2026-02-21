//! Integration test: feed real decoder JSON through MonitorState and verify correctness.

use std::time::Duration;

use trunker::monitor::parse::{ParsedMessage, parse_json_line};
use trunker::monitor::state::MonitorState;

/// Feed all 1223 lines from the SRRCS CQPSK sample through MonitorState.
fn load_sample_state() -> MonitorState {
    let mut state = MonitorState::new(Duration::from_secs(3));
    let jsonl = include_str!("../../../samples/op25_reference/trunker_cqpsk_output.jsonl");
    for line in jsonl.lines() {
        state.process_message(line);
    }
    state
}

// --- Full-file integration tests ---

#[test]
fn all_messages_processed() {
    let state = load_sample_state();
    assert_eq!(
        state.message_count, 1223,
        "expected 1223 messages from sample file"
    );
}

#[test]
fn system_info_wacn_populated() {
    let state = load_sample_state();
    assert!(
        state.system_info.wacn.is_some(),
        "WACN should be populated from NET_STS_BCST/NET2_STS_BCST"
    );
    // Last NET_STS_BCST in file is on line 1222 with WACN=0x0C018
    assert_eq!(state.system_info.wacn.as_deref(), Some("0x0C018"));
}

#[test]
fn system_info_system_id_populated() {
    let state = load_sample_state();
    assert!(
        state.system_info.system_id.is_some(),
        "system_id should be populated from NET_STS_BCST"
    );
    // Last setter is NET_STS_BCST on line 1222 with system_id=0x704
    assert_eq!(state.system_info.system_id.as_deref(), Some("0x704"));
}

#[test]
fn system_info_rfss_populated() {
    let state = load_sample_state();
    assert_eq!(
        state.system_info.rfss_id,
        Some(1),
        "RFSS ID should be 1 from RFSS_STS_BCST"
    );
}

#[test]
fn system_info_site_populated() {
    let state = load_sample_state();
    assert_eq!(
        state.system_info.site_id,
        Some(12),
        "Site ID should be 12 from RFSS_STS_BCST"
    );
}

#[test]
fn identifier_table_has_entries() {
    let state = load_sample_state();
    let populated_count = state.identifiers.iter().filter(|id| id.is_some()).count();
    assert!(
        populated_count >= 3,
        "expected at least 3 identifier entries, got {populated_count}"
    );
    // Verify specific identifier slots that appear in the sample data
    assert!(
        state.identifiers[0].is_some(),
        "identifier 0 should be populated"
    );
    assert!(
        state.identifiers[1].is_some(),
        "identifier 1 should be populated"
    );
    assert!(
        state.identifiers[4].is_some(),
        "identifier 4 should be populated"
    );
}

#[test]
fn identifier_values_correct() {
    let state = load_sample_state();
    let entry = state.identifiers[0].expect("identifier 0 should exist");
    assert_eq!(entry.base_frequency, 851006250);
    assert_eq!(entry.channel_spacing, 6250);
    assert_eq!(entry.transmit_offset, -45000000);
}

#[test]
fn activity_events_nonempty() {
    let state = load_sample_state();
    assert!(
        !state.activity_events.is_empty(),
        "activity_events should have entries from grants"
    );
}

#[test]
fn activity_events_capped_at_1000() {
    let state = load_sample_state();
    assert!(
        state.activity_events.len() <= 1000,
        "activity_events should be capped at 1000, got {}",
        state.activity_events.len()
    );
}

#[test]
fn known_channels_nonempty() {
    let state = load_sample_state();
    assert!(
        !state.known_channels.is_empty(),
        "known_channels should be populated from grants"
    );
}

#[test]
fn known_channels_are_sorted() {
    let state = load_sample_state();
    for window in state.known_channels.windows(2) {
        assert!(
            window[0] < window[1],
            "known_channels should be sorted: found {} before {}",
            window[0],
            window[1]
        );
    }
}

#[test]
fn raw_log_nonempty() {
    let state = load_sample_state();
    assert!(!state.raw_log.is_empty(), "raw_log should contain entries");
}

#[test]
fn raw_log_capped_at_500() {
    let state = load_sample_state();
    assert!(
        state.raw_log.len() <= 500,
        "raw_log should be capped at 500, got {}",
        state.raw_log.len()
    );
    // With 1223 messages, the cap should be hit exactly
    assert_eq!(
        state.raw_log.len(),
        500,
        "raw_log should be exactly at capacity"
    );
}

#[test]
fn no_panics_on_any_line() {
    // This test verifies that process_message never panics on any real data.
    // If this test passes, we know the parser handles all 1223 lines gracefully.
    let state = load_sample_state();
    assert_eq!(state.message_count, 1223);
}

#[test]
fn frequency_resolution_works_after_ident_table_populated() {
    let state = load_sample_state();
    // Identifier 0: base_frequency=851006250, channel_spacing=6250
    // Channel 0x0009 (identifier=0, index=9) => 851006250 + 6250*9 = 851062500 Hz = 851.0625 MHz
    let freq = state.resolve_frequency(0x0009);
    assert!(freq.is_some(), "should resolve frequency with identifier 0");
    let mhz = freq.unwrap();
    assert!(
        (mhz - 851.0625).abs() < 0.0001,
        "expected ~851.0625 MHz, got {mhz}"
    );
}

// --- Parse module tests with real sample lines ---

#[test]
fn parse_real_group_voice_grant() {
    let line = r#"{"nac":"0x5FC","opcode":"0x00","name":"GRP_V_CH_GRANT","last_block":false,"manufacturer_id":144,"channel":3851,"frequency":null,"talkgroup":3851,"source":985871}"#;
    let msg = parse_json_line(line).unwrap();
    match msg {
        ParsedMessage::GroupVoiceGrant {
            channel,
            talkgroup,
            source,
            frequency,
        } => {
            assert_eq!(channel, 3851);
            assert_eq!(talkgroup, 3851);
            assert_eq!(source, 985871);
            assert!(frequency.is_none());
        }
        other => panic!("expected GroupVoiceGrant, got {other:?}"),
    }
}

#[test]
fn parse_real_grant_update() {
    let line = r#"{"nac":"0x5FC","opcode":"0x02","name":"GRP_V_CH_GRANT_UPDT","last_block":true,"manufacturer_id":0,"channel_a":433,"frequency_a":null,"talkgroup_a":2821,"channel_b":135,"frequency_b":null,"talkgroup_b":3769}"#;
    let msg = parse_json_line(line).unwrap();
    match msg {
        ParsedMessage::GroupVoiceGrantUpdate {
            channel_a,
            talkgroup_a,
            channel_b,
            talkgroup_b,
            ..
        } => {
            assert_eq!(channel_a, 433);
            assert_eq!(talkgroup_a, 2821);
            assert_eq!(channel_b, 135);
            assert_eq!(talkgroup_b, 3769);
        }
        other => panic!("expected GroupVoiceGrantUpdate, got {other:?}"),
    }
}

#[test]
fn parse_real_net_sts_bcst() {
    let line = r#"{"nac":"0x5FC","opcode":"0x39","name":"NET_STS_BCST","last_block":true,"manufacturer_id":0,"wacn":"0x0C018","system_id":"0x704","channel":459,"frequency":null}"#;
    let msg = parse_json_line(line).unwrap();
    match msg {
        ParsedMessage::NetworkStatusBroadcast {
            wacn,
            system_id,
            channel,
            frequency,
        } => {
            assert_eq!(wacn, "0x0C018");
            assert_eq!(system_id, "0x704");
            assert_eq!(channel, 459);
            assert!(frequency.is_none());
        }
        other => panic!("expected NetworkStatusBroadcast, got {other:?}"),
    }
}

#[test]
fn parse_real_rfss_sts_bcst() {
    let line = r#"{"nac":"0x5FC","opcode":"0x3A","name":"RFSS_STS_BCST","last_block":false,"manufacturer_id":0,"system_id":"0x5F2","rfss_id":1,"site_id":12,"channel":215,"frequency":null}"#;
    let msg = parse_json_line(line).unwrap();
    match msg {
        ParsedMessage::RfssStatusBroadcast {
            system_id,
            rfss_id,
            site_id,
            channel,
            ..
        } => {
            assert_eq!(system_id, "0x5F2");
            assert_eq!(rfss_id, 1);
            assert_eq!(site_id, 12);
            assert_eq!(channel, 215);
        }
        other => panic!("expected RfssStatusBroadcast, got {other:?}"),
    }
}

#[test]
fn parse_real_iden_up_tdma() {
    let line = r#"{"nac":"0x5FC","opcode":"0x3D","name":"IDEN_UP_TDMA","last_block":false,"manufacturer_id":0,"identifier":4,"bandwidth":12500,"transmit_offset":-39000000,"channel_spacing":12500,"base_frequency":935012500}"#;
    let msg = parse_json_line(line).unwrap();
    match msg {
        ParsedMessage::IdentifierUpdateTdma {
            identifier,
            base_frequency,
            channel_spacing,
            transmit_offset,
        } => {
            assert_eq!(identifier, 4);
            assert_eq!(base_frequency, 935012500);
            assert_eq!(channel_spacing, 12500);
            assert_eq!(transmit_offset, -39000000);
        }
        other => panic!("expected IdentifierUpdateTdma, got {other:?}"),
    }
}

#[test]
fn parse_real_adj_sts_bcst() {
    let line = r#"{"nac":"0x5FC","opcode":"0x3C","name":"ADJ_STS_BCST","last_block":true,"manufacturer_id":0,"system_id":"0x5F2","rfss_id":1,"site_id":3,"channel":451,"frequency":null}"#;
    let msg = parse_json_line(line).unwrap();
    match msg {
        ParsedMessage::AdjacentStatusBroadcast {
            system_id,
            rfss_id,
            site_id,
            channel,
            ..
        } => {
            assert_eq!(system_id, "0x5F2");
            assert_eq!(rfss_id, 1);
            assert_eq!(site_id, 3);
            assert_eq!(channel, 451);
        }
        other => panic!("expected AdjacentStatusBroadcast, got {other:?}"),
    }
}

#[test]
fn parse_unknown_opcode_from_sample() {
    let line = r#"{"nac":"0x5FC","opcode":"0x30","name":"UNKNOWN","last_block":false,"manufacturer_id":0,"data":"00042A344F50EBD6"}"#;
    let msg = parse_json_line(line).unwrap();
    match msg {
        ParsedMessage::Other { name } => {
            assert_eq!(name, "UNKNOWN");
        }
        other => panic!("expected Other, got {other:?}"),
    }
}

#[test]
fn parse_grant_with_resolved_frequency() {
    // Last lines of the sample file have resolved frequencies
    let line = r#"{"nac":"0x5FC","opcode":"0x00","name":"GRP_V_CH_GRANT","last_block":false,"manufacturer_id":144,"channel":3847,"frequency":875.05,"talkgroup":3847,"source":984847}"#;
    let msg = parse_json_line(line).unwrap();
    match msg {
        ParsedMessage::GroupVoiceGrant {
            channel,
            frequency,
            talkgroup,
            source,
        } => {
            assert_eq!(channel, 3847);
            assert!((frequency.unwrap() - 875.05).abs() < 0.001);
            assert_eq!(talkgroup, 3847);
            assert_eq!(source, 984847);
        }
        other => panic!("expected GroupVoiceGrant, got {other:?}"),
    }
}
