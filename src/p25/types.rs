//! Domain types for P25 protocol values.
//!
//! All types are newtypes wrapping primitives to ensure type safety
//! and prevent mixing up bare integers.

use serde::Serialize;
use std::fmt;

/// A two-bit symbol (dibit) from C4FM demodulation.
///
/// Values are constrained to 0..=3 (2 bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Dibit(u8);

impl Dibit {
    /// Create a new dibit from the two least significant bits of the input.
    pub const fn new(value: u8) -> Self {
        Self(value & 0x03)
    }

    /// Return the 2-bit value as a u8.
    pub const fn bits(self) -> u8 {
        self.0
    }
}

impl fmt::Display for Dibit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02b}", self.0)
    }
}

/// A six-bit symbol (hexbit) used in Reed-Solomon coding.
///
/// Values are constrained to 0..=63 (6 bits). Reed-Solomon codes in P25
/// operate over GF(2^6), where each symbol is a hexbit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Hexbit(u8);

impl Hexbit {
    /// Create a new hexbit from the six least significant bits of the input.
    pub const fn new(value: u8) -> Self {
        Self(value & 0x3F)
    }

    /// Return the 6-bit value as a u8.
    pub const fn bits(self) -> u8 {
        self.0
    }
}

impl fmt::Display for Hexbit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0o{:02o}", self.0)
    }
}

/// A frequency in hertz.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Frequency(u64);

impl Frequency {
    /// Create a frequency from a value in hertz.
    pub fn from_hz(hz: u64) -> Self {
        Self(hz)
    }

    /// Return the frequency in hertz.
    pub fn hz(self) -> u64 {
        self.0
    }

    /// Return the frequency in megahertz.
    pub fn mhz(self) -> f64 {
        self.0 as f64 / 1_000_000.0
    }
}

impl fmt::Display for Frequency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.5} MHz", self.mhz())
    }
}

/// A P25 talkgroup identifier (16 bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct TalkgroupId(u16);

impl TalkgroupId {
    /// Create a new talkgroup identifier.
    pub fn new(id: u16) -> Self {
        Self(id)
    }

    /// Return the raw talkgroup ID value.
    pub fn value(self) -> u16 {
        self.0
    }
}

impl fmt::Display for TalkgroupId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A P25 Network Access Code (12 bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Nac(u16);

impl Nac {
    /// Create a NAC from the 12 least significant bits of the input.
    pub fn new(value: u16) -> Self {
        Self(value & 0x0FFF)
    }

    /// Return the raw NAC value.
    pub fn value(self) -> u16 {
        self.0
    }
}

impl fmt::Display for Nac {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:03X}", self.0)
    }
}

/// A P25 unit (subscriber radio) identifier (24 bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SourceId(u32);

impl SourceId {
    /// Create a source ID from the 24 least significant bits of the input.
    pub fn new(id: u32) -> Self {
        Self(id & 0x00FF_FFFF)
    }

    /// Return the raw source ID value.
    pub fn value(self) -> u32 {
        self.0
    }
}

impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A P25 system identifier (12 bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct SystemId(u16);

impl SystemId {
    /// Create a system ID from the 12 least significant bits of the input.
    pub fn new(id: u16) -> Self {
        Self(id & 0x0FFF)
    }

    /// Return the raw system ID value.
    pub fn value(self) -> u16 {
        self.0
    }
}

impl fmt::Display for SystemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:03X}", self.0)
    }
}

/// A P25 Wide Area Communication Network identifier (20 bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Wacn(u32);

impl Wacn {
    /// Create a WACN from the 20 least significant bits of the input.
    pub fn new(id: u32) -> Self {
        Self(id & 0x000F_FFFF)
    }

    /// Return the raw WACN value.
    pub fn value(self) -> u32 {
        self.0
    }
}

impl fmt::Display for Wacn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:05X}", self.0)
    }
}

/// An RF subsystem identifier (8 bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct RfssId(u8);

impl RfssId {
    /// Create a new RFSS identifier.
    pub fn new(id: u8) -> Self {
        Self(id)
    }

    /// Return the raw RFSS ID value.
    pub fn value(self) -> u8 {
        self.0
    }
}

impl fmt::Display for RfssId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A site identifier (8 bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct SiteId(u8);

impl SiteId {
    /// Create a new site identifier.
    pub fn new(id: u8) -> Self {
        Self(id)
    }

    /// Return the raw site ID value.
    pub fn value(self) -> u8 {
        self.0
    }
}

impl fmt::Display for SiteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A 16-bit channel number used in TSBK channel grants and identifier tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ChannelNumber(u16);

impl ChannelNumber {
    /// Create a new channel number.
    pub fn new(value: u16) -> Self {
        Self(value)
    }

    /// Return the raw channel number value.
    pub fn value(self) -> u16 {
        self.0
    }

    /// Extract the 4-bit identifier prefix.
    pub fn identifier(self) -> u8 {
        ((self.0 >> 12) & 0x0F) as u8
    }

    /// Extract the 12-bit channel index within the identifier band.
    pub fn index(self) -> u16 {
        self.0 & 0x0FFF
    }
}

impl fmt::Display for ChannelNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:04X}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hexbit_masks_to_six_bits() {
        assert_eq!(Hexbit::new(0xFF).bits(), 0x3F);
        assert_eq!(Hexbit::new(0).bits(), 0);
        assert_eq!(Hexbit::new(63).bits(), 63);
    }

    #[test]
    fn hexbit_display() {
        assert_eq!(format!("{}", Hexbit::new(0o77)), "0o77");
        assert_eq!(format!("{}", Hexbit::new(0)), "0o00");
    }

    #[test]
    fn hexbit_default_is_zero() {
        assert_eq!(Hexbit::default().bits(), 0);
    }

    #[test]
    fn dibit_masks_to_two_bits() {
        assert_eq!(Dibit::new(0xFF).bits(), 0x03);
        assert_eq!(Dibit::new(0).bits(), 0);
        assert_eq!(Dibit::new(3).bits(), 3);
    }

    #[test]
    fn dibit_values_above_3_are_masked() {
        assert_eq!(Dibit::new(4).bits(), 0);
        assert_eq!(Dibit::new(5).bits(), 1);
        assert_eq!(Dibit::new(6).bits(), 2);
        assert_eq!(Dibit::new(7).bits(), 3);
    }

    #[test]
    fn dibit_display() {
        assert_eq!(format!("{}", Dibit::new(0)), "00");
        assert_eq!(format!("{}", Dibit::new(1)), "01");
        assert_eq!(format!("{}", Dibit::new(2)), "10");
        assert_eq!(format!("{}", Dibit::new(3)), "11");
    }

    #[test]
    fn frequency_conversions() {
        let freq = Frequency::from_hz(852_350_000);
        assert_eq!(freq.hz(), 852_350_000);
        assert!((freq.mhz() - 852.35).abs() < 1e-6);
    }

    #[test]
    fn frequency_display() {
        let freq = Frequency::from_hz(852_350_000);
        assert_eq!(format!("{}", freq), "852.35000 MHz");
    }

    #[test]
    fn talkgroup_id_roundtrip() {
        let tg = TalkgroupId::new(12345);
        assert_eq!(tg.value(), 12345);
        assert_eq!(format!("{}", tg), "12345");
    }

    #[test]
    fn nac_masks_to_12_bits() {
        assert_eq!(Nac::new(0xFFFF).value(), 0x0FFF);
        assert_eq!(Nac::new(0x293).value(), 0x293);
    }

    #[test]
    fn nac_display() {
        assert_eq!(format!("{}", Nac::new(0x293)), "0x293");
    }

    #[test]
    fn source_id_masks_to_24_bits() {
        assert_eq!(SourceId::new(0xFFFF_FFFF).value(), 0x00FF_FFFF);
        assert_eq!(SourceId::new(123456).value(), 123456);
    }

    #[test]
    fn source_id_display() {
        assert_eq!(format!("{}", SourceId::new(123456)), "123456");
    }

    #[test]
    fn system_id_masks_to_12_bits() {
        assert_eq!(SystemId::new(0xFFFF).value(), 0x0FFF);
        assert_eq!(format!("{}", SystemId::new(0x2B9)), "0x2B9");
    }

    #[test]
    fn wacn_masks_to_20_bits() {
        assert_eq!(Wacn::new(0xFFFF_FFFF).value(), 0x000F_FFFF);
        assert_eq!(format!("{}", Wacn::new(0xBEE00)), "0xBEE00");
    }

    #[test]
    fn rfss_id_roundtrip() {
        let rfss = RfssId::new(1);
        assert_eq!(rfss.value(), 1);
        assert_eq!(format!("{}", rfss), "1");
    }

    #[test]
    fn site_id_roundtrip() {
        let site = SiteId::new(42);
        assert_eq!(site.value(), 42);
        assert_eq!(format!("{}", site), "42");
    }

    #[test]
    fn channel_number_fields() {
        // Channel 0x1234: identifier=1, index=0x234
        let ch = ChannelNumber::new(0x1234);
        assert_eq!(ch.identifier(), 1);
        assert_eq!(ch.index(), 0x234);
        assert_eq!(ch.value(), 0x1234);
        assert_eq!(format!("{}", ch), "0x1234");
    }

    #[test]
    fn channel_number_identifier_edge_cases() {
        assert_eq!(ChannelNumber::new(0x0000).identifier(), 0);
        assert_eq!(ChannelNumber::new(0xFFFF).identifier(), 0x0F);
        assert_eq!(ChannelNumber::new(0xFFFF).index(), 0x0FFF);
    }
}
