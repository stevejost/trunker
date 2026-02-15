//! Domain newtypes for P25 protocol values.
//!
//! Every domain concept gets its own type to prevent mixing up bare integers.
//! These types are used throughout the protocol decoder and output layers.

use serde::Serialize;

/// RF frequency in Hertz.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct Frequency(pub u64);

/// P25 talkgroup identifier (16-bit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct TalkgroupId(pub u16);

/// P25 unit (radio) identifier (24-bit, stored as u32).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct UnitId(pub u32);

/// Network Access Code (12-bit, stored as u16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct Nac(pub u16);

/// System ID (12-bit, stored as u16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct SystemId(pub u16);

/// Wide Area Communications Network identifier (20-bit, stored as u32).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct Wacn(pub u32);

/// RF Subsystem identifier (8-bit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct RfssId(pub u8);

/// Site identifier (8-bit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct SiteId(pub u8);

/// Logical channel identifier (16-bit: 4-bit ident prefix + 12-bit channel number).
///
/// The upper 4 bits select an entry from the identifier table (populated by
/// IDEN_UP messages), and the lower 12 bits are the channel number within
/// that identifier's frequency band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct ChannelId(pub u16);

impl ChannelId {
    /// Extract the 4-bit identifier prefix (bits 15-12).
    pub fn identifier(&self) -> ChannelIdentifier {
        ChannelIdentifier(((self.0 >> 12) & 0xF) as u8)
    }

    /// Extract the 12-bit channel number (bits 11-0).
    pub fn channel_number(&self) -> u16 {
        self.0 & 0x0FFF
    }
}

/// Channel identifier prefix (4-bit), used to look up frequency parameters
/// in the identifier table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct ChannelIdentifier(pub u8);

/// Manufacturer ID (8-bit). 0x00 = standard P25, 0x90 = Motorola.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct ManufacturerId(pub u8);

/// Service options byte from channel grant messages.
///
/// Reference: TIA-102.AABF-A Section 7.2.1
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct ServiceOptions(pub u8);

impl std::fmt::Display for Frequency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:.5} MHz", self.0 as f64 / 1_000_000.0)
    }
}

impl std::fmt::Display for Nac {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "0x{:03X}", self.0)
    }
}

impl std::fmt::Display for SystemId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "0x{:03X}", self.0)
    }
}

impl std::fmt::Display for Wacn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "0x{:05X}", self.0)
    }
}

impl std::fmt::Display for ChannelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "0x{:04X}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_id_extracts_identifier_and_number() {
        let channel = ChannelId(0x3019);
        assert_eq!(channel.identifier(), ChannelIdentifier(3));
        assert_eq!(channel.channel_number(), 0x019);
    }

    #[test]
    fn channel_id_zero() {
        let channel = ChannelId(0x0000);
        assert_eq!(channel.identifier(), ChannelIdentifier(0));
        assert_eq!(channel.channel_number(), 0);
    }

    #[test]
    fn channel_id_max_values() {
        let channel = ChannelId(0xFFFF);
        assert_eq!(channel.identifier(), ChannelIdentifier(0xF));
        assert_eq!(channel.channel_number(), 0xFFF);
    }

    #[test]
    fn frequency_display() {
        assert_eq!(format!("{}", Frequency(851_325_000)), "851.32500 MHz");
    }
}
