//! Status symbol deinterleaving for P25 transmitted streams.
//!
//! P25 inserts a status symbol every 35 data symbols (one status period = 36 dibits).
//! The frame sync (24 symbols) counts toward the first status period, so the first
//! status symbol appears 12 symbols after sync.

use crate::p25::consts::{DIBITS_PER_STATUS_UPDATE, SYNC_SYMBOLS};
use crate::p25::types::Dibit;

/// Status symbol values transmitted by P25 repeaters and subscribers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusCode {
    /// Subscriber transmitting directly to another subscriber (0b00).
    SubscriberDirect,
    /// Inbound channel is busy (0b01).
    InboundBusy,
    /// Subscriber transmitting to a repeater (0b10).
    SubscriberRepeater,
    /// Inbound channel is idle (0b11).
    InboundIdle,
}

impl StatusCode {
    /// Parse a status code from a dibit value.
    pub fn from_dibit(dibit: Dibit) -> Self {
        match dibit.bits() {
            0b00 => Self::SubscriberDirect,
            0b01 => Self::InboundBusy,
            0b10 => Self::SubscriberRepeater,
            0b11 => Self::InboundIdle,
            _ => unreachable!("dibit is constrained to 2 bits"),
        }
    }

    /// Convert to the dibit representation.
    pub fn to_dibit(self) -> Dibit {
        Dibit::new(match self {
            Self::SubscriberDirect => 0b00,
            Self::InboundBusy => 0b01,
            Self::SubscriberRepeater => 0b10,
            Self::InboundIdle => 0b11,
        })
    }
}

/// A classified symbol from the P25 stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamSymbol {
    /// A status symbol (to be discarded for data decoding).
    Status(StatusCode),
    /// A data symbol (to be passed to decoders).
    Data(Dibit),
}

/// Separates status symbols from data symbols in a P25 stream.
///
/// Must be initialized immediately after frame sync detection.
/// The sync pattern's 24 symbols count toward the first status period.
#[derive(Debug, Clone, Copy)]
pub struct StatusDeinterleaver {
    /// Current position within the status period.
    position: usize,
}

impl Default for StatusDeinterleaver {
    fn default() -> Self {
        Self::new()
    }
}

impl StatusDeinterleaver {
    /// Create a new deinterleaver positioned after frame sync.
    pub fn new() -> Self {
        Self {
            position: SYNC_SYMBOLS,
        }
    }

    /// Classify the given dibit as a status or data symbol.
    pub fn feed(&mut self, dibit: Dibit) -> StreamSymbol {
        self.position += 1;
        self.position %= DIBITS_PER_STATUS_UPDATE;

        if self.position == 0 {
            StreamSymbol::Status(StatusCode::from_dibit(dibit))
        } else {
            StreamSymbol::Data(dibit)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_code_from_dibit_roundtrip() {
        for value in 0..=3u8 {
            let dibit = Dibit::new(value);
            let code = StatusCode::from_dibit(dibit);
            assert_eq!(code.to_dibit(), dibit);
        }
    }

    #[test]
    fn status_code_variant_mapping() {
        assert_eq!(
            StatusCode::from_dibit(Dibit::new(0b00)),
            StatusCode::SubscriberDirect
        );
        assert_eq!(
            StatusCode::from_dibit(Dibit::new(0b01)),
            StatusCode::InboundBusy
        );
        assert_eq!(
            StatusCode::from_dibit(Dibit::new(0b10)),
            StatusCode::SubscriberRepeater
        );
        assert_eq!(
            StatusCode::from_dibit(Dibit::new(0b11)),
            StatusCode::InboundIdle
        );
    }

    #[test]
    fn first_status_after_sync_at_position_12() {
        let mut deinterleaver = StatusDeinterleaver::new();

        // After sync (24 symbols), next 11 should be data
        for _ in 0..11 {
            let result = deinterleaver.feed(Dibit::new(0b00));
            assert_eq!(result, StreamSymbol::Data(Dibit::new(0b00)));
        }

        // 12th symbol should be status (24 + 12 = 36 = one full period)
        let result = deinterleaver.feed(Dibit::new(0b00));
        assert_eq!(result, StreamSymbol::Status(StatusCode::SubscriberDirect));
    }

    #[test]
    fn subsequent_status_every_36_symbols() {
        let mut deinterleaver = StatusDeinterleaver::new();

        // First 11 data + 1 status
        for _ in 0..11 {
            deinterleaver.feed(Dibit::new(0b00));
        }
        let result = deinterleaver.feed(Dibit::new(0b00));
        assert!(matches!(result, StreamSymbol::Status(_)));

        // Next 35 data + 1 status
        for _ in 0..35 {
            let result = deinterleaver.feed(Dibit::new(0b00));
            assert_eq!(result, StreamSymbol::Data(Dibit::new(0b00)));
        }
        let result = deinterleaver.feed(Dibit::new(0b00));
        assert!(matches!(result, StreamSymbol::Status(_)));

        // And again: 35 data + 1 status
        for _ in 0..35 {
            let result = deinterleaver.feed(Dibit::new(0b00));
            assert_eq!(result, StreamSymbol::Data(Dibit::new(0b00)));
        }
        let result = deinterleaver.feed(Dibit::new(0b00));
        assert!(matches!(result, StreamSymbol::Status(_)));
    }

    #[test]
    fn status_symbol_carries_correct_code() {
        let mut deinterleaver = StatusDeinterleaver::new();

        // Advance to first status position
        for _ in 0..11 {
            deinterleaver.feed(Dibit::new(0b00));
        }

        // Feed InboundBusy (0b01) at status position
        let result = deinterleaver.feed(Dibit::new(0b01));
        assert_eq!(result, StreamSymbol::Status(StatusCode::InboundBusy));

        // Advance to next status position
        for _ in 0..35 {
            deinterleaver.feed(Dibit::new(0b00));
        }

        // Feed InboundIdle (0b11) at status position
        let result = deinterleaver.feed(Dibit::new(0b11));
        assert_eq!(result, StreamSymbol::Status(StatusCode::InboundIdle));
    }

    #[test]
    fn data_symbols_preserve_dibit_value() {
        let mut deinterleaver = StatusDeinterleaver::new();

        // Feed different dibit values and verify they are preserved
        for value in 0..=3u8 {
            let result = deinterleaver.feed(Dibit::new(value));
            assert_eq!(result, StreamSymbol::Data(Dibit::new(value)));
        }
    }
}
