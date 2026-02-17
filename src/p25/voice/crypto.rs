//! Crypto Control fields from LDU2 voice frames.
//!
//! The Crypto Control payload (12 bytes) carries the information needed
//! to decrypt an encrypted voice transmission: initialization vector,
//! algorithm identifier, and encryption key ID.

use serde::Serialize;
use std::fmt;

/// Number of bytes in a Crypto Control payload.
pub const CRYPTO_CONTROL_BYTES: usize = 12;

/// Crypto Control fields decoded from an LDU2 frame.
///
/// Layout (12 bytes):
/// - Bytes 0..9: Crypto initialization vector (72 bits)
/// - Byte 9: Algorithm ID (ALGID)
/// - Bytes 10..12: Key ID (16 bits)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CryptoControlFields([u8; CRYPTO_CONTROL_BYTES]);

impl CryptoControlFields {
    /// Create from a 12-byte buffer.
    pub fn new(buf: [u8; CRYPTO_CONTROL_BYTES]) -> Self {
        Self(buf)
    }

    /// The 9-byte initialization vector for the crypto algorithm.
    pub fn initialization_vector(&self) -> &[u8] {
        &self.0[..9]
    }

    /// The crypto algorithm in use.
    pub fn algorithm(&self) -> CryptoAlgorithm {
        CryptoAlgorithm::from_byte(self.0[9])
    }

    /// The 16-bit encryption key identifier.
    pub fn key_id(&self) -> u16 {
        u16::from_be_bytes([self.0[10], self.0[11]])
    }
}

impl fmt::Display for CryptoControlFields {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "algorithm={}, key_id=0x{:04X}",
            self.algorithm(),
            self.key_id()
        )
    }
}

/// Cryptographic algorithm identifier (ALGID).
///
/// Values from TIA-102.AABF.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum CryptoAlgorithm {
    /// Type 1: Accordion (0x00).
    Accordion,
    /// Type 1: Baton Even (0x01).
    BatonEven,
    /// Type 1: Firefly (0x02).
    Firefly,
    /// Type 1: Mayfly (0x03).
    Mayfly,
    /// Type 1: Saville (0x04).
    Saville,
    /// Type 1: Baton Odd (0x41).
    BatonOdd,
    /// No encryption (0x80).
    Unencrypted,
    /// Type 3: DES-OFB (0x81).
    Des,
    /// Type 3: Triple DES (0x83).
    TripleDes,
    /// Type 3: AES-256 (0x84).
    Aes,
    /// Unknown or vendor-specific algorithm.
    Unknown(u8),
}

impl CryptoAlgorithm {
    /// Parse a crypto algorithm from its 8-bit identifier.
    pub fn from_byte(byte: u8) -> Self {
        match byte {
            0x00 => Self::Accordion,
            0x01 => Self::BatonEven,
            0x02 => Self::Firefly,
            0x03 => Self::Mayfly,
            0x04 => Self::Saville,
            0x41 => Self::BatonOdd,
            0x80 => Self::Unencrypted,
            0x81 => Self::Des,
            0x83 => Self::TripleDes,
            0x84 => Self::Aes,
            b => Self::Unknown(b),
        }
    }
}

impl fmt::Display for CryptoAlgorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Accordion => write!(f, "Accordion"),
            Self::BatonEven => write!(f, "Baton Even"),
            Self::Firefly => write!(f, "Firefly"),
            Self::Mayfly => write!(f, "Mayfly"),
            Self::Saville => write!(f, "Saville"),
            Self::BatonOdd => write!(f, "Baton Odd"),
            Self::Unencrypted => write!(f, "Unencrypted"),
            Self::Des => write!(f, "DES-OFB"),
            Self::TripleDes => write!(f, "3DES"),
            Self::Aes => write!(f, "AES-256"),
            Self::Unknown(b) => write!(f, "Unknown(0x{b:02X})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crypto_control_fields_parsing() {
        let cc = CryptoControlFields::new([
            0, 0, 0, 1, 0, 0, 0, 2, 0,
            0x84,
            0xDE, 0xAD,
        ]);

        assert_eq!(cc.initialization_vector(), &[0, 0, 0, 1, 0, 0, 0, 2, 0]);
        assert_eq!(cc.algorithm(), CryptoAlgorithm::Aes);
        assert_eq!(cc.key_id(), 0xDEAD);
    }

    #[test]
    fn crypto_control_unencrypted() {
        let cc = CryptoControlFields::new([0; 12]);
        assert_eq!(cc.algorithm(), CryptoAlgorithm::Accordion);
        assert_eq!(cc.key_id(), 0);

        let mut buf = [0u8; 12];
        buf[9] = 0x80;
        let cc = CryptoControlFields::new(buf);
        assert_eq!(cc.algorithm(), CryptoAlgorithm::Unencrypted);
    }

    #[test]
    fn crypto_algorithm_known_values() {
        assert_eq!(CryptoAlgorithm::from_byte(0x00), CryptoAlgorithm::Accordion);
        assert_eq!(CryptoAlgorithm::from_byte(0x01), CryptoAlgorithm::BatonEven);
        assert_eq!(CryptoAlgorithm::from_byte(0x02), CryptoAlgorithm::Firefly);
        assert_eq!(CryptoAlgorithm::from_byte(0x03), CryptoAlgorithm::Mayfly);
        assert_eq!(CryptoAlgorithm::from_byte(0x04), CryptoAlgorithm::Saville);
        assert_eq!(CryptoAlgorithm::from_byte(0x41), CryptoAlgorithm::BatonOdd);
        assert_eq!(CryptoAlgorithm::from_byte(0x80), CryptoAlgorithm::Unencrypted);
        assert_eq!(CryptoAlgorithm::from_byte(0x81), CryptoAlgorithm::Des);
        assert_eq!(CryptoAlgorithm::from_byte(0x83), CryptoAlgorithm::TripleDes);
        assert_eq!(CryptoAlgorithm::from_byte(0x84), CryptoAlgorithm::Aes);
    }

    #[test]
    fn crypto_algorithm_unknown() {
        assert_eq!(CryptoAlgorithm::from_byte(0xFF), CryptoAlgorithm::Unknown(0xFF));
        assert_eq!(CryptoAlgorithm::from_byte(0x42), CryptoAlgorithm::Unknown(0x42));
    }

    #[test]
    fn crypto_control_display() {
        let cc = CryptoControlFields::new([
            0, 0, 0, 0, 0, 0, 0, 0, 0,
            0x80,
            0x00, 0x01,
        ]);
        assert_eq!(format!("{cc}"), "algorithm=Unencrypted, key_id=0x0001");
    }
}
