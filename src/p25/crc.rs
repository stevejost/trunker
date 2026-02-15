//! CRC-16 validation for P25 Trunking Signaling Blocks.
//!
//! P25 TSBKs use CRC-16/CCITT with polynomial x^16 + x^12 + x^5 + 1 (0x1021),
//! initial value 0, and final XOR of 0xFFFF. The CRC is computed MSB-first
//! over the first 80 bits (10 bytes) and compared against bytes 10-11.
//!
//! Reference: TIA-102.AABF-A Section 7.1

/// CRC-16 polynomial for P25 TSBK validation.
///
/// Polynomial: x^16 + x^12 + x^5 + 1 = 0x1021 (implicit x^16 term).
const CRC16_POLY: u32 = 0x1021;

/// Final XOR value applied after CRC computation.
const CRC16_FINAL_XOR: u32 = 0xFFFF;

/// Compute the CRC-16 over a byte buffer, processing bits MSB-first.
///
/// To validate a TSBK, compute `crc16(data[..10])` and compare with
/// the transmitted CRC in bytes 10-11.
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u32 = 0;
    for &byte in data {
        for j in 0..8 {
            let bit = ((byte >> (7 - j)) & 1) as u32;
            crc = ((crc << 1) | bit) & 0x1_FFFF;
            if crc & 0x1_0000 != 0 {
                crc = (crc & 0xFFFF) ^ CRC16_POLY;
            }
        }
    }
    crc ^= CRC16_FINAL_XOR;
    (crc & 0xFFFF) as u16
}

/// Validate that a 12-byte TSBK passes its CRC-16 check.
///
/// Computes the CRC-16 over the first 10 bytes (header + payload) and
/// compares it against the transmitted CRC in bytes 10-11.
///
/// Returns `Ok(())` if the CRC is valid, or `Err(CrcError)` with the
/// transmitted and computed CRC values for diagnostics.
pub fn validate_tsbk_crc(data: &[u8; 12]) -> Result<(), CrcError> {
    let transmitted = ((data[10] as u16) << 8) | data[11] as u16;
    let computed = crc16(&data[..10]);
    if computed != transmitted {
        return Err(CrcError::Mismatch {
            expected: transmitted,
            computed,
        });
    }
    Ok(())
}

/// CRC validation error.
#[derive(Debug, thiserror::Error)]
pub enum CrcError {
    /// The computed CRC does not match the transmitted CRC.
    #[error("CRC-16 mismatch: transmitted {expected:#06x}, computed {computed:#06x}")]
    Mismatch {
        /// CRC value from the TSBK (bytes 10-11).
        expected: u16,
        /// CRC computed over the first 10 bytes.
        computed: u16,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: compute CRC for first 10 bytes and append it to make a valid 12-byte TSBK.
    fn make_valid_tsbk(header_and_payload: &[u8; 10]) -> [u8; 12] {
        let mut data = [0u8; 12];
        data[..10].copy_from_slice(header_and_payload);
        let crc = crc16(&data[..10]);
        data[10] = (crc >> 8) as u8;
        data[11] = (crc & 0xFF) as u8;
        data
    }

    #[test]
    fn valid_tsbk_crc_matches_stored() {
        let data = make_valid_tsbk(&[0x80, 0x00, 0x00, 0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD]);
        let computed = crc16(&data[..10]);
        let stored = ((data[10] as u16) << 8) | data[11] as u16;
        assert_eq!(computed, stored);
    }

    #[test]
    fn validate_tsbk_crc_accepts_valid() {
        let data = make_valid_tsbk(&[0x80, 0x00, 0x00, 0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD]);
        assert!(validate_tsbk_crc(&data).is_ok());
    }

    #[test]
    fn validate_tsbk_crc_rejects_bit_flip() {
        let mut data =
            make_valid_tsbk(&[0x80, 0x00, 0x00, 0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD]);
        data[5] ^= 0x01; // flip one bit in payload
        assert!(validate_tsbk_crc(&data).is_err());
    }

    #[test]
    fn crc_error_contains_both_values() {
        let mut data =
            make_valid_tsbk(&[0x80, 0x00, 0x00, 0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD]);
        let original_transmitted = ((data[10] as u16) << 8) | data[11] as u16;
        data[5] ^= 0x01;
        match validate_tsbk_crc(&data) {
            Err(CrcError::Mismatch { expected, computed }) => {
                // Transmitted CRC bytes haven't changed
                assert_eq!(expected, original_transmitted);
                // Computed CRC over corrupted data should differ
                assert_ne!(computed, original_transmitted);
            }
            Ok(()) => panic!("expected CRC error"),
        }
    }

    #[test]
    fn different_payloads_produce_different_crcs() {
        let crc_a = crc16(&[0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        let crc_b = crc16(&[0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01]);
        assert_ne!(crc_a, crc_b);
    }
}
