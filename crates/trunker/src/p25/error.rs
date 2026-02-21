//! Error types for P25 protocol decoding.

/// Errors that can occur during P25 protocol decoding.
#[derive(Clone, Debug, thiserror::Error)]
pub enum P25Error {
    /// CRC check failed on a TSBK payload.
    #[error("CRC mismatch: expected 0x{expected:04X}, got 0x{actual:04X}")]
    CrcMismatch {
        /// Expected CRC value from the payload.
        expected: u16,
        /// Computed CRC value.
        actual: u16,
    },

    /// Trellis decoding failed.
    #[error("trellis decode error: {reason}")]
    TrellisDecode {
        /// Description of what went wrong.
        reason: String,
    },

    /// Frame sync not found within expected window.
    #[error("frame sync not found")]
    NoFrameSync,

    /// NID decoding failed (BCH error correction failed).
    #[error("NID decode error: {reason}")]
    NidDecode {
        /// Description of what went wrong.
        reason: String,
    },

    /// Payload too short to parse.
    #[error("payload too short: expected {expected} bytes, got {actual}")]
    PayloadTooShort {
        /// Minimum expected length in bytes.
        expected: usize,
        /// Actual length in bytes.
        actual: usize,
    },

    /// Unknown or unsupported data unit type.
    #[error("unknown data unit: DUID 0x{duid:X}")]
    UnknownDataUnit {
        /// The data unit identifier value.
        duid: u8,
    },

    /// Golay standard (23,12,7) decoding failed on a voice frame chunk.
    #[error("Golay decode failed on voice chunk u_{chunk}")]
    VoiceGolayDecode {
        /// Which chunk (0..=3) failed.
        chunk: usize,
    },

    /// Hamming standard (15,11,3) decoding failed on a voice frame chunk.
    #[error("Hamming decode failed on voice chunk u_{chunk}")]
    VoiceHammingDecode {
        /// Which chunk (4..=6) failed.
        chunk: usize,
    },

    /// Reed-Solomon short (24,12,13) decode failed (link control).
    #[error("Reed-Solomon short decode failed: unrecoverable errors in link control")]
    RsShortUnrecoverable,

    /// Reed-Solomon medium (24,16,9) decode failed (crypto control).
    #[error("Reed-Solomon medium decode failed: unrecoverable errors in crypto control")]
    RsMediumUnrecoverable,

    /// Reed-Solomon long (36,20,17) decode failed (voice header).
    #[error("Reed-Solomon long decode failed: unrecoverable errors in voice header")]
    RsLongUnrecoverable,

    /// Cyclic (16,8,5) decode failed (low-speed data fragment).
    #[error("cyclic decode failed: unrecoverable errors in low-speed data")]
    CyclicUnrecoverable,

    /// NID rejected by integrity policy (parity or BCH check failed).
    #[error("NID rejected: {reason} (NAC=0x{nac:03X}, DUID=0x{duid:X})")]
    NidRejected {
        /// Why the NID was rejected.
        reason: &'static str,
        /// The NAC from the rejected NID.
        nac: u16,
        /// The DUID from the rejected NID.
        duid: u8,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_mismatch_display() {
        let err = P25Error::CrcMismatch {
            expected: 0x1A3F,
            actual: 0x0000,
        };
        assert_eq!(
            format!("{err}"),
            "CRC mismatch: expected 0x1A3F, got 0x0000"
        );
    }

    #[test]
    fn payload_too_short_display() {
        let err = P25Error::PayloadTooShort {
            expected: 12,
            actual: 8,
        };
        assert_eq!(
            format!("{err}"),
            "payload too short: expected 12 bytes, got 8"
        );
    }

    #[test]
    fn trellis_decode_display() {
        let err = P25Error::TrellisDecode {
            reason: "invalid constellation point".to_string(),
        };
        assert_eq!(
            format!("{err}"),
            "trellis decode error: invalid constellation point"
        );
    }

    #[test]
    fn no_frame_sync_display() {
        let err = P25Error::NoFrameSync;
        assert_eq!(format!("{err}"), "frame sync not found");
    }

    #[test]
    fn unknown_data_unit_display() {
        let err = P25Error::UnknownDataUnit { duid: 0x0F };
        assert_eq!(format!("{err}"), "unknown data unit: DUID 0xF");
    }

    #[test]
    fn nid_rejected_display() {
        let err = P25Error::NidRejected {
            reason: "parity check failed",
            nac: 0x5FC,
            duid: 0x7,
        };
        assert_eq!(
            format!("{err}"),
            "NID rejected: parity check failed (NAC=0x5FC, DUID=0x7)"
        );
    }
}
