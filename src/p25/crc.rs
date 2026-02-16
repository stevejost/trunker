//! CRC-16 calculator for P25 TSBK payloads.
//!
//! Uses the CRC-CCITT polynomial (0x1021) with initial value 0
//! and final XOR of 0xFFFF, as specified in TIA-102.AABF.

use crate::p25::consts::{CRC_FINAL_XOR, CRC_POLYNOMIAL};

/// Compute the P25 CRC-16 over the given data bytes.
///
/// Uses polynomial 0x1021, initial value 0, final XOR 0xFFFF.
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;

    for &byte in data {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            if crc & 0x8000 != 0 {
                crc = (crc << 1) ^ CRC_POLYNOMIAL;
            } else {
                crc <<= 1;
            }
        }
    }

    crc ^ CRC_FINAL_XOR
}

/// Verify a TSBK's CRC-16.
///
/// Computes CRC over the first 10 bytes and compares with
/// the stored CRC in bytes 10-11.
pub fn verify_tsbk_crc(data: &[u8; 12]) -> bool {
    let computed = crc16(&data[..10]);
    let stored = u16::from_be_bytes([data[10], data[11]]);
    computed == stored
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc16_empty_input() {
        // With init=0 and no data, final XOR gives 0xFFFF
        assert_eq!(crc16(&[]), 0xFFFF);
    }

    #[test]
    fn crc16_single_byte() {
        // Known CRC-CCITT value for single byte 0x00
        let result = crc16(&[0x00]);
        // With poly 0x1021, init 0, final XOR 0xFFFF:
        // Processing 0x00: crc stays 0 through all 8 shifts
        // Final XOR: 0x0000 ^ 0xFFFF = 0xFFFF
        assert_eq!(result, 0xFFFF);
    }

    #[test]
    fn crc16_known_vector_from_p25_reference() {
        // Test vector from p25.rs: TsbkFields with known bytes
        // bytes[0..10] = [0xB9, 0x01, 0xF0, 0x0F, 0xAA, 0x55, 0x00, 0xFF, 0xCC, 0x33]
        // Expected calc_crc from p25.rs test: 0b0111010000111100 = 0x703C
        let data = [0xB9, 0x01, 0xF0, 0x0F, 0xAA, 0x55, 0x00, 0xFF, 0xCC, 0x33];
        assert_eq!(crc16(&data), 0x743C);
    }

    #[test]
    fn crc16_another_known_vector() {
        // Adjacent site test vector from p25.rs
        let data = [0x3C, 0x00, 0xCC, 0xDF, 0x3C, 0xAA, 0x55, 0x36, 0x7E, 0x51];
        let crc = crc16(&data);
        // Build a full TSBK with this CRC and verify
        let mut tsbk = [0u8; 12];
        tsbk[..10].copy_from_slice(&data);
        tsbk[10] = (crc >> 8) as u8;
        tsbk[11] = crc as u8;
        assert!(verify_tsbk_crc(&tsbk));
    }

    #[test]
    fn verify_tsbk_crc_valid() {
        let data = [0x3C, 0x00, 0xCC, 0xDF, 0x3C, 0xAA, 0x55, 0x36, 0x7E, 0x51];
        let crc = crc16(&data);
        let mut tsbk = [0u8; 12];
        tsbk[..10].copy_from_slice(&data);
        tsbk[10] = (crc >> 8) as u8;
        tsbk[11] = crc as u8;
        assert!(verify_tsbk_crc(&tsbk));
    }

    #[test]
    fn verify_tsbk_crc_invalid() {
        let mut tsbk = [
            0xB9, 0x01, 0xF0, 0x0F, 0xAA, 0x55, 0x00, 0xFF, 0xCC, 0x33, 0xD7, 0xD7,
        ];
        // 0xD7D7 is not the correct CRC for this data
        assert!(!verify_tsbk_crc(&tsbk));

        // Fix the CRC
        let crc = crc16(&tsbk[..10]);
        tsbk[10] = (crc >> 8) as u8;
        tsbk[11] = crc as u8;
        assert!(verify_tsbk_crc(&tsbk));
    }

    #[test]
    fn crc16_deterministic() {
        let data = [0x01, 0x02, 0x03, 0x04, 0x05];
        let crc1 = crc16(&data);
        let crc2 = crc16(&data);
        assert_eq!(crc1, crc2);
    }
}
