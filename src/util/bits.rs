//! Bit extraction helpers for protocol parsing.
//!
//! These functions provide ergonomic access to bit fields within P25 protocol
//! data using [`bitvec`] with MSB-first ordering, which matches P25's
//! over-the-air bit ordering.

use bitvec::field::BitField;
use bitvec::order::Msb0;
use bitvec::slice::BitSlice;
use std::ops::Range;

/// Extract a `u8` value from the given bit range.
///
/// The range is specified in bitvec indices where index 0 is the MSB of byte 0.
pub fn extract_u8(bits: &BitSlice<u8, Msb0>, range: Range<usize>) -> u8 {
    bits[range].load_be::<u8>()
}

/// Extract a `u16` value from the given bit range.
///
/// The range is specified in bitvec indices where index 0 is the MSB of byte 0.
pub fn extract_u16(bits: &BitSlice<u8, Msb0>, range: Range<usize>) -> u16 {
    bits[range].load_be::<u16>()
}

/// Extract a `u32` value from the given bit range (up to 32 bits).
///
/// The range is specified in bitvec indices where index 0 is the MSB of byte 0.
pub fn extract_u32(bits: &BitSlice<u8, Msb0>, range: Range<usize>) -> u32 {
    bits[range].load_be::<u32>()
}

/// Extract a `u64` value from the given bit range (up to 64 bits).
///
/// The range is specified in bitvec indices where index 0 is the MSB of byte 0.
pub fn extract_u64(bits: &BitSlice<u8, Msb0>, range: Range<usize>) -> u64 {
    bits[range].load_be::<u64>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitvec::view::BitView;

    #[test]
    fn extract_byte_aligned_u8() {
        let data: [u8; 2] = [0xAB, 0xCD];
        let bits = data.view_bits::<Msb0>();
        assert_eq!(extract_u8(bits, 0..8), 0xAB);
        assert_eq!(extract_u8(bits, 8..16), 0xCD);
    }

    #[test]
    fn extract_non_aligned_u8() {
        // 0xAB = 1010_1011, 0xCD = 1100_1101
        let data: [u8; 2] = [0xAB, 0xCD];
        let bits = data.view_bits::<Msb0>();
        // Bits 4..12 = 1011_1100 = 0xBC
        assert_eq!(extract_u8(bits, 4..12), 0xBC);
    }

    #[test]
    fn extract_u16_across_bytes() {
        let data: [u8; 4] = [0x12, 0x34, 0x56, 0x78];
        let bits = data.view_bits::<Msb0>();
        assert_eq!(extract_u16(bits, 0..16), 0x1234);
        assert_eq!(extract_u16(bits, 16..32), 0x5678);
    }

    #[test]
    fn extract_u32_full() {
        let data: [u8; 4] = [0xDE, 0xAD, 0xBE, 0xEF];
        let bits = data.view_bits::<Msb0>();
        assert_eq!(extract_u32(bits, 0..32), 0xDEADBEEF);
    }

    #[test]
    fn extract_24_bit_into_u32() {
        let data: [u8; 4] = [0x00, 0x12, 0x34, 0x56];
        let bits = data.view_bits::<Msb0>();
        // 24 bits from position 8..32 = 0x123456
        assert_eq!(extract_u32(bits, 8..32), 0x123456);
    }

    #[test]
    fn extract_4_bit_nibble() {
        let data: [u8; 1] = [0xA5];
        let bits = data.view_bits::<Msb0>();
        assert_eq!(extract_u8(bits, 0..4), 0xA);
        assert_eq!(extract_u8(bits, 4..8), 0x5);
    }
}
