//! Channel identifier table for frequency resolution.
//!
//! P25 systems use logical channel numbers (16-bit values) in channel grant
//! messages. To determine the actual RF frequency, the decoder must maintain
//! a table of identifier entries populated from IDEN_UP TSBK messages.
//!
//! The upper 4 bits of a channel ID select an entry from this table, and
//! the lower 12 bits are the channel number. The frequency is computed as:
//!
//! ```text
//! frequency = base_freq + (channel_spacing * channel_number)
//! ```
//!
//! Reference: TIA-102.AABF-A Section 7.2.31

use std::collections::HashMap;

use crate::p25::types::{ChannelId, ChannelIdentifier, Frequency};

/// Frequency parameters for a channel identifier, populated from IDEN_UP messages.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelParams {
    /// Base frequency in Hertz.
    pub base_frequency: u64,
    /// Channel spacing in Hertz.
    pub channel_spacing: u32,
    /// Transmit offset in Hertz (signed: positive or negative).
    pub transmit_offset: i32,
}

/// Table mapping 4-bit channel identifier prefixes to frequency parameters.
///
/// Populated from IDEN_UP (0x20/0x3D), IDEN_UP_VU (0x34), and IDEN_UP_TDMA (0x33)
/// TSBK messages. Used to resolve logical channel IDs in channel grants to
/// actual RF frequencies.
///
/// The table has at most 16 entries (4-bit key space).
///
/// Reference: TIA-102.AABF-A Section 7.2.31
#[derive(Debug, Clone, Default)]
pub struct IdentifierTable {
    entries: HashMap<u8, ChannelParams>,
}

impl IdentifierTable {
    /// Create an empty identifier table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or update an identifier entry.
    pub fn update(&mut self, identifier: ChannelIdentifier, params: ChannelParams) {
        self.entries.insert(identifier.0, params);
    }

    /// Resolve a 16-bit channel ID to a receive (downlink) RF frequency in Hertz.
    ///
    /// Returns `None` if the identifier prefix has not been seen in any
    /// IDEN_UP message yet.
    ///
    /// Formula: `frequency = base_freq + (channel_spacing * channel_number)`
    pub fn resolve_frequency(&self, channel_id: ChannelId) -> Option<Frequency> {
        let ident = channel_id.identifier();
        let channel_number = channel_id.channel_number() as u64;
        let params = self.entries.get(&ident.0)?;
        let freq = params.base_frequency + (params.channel_spacing as u64 * channel_number);
        Some(Frequency(freq))
    }

    /// Resolve a channel ID to its transmit (uplink) frequency in Hertz.
    ///
    /// Returns `None` if the identifier prefix has not been seen yet.
    ///
    /// Formula: `frequency = base_freq + (channel_spacing * channel_number) + transmit_offset`
    pub fn resolve_transmit_frequency(&self, channel_id: ChannelId) -> Option<Frequency> {
        let ident = channel_id.identifier();
        let channel_number = channel_id.channel_number() as u64;
        let params = self.entries.get(&ident.0)?;
        let base = params.base_frequency + (params.channel_spacing as u64 * channel_number);
        let tx_freq = (base as i64 + params.transmit_offset as i64) as u64;
        Some(Frequency(tx_freq))
    }

    /// Check if an identifier entry exists.
    pub fn contains(&self, identifier: ChannelIdentifier) -> bool {
        self.entries.contains_key(&identifier.0)
    }

    /// Number of identifier entries in the table.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the table is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_channel_frequency() {
        let mut table = IdentifierTable::new();
        // Typical SRRCS identifier: base=851_012_500 Hz, spacing=12_500 Hz
        table.update(
            ChannelIdentifier(1),
            ChannelParams {
                base_frequency: 851_012_500,
                channel_spacing: 12_500,
                transmit_offset: -45_000_000,
            },
        );

        // Channel 0x1019 = ident=1, channel_number=0x019=25
        let channel = ChannelId(0x1019);
        let freq = table.resolve_frequency(channel).unwrap();
        assert_eq!(freq, Frequency(851_012_500 + 12_500 * 25));
        assert_eq!(freq, Frequency(851_325_000));
    }

    #[test]
    fn resolve_transmit_frequency() {
        let mut table = IdentifierTable::new();
        table.update(
            ChannelIdentifier(1),
            ChannelParams {
                base_frequency: 851_012_500,
                channel_spacing: 12_500,
                transmit_offset: -45_000_000,
            },
        );

        let channel = ChannelId(0x1019);
        let tx_freq = table.resolve_transmit_frequency(channel).unwrap();
        // 851_325_000 + (-45_000_000) = 806_325_000
        assert_eq!(tx_freq, Frequency(806_325_000));
    }

    #[test]
    fn resolve_unknown_identifier_returns_none() {
        let table = IdentifierTable::new();
        assert!(table.resolve_frequency(ChannelId(0x2000)).is_none());
    }

    #[test]
    fn update_overwrites_existing() {
        let mut table = IdentifierTable::new();
        table.update(
            ChannelIdentifier(1),
            ChannelParams {
                base_frequency: 100,
                channel_spacing: 10,
                transmit_offset: 0,
            },
        );
        table.update(
            ChannelIdentifier(1),
            ChannelParams {
                base_frequency: 200,
                channel_spacing: 20,
                transmit_offset: 0,
            },
        );

        let freq = table.resolve_frequency(ChannelId(0x1001)).unwrap();
        // base=200, spacing=20, channel_number=1 -> 200 + 20 = 220
        assert_eq!(freq, Frequency(220));
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn multiple_identifiers() {
        let mut table = IdentifierTable::new();
        table.update(
            ChannelIdentifier(0),
            ChannelParams {
                base_frequency: 800_000_000,
                channel_spacing: 25_000,
                transmit_offset: 0,
            },
        );
        table.update(
            ChannelIdentifier(3),
            ChannelParams {
                base_frequency: 900_000_000,
                channel_spacing: 12_500,
                transmit_offset: 0,
            },
        );

        assert_eq!(table.len(), 2);
        assert!(table.contains(ChannelIdentifier(0)));
        assert!(table.contains(ChannelIdentifier(3)));
        assert!(!table.contains(ChannelIdentifier(1)));

        // ident=0, ch=10 -> 800_000_000 + 25_000*10 = 800_250_000
        assert_eq!(
            table.resolve_frequency(ChannelId(0x000A)).unwrap(),
            Frequency(800_250_000)
        );
        // ident=3, ch=100 -> 900_000_000 + 12_500*100 = 901_250_000
        assert_eq!(
            table.resolve_frequency(ChannelId(0x3064)).unwrap(),
            Frequency(901_250_000)
        );
    }

    #[test]
    fn empty_table() {
        let table = IdentifierTable::new();
        assert!(table.is_empty());
        assert_eq!(table.len(), 0);
    }
}
