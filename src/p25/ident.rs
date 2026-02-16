//! Identifier table for resolving channel numbers to frequencies.
//!
//! P25 systems broadcast identifier updates (IDENT_UP, IDEN_UP_VU, etc.)
//! that map 4-bit identifier IDs to frequency band parameters. Channel
//! numbers in grant messages combine an identifier (upper 4 bits) with
//! a channel index (lower 12 bits) to compute the actual frequency.

use crate::p25::tsbk::{Tsbk, TsbkPayload};
use crate::p25::types::{ChannelNumber, Frequency};

/// Parameters for a frequency band identified by a 4-bit identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdentEntry {
    /// Base receive frequency in hertz.
    pub base_frequency: u64,
    /// Channel spacing in hertz.
    pub channel_spacing: u32,
    /// Transmit offset in hertz (signed).
    pub transmit_offset: i64,
}

impl IdentEntry {
    /// Compute the receive frequency for a channel index.
    pub fn receive_frequency(&self, channel_index: u16) -> Frequency {
        let freq = self.base_frequency + self.channel_spacing as u64 * channel_index as u64;
        Frequency::from_hz(freq)
    }

    /// Compute the transmit frequency for a channel index.
    pub fn transmit_frequency(&self, channel_index: u16) -> Frequency {
        let rx = self.base_frequency + self.channel_spacing as u64 * channel_index as u64;
        let tx = (rx as i64 + self.transmit_offset) as u64;
        Frequency::from_hz(tx)
    }
}

/// Table mapping 4-bit identifier IDs to frequency band parameters.
///
/// Populated by IDENT_UP (0x20), IDEN_UP_TDMA (0x33), IDEN_UP_VU (0x34),
/// and channel parameters update (0x3D) messages.
#[derive(Debug, Clone, Default)]
pub struct IdentTable {
    entries: [Option<IdentEntry>; 16],
}

impl IdentTable {
    /// Create an empty identifier table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Update the table from a parsed TSBK if it is an identifier update.
    ///
    /// Returns `true` if the table was updated, `false` if the TSBK
    /// was not an identifier update message.
    pub fn update(&mut self, tsbk: &Tsbk) -> bool {
        match &tsbk.payload {
            TsbkPayload::IdentifierUpdate {
                identifier,
                base_frequency,
                channel_spacing,
                transmit_offset,
                ..
            } => {
                self.entries[*identifier as usize] = Some(IdentEntry {
                    base_frequency: *base_frequency,
                    channel_spacing: *channel_spacing,
                    transmit_offset: *transmit_offset,
                });
                true
            }
            _ => false,
        }
    }

    /// Look up the entry for a given identifier ID.
    pub fn get(&self, identifier: u8) -> Option<&IdentEntry> {
        self.entries.get(identifier as usize)?.as_ref()
    }

    /// Resolve a channel number to a receive frequency.
    ///
    /// Returns `None` if the identifier has not been populated yet.
    pub fn resolve_frequency(&self, channel: ChannelNumber) -> Option<Frequency> {
        let entry = self.get(channel.identifier())?;
        Some(entry.receive_frequency(channel.index()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p25::tsbk::{self, TsbkOpcode};

    fn make_ident_update_tsbk(
        identifier: u8,
        base_frequency: u64,
        channel_spacing: u32,
        transmit_offset: i64,
    ) -> Tsbk {
        Tsbk {
            header: tsbk::TsbkHeader {
                last_block: true,
                protected: false,
                opcode: TsbkOpcode::IdentifierUpdate,
                manufacturer_id: 0,
            },
            payload: TsbkPayload::IdentifierUpdate {
                identifier,
                bandwidth: 12_500,
                transmit_offset,
                channel_spacing,
                base_frequency,
            },
        }
    }

    #[test]
    fn empty_table_returns_none() {
        let table = IdentTable::new();
        let channel = ChannelNumber::new(0x6009);
        assert!(table.resolve_frequency(channel).is_none());
    }

    #[test]
    fn update_populates_entry() {
        let mut table = IdentTable::new();
        let tsbk = make_ident_update_tsbk(6, 851_006_250, 6_250, -45_000_000);
        assert!(table.update(&tsbk));
        assert!(table.get(6).is_some());
    }

    #[test]
    fn resolve_frequency_after_update() {
        let mut table = IdentTable::new();
        let tsbk = make_ident_update_tsbk(6, 851_006_250, 6_250, -45_000_000);
        table.update(&tsbk);

        // Channel 0x6009: identifier=6, index=9
        let channel = ChannelNumber::new(0x6009);
        let freq = table.resolve_frequency(channel).unwrap();
        assert_eq!(freq.hz(), 851_062_500);
    }

    #[test]
    fn receive_and_transmit_frequencies() {
        let entry = IdentEntry {
            base_frequency: 851_006_250,
            channel_spacing: 6_250,
            transmit_offset: -45_000_000,
        };
        let rx = entry.receive_frequency(9);
        let tx = entry.transmit_frequency(9);
        assert_eq!(rx.hz(), 851_062_500);
        assert_eq!(tx.hz(), 806_062_500);
    }

    #[test]
    fn update_ignores_non_ident_tsbk() {
        let mut table = IdentTable::new();
        let tsbk = Tsbk {
            header: tsbk::TsbkHeader {
                last_block: true,
                protected: false,
                opcode: TsbkOpcode::NetworkStatusBroadcast,
                manufacturer_id: 0,
            },
            payload: TsbkPayload::NetworkStatusBroadcast {
                wacn: crate::p25::types::Wacn::new(0),
                system_id: crate::p25::types::SystemId::new(0),
                channel: ChannelNumber::new(0),
            },
        };
        assert!(!table.update(&tsbk));
    }

    #[test]
    fn multiple_identifiers_coexist() {
        let mut table = IdentTable::new();
        table.update(&make_ident_update_tsbk(6, 851_006_250, 6_250, -45_000_000));
        table.update(&make_ident_update_tsbk(7, 866_000_000, 12_500, 0));

        let ch6 = ChannelNumber::new(0x6009);
        let ch7 = ChannelNumber::new(0x7004);
        assert_eq!(table.resolve_frequency(ch6).unwrap().hz(), 851_062_500);
        assert_eq!(table.resolve_frequency(ch7).unwrap().hz(), 866_050_000);
    }

    #[test]
    fn update_overwrites_existing_entry() {
        let mut table = IdentTable::new();
        table.update(&make_ident_update_tsbk(6, 851_006_250, 6_250, -45_000_000));
        table.update(&make_ident_update_tsbk(6, 852_000_000, 12_500, 0));

        let channel = ChannelNumber::new(0x6009);
        // Should use the second (newer) entry
        assert_eq!(table.resolve_frequency(channel).unwrap().hz(), 852_112_500);
    }
}
