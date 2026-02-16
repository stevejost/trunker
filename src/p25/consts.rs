//! P25 protocol constants from TIA-102.BAAA and TIA-102.AABF.

/// Symbol rate in symbols per second.
pub const SYMBOL_RATE: u32 = 4800;

/// Number of symbols in the frame sync pattern.
pub const SYNC_SYMBOLS: usize = 24;

/// Number of dibits in the Network Identifier (NID).
pub const NID_DIBITS: usize = 32;

/// Number of coded dibits in a TSBK data unit (before trellis decode).
pub const CODING_DIBITS: usize = 98;

/// Number of dibits in a decoded TSBK payload.
pub const TSBK_DIBITS: usize = 48;

/// Number of bytes in a decoded TSBK payload (96 bits).
pub const TSBK_BYTES: usize = 12;

/// Interval between status symbols in dibits.
pub const DIBITS_PER_STATUS_UPDATE: usize = 36;

/// Frame sync pattern for HDU/TDU/LDU/TSDU data units (48 bits).
pub const FRAME_SYNC: u64 = 0x5575_F5FF_77FF;

/// Number of bits in the frame sync pattern.
pub const FRAME_SYNC_BITS: usize = 48;

/// P25 CRC-16 polynomial (CRC-CCITT).
pub const CRC_POLYNOMIAL: u16 = 0x1021;

/// P25 CRC-16 final XOR mask.
pub const CRC_FINAL_XOR: u16 = 0xFFFF;

/// Outer C4FM deviation in Hz.
pub const DEVIATION_OUTER_HZ: f32 = 1800.0;

/// Inner C4FM deviation in Hz.
pub const DEVIATION_INNER_HZ: f32 = 600.0;

/// P25 channel bandwidth in Hz.
pub const CHANNEL_BANDWIDTH_HZ: u32 = 12_500;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_rate_is_4800() {
        assert_eq!(SYMBOL_RATE, 4800);
    }

    #[test]
    fn tsbk_bytes_matches_dibits() {
        // 48 dibits = 96 bits = 12 bytes
        assert_eq!(TSBK_DIBITS * 2 / 8, TSBK_BYTES);
    }

    #[test]
    fn frame_sync_fits_in_48_bits() {
        assert!(FRAME_SYNC < (1u64 << FRAME_SYNC_BITS));
    }
}
