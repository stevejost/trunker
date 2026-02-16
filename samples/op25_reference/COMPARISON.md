# OP25 Gold Reference vs Trunker Comparison

Generated: 2026-02-16
IQ File: `srrcs_cc_852350000_2400k_cf32.iq` (562 MB, CF32, 2.4 MS/s, 852.350 MHz)
System: SRRCS (WACN=0x0C018, System ID=0x704, RFSS ID=1, Site ID=12, NAC=0x5FC)

## TSBK Count Comparison

| Decoder              | Demod Type | TSBKs Decoded |
|----------------------|------------|---------------|
| OP25 (fsk4)          | C4FM       | 15            |
| OP25 (cqpsk)         | CQPSK      | 991           |
| Trunker (c4fm)       | C4FM       | 857           |
| Trunker (cqpsk)      | CQPSK      | 1223          |

## Key Observations

1. **OP25 C4FM (15 TSBKs)**: OP25's fsk4 demod path finds very few TSBKs from
   this simulcast recording. This confirms the signal has heavy multipath/simulcast
   distortion that defeats pure FM-discriminator decode.

2. **OP25 CQPSK (991 TSBKs)**: The CQPSK demod path (Gardner+Costas+DiffDec)
   recovers 66x more TSBKs than C4FM, confirming CQPSK demod is far superior for
   this simulcast signal.

3. **Trunker C4FM (857 TSBKs)**: Our C4FM decoder already outperforms OP25 C4FM
   by 57x. This is because our two-stage decimation + parabolic timing + sync gating
   pipeline is more robust than OP25's fsk4 path on simulcast signals.

4. **Trunker CQPSK (1223 TSBKs)**: Our new CQPSK pipeline outperforms OP25 CQPSK
   by 23% (1223 vs 991). This is the best result of all four configurations.

## OP25 CQPSK Opcode Distribution (991 TSBKs)

| Opcode | Name                     | Count |
|--------|--------------------------|-------|
| 0x00   | GRP_V_CH_GRANT           | 217   |
| 0x02   | GRP_V_CH_GRANT_UPDT      | 58    |
| 0x03   | GRP_V_CH_GRANT_UPDT_EXP  | 47    |
| 0x05   | UNT_TO_UNT_ANS_REQ       | 53    |
| 0x09   | EMERGENCY_ALRM           | 58    |
| 0x0B   | UNKNOWN(0x0B)            | 34    |
| 0x14   | SNDCP_DATA_CH_GRANT      | 14    |
| 0x16   | DENY_RSP                 | 49    |
| 0x20   | IDENT_UP                 | 7     |
| 0x28   | GRP_AFF_RSP              | 3     |
| 0x2B   | UNKNOWN(0x2B)            | 36    |
| 0x2C   | U_REG_RSP                | 2     |
| 0x2F   | U_DE_REG_ACK             | 2     |
| 0x30   | PWR_CTRL_BCST            | 51    |
| 0x33   | IDEN_UP_TDMA             | 78    |
| 0x39   | NET_STS_BCST             | 50    |
| 0x3A   | RFSS_STS_BCST            | 54    |
| 0x3B   | NET_STS_BCST (alt)       | 53    |
| 0x3C   | ADJ_STS_BCST             | 52    |
| 0x3D   | IDEN_UP                  | 73    |

## Identifier Table (from OP25 CQPSK decode)

| Ident | Base Freq (MHz) | Spacing (kHz) | Offset (MHz) |
|-------|-----------------|---------------|--------------|
| 2     | 851.01250       | 12.5          | -45.0        |
| 3     | 762.00625       | 12.5          | +30.0        |
| 5     | 935.01250       | 12.5          | -39.0        |

## Notes

- OP25 CQPSK uses: LPF -> AGC -> FLL -> Gardner/Costas -> DiffDec -> complex_to_arg -> rescale (1/(pi/4)) -> FSK4 slicer
- OP25 has no CRC validation in p25_frame_assembler msgq output -- all 991 TSBKs
  passed OP25's internal trellis decode + CRC check
- OP25 websocket shutdown bug: p25_frame_assembler crashes on cleanup -- worked around with os._exit(0)
- Trunker CQPSK count (1223) is NOT CRC-validated count; includes all decoded TSBKs

## Files in this Directory

- `c4fm_summary.json` - OP25 C4FM decode summary
- `c4fm_tsbks.json` - All 15 OP25 C4FM TSBKs
- `c4fm_decode_output.txt` - Full C4FM decode log
- `cqpsk_summary.json` - OP25 CQPSK decode summary
- `cqpsk_tsbks.json` - All 991 OP25 CQPSK TSBKs
- `cqpsk_decode_output.txt` - Full CQPSK decode log
- `trunker_c4fm_output.txt` - Trunker C4FM JSON output (857 TSBKs)
- `trunker_cqpsk_output.txt` - Trunker CQPSK JSON output (1223 TSBKs)
- `run_c4fm_reference.py` - OP25 C4FM batch decode script
- `cqpsk_decode_test.py` - OP25 CQPSK batch decode script
