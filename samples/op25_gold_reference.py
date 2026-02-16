#!/usr/bin/env python3
"""Generate OP25 gold reference output from the SRRCS IQ recording.

Runs OP25's P25 demodulator in both C4FM (fsk4) and CQPSK modes against
the SRRCS control channel recording. Captures TSBK counts, NACs, opcode
distributions, and optionally raw symbol streams.

Usage:
    python3 samples/op25_gold_reference.py [--mode fsk4|cqpsk|both] [--symbols]

Output:
    samples/op25_gold_fsk4.json     - C4FM decode results
    samples/op25_gold_cqpsk.json    - CQPSK decode results
    samples/op25_gold_fsk4.raw      - Raw symbol stream (C4FM, if --symbols)
    samples/op25_gold_cqpsk.raw     - Raw symbol stream (CQPSK, if --symbols)
"""

import argparse
import json
import os
import sys
import time

sys.path.insert(0, '/home/fuzz/source/op25/op25/gr-op25_repeater/apps')
sys.path.insert(0, '/home/fuzz/source/op25/op25/gr-op25_repeater/apps/tx')

from gnuradio import gr, blocks
from gnuradio import op25_repeater
import p25_demodulator

IQ_FILE = '/home/fuzz/source/trunker/samples/iq/srrcs_cc_852350000_2400k_cf32.iq'
SAMPLE_RATE = 2400000
OUTPUT_DIR = '/home/fuzz/source/trunker/samples'

OPCODE_NAMES = {
    0x00: 'GRP_V_CH_GRANT',
    0x02: 'GRP_V_CH_GRANT_UPDT',
    0x03: 'GRP_V_CH_GRANT_UPDT_EXP',
    0x04: 'UNT_TO_UNT_CH_GRANT',
    0x05: 'UNT_TO_UNT_ANS_REQ',
    0x06: 'UNT_TO_UNT_CH_GRANT_UPDT',
    0x08: 'TEL_INT_CH_GRANT',
    0x09: 'EMERGENCY_ALRM',
    0x10: 'ACK_RSP',
    0x14: 'SNDCP_DATA_CH_GRANT',
    0x15: 'SNDCP_RCN_REQ',
    0x16: 'DENY_RSP',
    0x18: 'GRP_AFF_QRY',
    0x1A: 'LOC_REG_RSP',
    0x20: 'IDENT_UP',
    0x24: 'EXT_FNCT_CMD',
    0x28: 'GRP_AFF_RSP',
    0x2A: 'GRP_AFF_QRY_RSP',
    0x2C: 'U_REG_RSP',
    0x2F: 'U_DE_REG_ACK',
    0x30: 'PWR_CTRL_BCST',
    0x33: 'IDEN_UP_TDMA',
    0x34: 'IDEN_UP_VU',
    0x39: 'NET_STS_BCST',
    0x3A: 'RFSS_STS_BCST',
    0x3B: 'NET_STS_BCST',
    0x3C: 'ADJ_STS_BCST',
    0x3D: 'IDEN_UP',
}


def parse_tsbk(data):
    """Parse a 12-byte OP25 TSBK message (2 NAC + 10 TSBK payload).

    Returns dict with nac, opcode, name, and opcode-specific fields.
    """
    if len(data) < 12:
        return None

    nac = (data[0] << 8) | data[1]
    t = int.from_bytes(data[2:12], 'big')
    tsbk = t << 16

    opcode = (tsbk >> 88) & 0x3F
    mfrid = (tsbk >> 80) & 0xFF
    name = OPCODE_NAMES.get(opcode, f'UNKNOWN(0x{opcode:02X})')

    return {
        'nac': nac,
        'opcode': opcode,
        'opcode_hex': f'0x{opcode:02X}',
        'name': name,
        'mfrid': mfrid,
    }


class GoldReferenceDecoder(gr.top_block):
    """OP25 flowgraph for gold reference generation."""

    def __init__(self, demod_type, raw_symbols_path=None):
        gr.top_block.__init__(self, f"OP25 Gold Reference ({demod_type})")

        self.file_src = blocks.file_source(gr.sizeof_gr_complex, IQ_FILE, False)
        self.throttle = blocks.throttle(gr.sizeof_gr_complex, SAMPLE_RATE)

        self.demod = p25_demodulator.p25_demod_cb(
            msgq_id=0,
            debug=0,
            input_rate=SAMPLE_RATE,
            demod_type=demod_type,
            filter_type='rc',
            excess_bw=0.2,
            relative_freq=0,
            offset=0,
            if_rate=24000,
            gain_mu=0.025,
            costas_alpha=0.008,
            symbol_rate=4800,
        )

        self.msgq = gr.msg_queue(20)
        self.decoder = op25_repeater.p25_frame_assembler(
            '127.0.0.1', 0, 0, False, False, True, self.msgq, False, False, False,
        )

        self.connect(self.file_src, self.throttle, self.demod, self.decoder)

        # Optionally dump raw symbols (slicer output) to file.
        if raw_symbols_path:
            self.raw_sink = blocks.file_sink(gr.sizeof_char, raw_symbols_path)
            # Connect from slicer output (same as decoder input).
            self.connect(self.demod, self.raw_sink)


def run_decode(demod_type, dump_symbols=False):
    """Run OP25 against the SRRCS recording and collect results."""
    raw_path = None
    if dump_symbols:
        raw_path = os.path.join(OUTPUT_DIR, f'op25_gold_{demod_type}.raw')

    print(f"\n{'='*70}")
    print(f"  OP25 Gold Reference: {demod_type.upper()} mode")
    print(f"  Input: {IQ_FILE}")
    print(f"  Sample rate: {SAMPLE_RATE}")
    if raw_path:
        print(f"  Raw symbols: {raw_path}")
    print(f"{'='*70}\n")

    tb = GoldReferenceDecoder(demod_type, raw_path)
    tb.start()

    tsbk_count = 0
    nac_counts = {}
    opcode_counts = {}
    talkgroups = set()
    sources = set()
    tsbk_list = []
    idle_count = 0

    try:
        while True:
            if not tb.msgq.empty_p():
                idle_count = 0
                msg = tb.msgq.delete_head()
                if msg is None:
                    continue

                msg_type = msg.type()
                data = msg.to_string()

                if msg_type == 7 and len(data) == 12:
                    tsbk_count += 1
                    parsed = parse_tsbk(data)
                    if parsed:
                        nac = parsed['nac']
                        nac_counts[nac] = nac_counts.get(nac, 0) + 1
                        op = parsed['opcode']
                        opcode_counts[op] = opcode_counts.get(op, 0) + 1
                        tsbk_list.append(parsed)

                        if tsbk_count % 50 == 0:
                            print(f"  [{tsbk_count:5d}] NAC=0x{nac:03X} "
                                  f"{parsed['opcode_hex']} {parsed['name']}")
            else:
                idle_count += 1
                time.sleep(0.01)
                # If we've been idle for 2 seconds, assume EOF.
                if idle_count > 200:
                    break
    except KeyboardInterrupt:
        print("\n  Interrupted by user")
    finally:
        tb.stop()
        tb.wait()

    # Build summary.
    summary = {
        'demod_type': demod_type,
        'input_file': os.path.basename(IQ_FILE),
        'sample_rate': SAMPLE_RATE,
        'total_tsbks': tsbk_count,
        'nac_distribution': {f'0x{k:03X}': v for k, v in sorted(nac_counts.items())},
        'opcode_distribution': {
            f'0x{k:02X} ({OPCODE_NAMES.get(k, "UNKNOWN")})': v
            for k, v in sorted(opcode_counts.items())
        },
        'unique_opcodes': len(opcode_counts),
    }

    if dump_symbols and raw_path and os.path.exists(raw_path):
        summary['raw_symbols_file'] = os.path.basename(raw_path)
        summary['raw_symbols_bytes'] = os.path.getsize(raw_path)

    # Print summary.
    print(f"\n  --- Results ({demod_type.upper()}) ---")
    print(f"  Total TSBKs:     {tsbk_count}")
    print(f"  NAC distribution:")
    for nac_hex, count in summary['nac_distribution'].items():
        print(f"    {nac_hex}: {count}")
    print(f"  Opcode distribution:")
    for op_name, count in sorted(summary['opcode_distribution'].items(),
                                  key=lambda x: -x[1]):
        print(f"    {op_name}: {count}")

    # Save JSON.
    json_path = os.path.join(OUTPUT_DIR, f'op25_gold_{demod_type}.json')
    with open(json_path, 'w') as f:
        json.dump(summary, f, indent=2)
    print(f"\n  Saved: {json_path}")

    return summary


def main():
    parser = argparse.ArgumentParser(
        description='Generate OP25 gold reference from SRRCS recording')
    parser.add_argument('--mode', choices=['fsk4', 'cqpsk', 'both'],
                        default='both',
                        help='Demodulation mode (default: both)')
    parser.add_argument('--symbols', action='store_true',
                        help='Dump raw symbol streams to .raw files')
    args = parser.parse_args()

    if not os.path.exists(IQ_FILE):
        print(f"ERROR: IQ file not found: {IQ_FILE}", file=sys.stderr)
        sys.exit(1)

    results = {}

    if args.mode in ('fsk4', 'both'):
        results['fsk4'] = run_decode('fsk4', dump_symbols=args.symbols)

    if args.mode in ('cqpsk', 'both'):
        results['cqpsk'] = run_decode('cqpsk', dump_symbols=args.symbols)

    if len(results) == 2:
        print(f"\n{'='*70}")
        print("  COMPARISON: C4FM vs CQPSK")
        print(f"{'='*70}")
        fsk4_count = results['fsk4']['total_tsbks']
        cqpsk_count = results['cqpsk']['total_tsbks']
        print(f"  C4FM  TSBKs: {fsk4_count}")
        print(f"  CQPSK TSBKs: {cqpsk_count}")
        if fsk4_count > 0:
            ratio = cqpsk_count / fsk4_count
            print(f"  Ratio (CQPSK/C4FM): {ratio:.3f}")

    print("\nDone.")


if __name__ == '__main__':
    main()
