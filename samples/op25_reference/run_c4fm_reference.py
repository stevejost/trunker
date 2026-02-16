#!/usr/bin/env python3
"""OP25 C4FM (fsk4) reference decode -- batch mode, no throttle.

Runs as fast as possible through the IQ file, collects all TSBKs,
then prints summary statistics. Used for gold reference generation.

Note: OP25's p25_frame_assembler has a websocket shutdown bug that
causes a crash on exit. We save results before stopping to work around it.
"""

import sys
import os
import time
import json
import signal
sys.path.insert(0, '/home/fuzz/source/op25/op25/gr-op25_repeater/apps')
sys.path.insert(0, '/home/fuzz/source/op25/op25/gr-op25_repeater/apps/tx')

from gnuradio import gr, blocks
from gnuradio import op25_repeater
import p25_demodulator

IQ_FILE = '/home/fuzz/source/trunker/samples/iq/srrcs_cc_852350000_2400k_cf32.iq'
SAMPLE_RATE = 2400000
OUTPUT_DIR = '/home/fuzz/source/trunker/samples/op25_reference'

OPCODE_NAMES = {
    0x00: 'GRP_V_CH_GRANT', 0x02: 'GRP_V_CH_GRANT_UPDT',
    0x03: 'GRP_V_CH_GRANT_UPDT_EXP', 0x04: 'UNT_TO_UNT_CH_GRANT',
    0x05: 'UNT_TO_UNT_ANS_REQ', 0x09: 'EMERGENCY_ALRM',
    0x0B: 'UNKNOWN(0x0B)', 0x10: 'ACK_RSP', 0x16: 'DENY_RSP',
    0x20: 'IDENT_UP', 0x28: 'GRP_AFF_RSP', 0x2C: 'U_REG_RSP',
    0x30: 'PWR_CTRL_BCST', 0x33: 'IDEN_UP_TDMA', 0x34: 'IDEN_UP_VU',
    0x39: 'NET_STS_BCST', 0x3A: 'RFSS_STS_BCST', 0x3B: 'NET_STS_BCST',
    0x3C: 'ADJ_STS_BCST', 0x3D: 'IDEN_UP',
}

ident_table = {}

def resolve_channel(channel_id):
    ident = (channel_id >> 12) & 0xF
    channel_number = channel_id & 0xFFF
    if ident in ident_table:
        entry = ident_table[ident]
        return entry['base_freq'] + (entry['spacing'] * channel_number)
    return None

def fmt_freq(freq_hz):
    if freq_hz is None:
        return '?'
    return f"{freq_hz / 1e6:.5f}"

def parse_tsbk(data):
    if len(data) < 12:
        return None
    nac = (data[0] << 8) | data[1]
    t = int.from_bytes(data[2:12], 'big')
    tsbk = t << 16
    opcode = (tsbk >> 88) & 0x3F
    mfrid = (tsbk >> 80) & 0xFF
    name = OPCODE_NAMES.get(opcode, f'UNKNOWN(0x{opcode:02X})')
    result = {'nac': f'0x{nac:03X}', 'opcode': f'0x{opcode:02X}', 'name': name}

    if opcode == 0x34:  # IDEN_UP_VU
        ident = (tsbk >> 76) & 0xF
        toff0 = (tsbk >> 58) & 0x3FFF
        spac = (tsbk >> 48) & 0x3FF
        freq = (tsbk >> 16) & 0xFFFFFFFF
        toff_sign = (toff0 >> 13) & 1
        toff = toff0 & 0x1FFF
        if toff_sign == 0:
            toff = -toff
        spacing_hz = spac * 125
        base_freq_hz = freq * 5
        ident_table[ident] = {'base_freq': base_freq_hz, 'spacing': spacing_hz}
        result.update({'ident': ident, 'base_freq_mhz': fmt_freq(base_freq_hz), 'spacing_khz': spacing_hz / 1000})

    elif opcode == 0x33:  # IDEN_UP_TDMA
        ident = (tsbk >> 76) & 0xF
        toff0 = (tsbk >> 58) & 0x3FFF
        spac = (tsbk >> 48) & 0x3FF
        freq = (tsbk >> 16) & 0xFFFFFFFF
        toff_sign = (toff0 >> 13) & 1
        toff = toff0 & 0x1FFF
        if toff_sign == 0:
            toff = -toff
        spacing_hz = spac * 125
        base_freq_hz = freq * 5
        ident_table[ident] = {'base_freq': base_freq_hz, 'spacing': spacing_hz}
        result.update({'ident': ident, 'base_freq_mhz': fmt_freq(base_freq_hz), 'spacing_khz': spacing_hz / 1000})

    elif opcode == 0x00:  # GRP_V_CH_GRANT
        if mfrid != 0x90:
            channel = (tsbk >> 56) & 0xFFFF
            talkgroup = (tsbk >> 40) & 0xFFFF
            source = (tsbk >> 16) & 0xFFFFFF
            result.update({'channel': f'0x{channel:04X}', 'freq_mhz': fmt_freq(resolve_channel(channel)),
                           'talkgroup': talkgroup, 'source': source})
        else:
            result['name'] = 'MOT_GRG_CMD'

    elif opcode == 0x02:  # GRP_V_CH_GRANT_UPDT
        ch1 = (tsbk >> 64) & 0xFFFF
        tg1 = (tsbk >> 48) & 0xFFFF
        ch2 = (tsbk >> 32) & 0xFFFF
        tg2 = (tsbk >> 16) & 0xFFFF
        result.update({'ch1': f'0x{ch1:04X}', 'tg1': tg1, 'ch2': f'0x{ch2:04X}', 'tg2': tg2})

    elif opcode in (0x39, 0x3B):  # NET_STS_BCST
        wacn = (tsbk >> 52) & 0xFFFFF
        sysid = (tsbk >> 40) & 0xFFF
        result.update({'wacn': f'0x{wacn:05X}', 'sysid': f'0x{sysid:03X}'})

    elif opcode == 0x3A:  # RFSS_STS_BCST
        sysid = (tsbk >> 56) & 0xFFF
        rfss = (tsbk >> 48) & 0xFF
        site = (tsbk >> 40) & 0xFF
        result.update({'sysid': f'0x{sysid:03X}', 'rfss': rfss, 'site': site})

    return result


class p25_c4fm_batch(gr.top_block):
    def __init__(self):
        gr.top_block.__init__(self, "P25 C4FM Batch Decode")

        self.file_src = blocks.file_source(gr.sizeof_gr_complex, IQ_FILE, False)

        self.demod = p25_demodulator.p25_demod_cb(
            msgq_id=0, debug=0,
            input_rate=SAMPLE_RATE,
            demod_type='fsk4',
            filter_type='rc',
            excess_bw=0.2,
            relative_freq=0, offset=0,
            if_rate=24000,
            gain_mu=0.025,
            costas_alpha=0.008,
            symbol_rate=4800,
        )

        self.msgq = gr.msg_queue(20)
        self.decoder = op25_repeater.p25_frame_assembler(
            '127.0.0.1', 0, 0, False, False, True, self.msgq, False, False, False,
        )

        self.connect(self.file_src, self.demod, self.decoder)


def save_results(tsbk_count, opcode_counts, all_tsbks, prefix='c4fm'):
    """Save results to JSON files."""
    summary = {
        'demod_type': prefix,
        'total_tsbks': tsbk_count,
        'opcode_counts': opcode_counts,
        'ident_table': {str(k): v for k, v in ident_table.items()},
    }
    summary_path = os.path.join(OUTPUT_DIR, f'{prefix}_summary.json')
    with open(summary_path, 'w') as f:
        json.dump(summary, f, indent=2)
    print(f"Saved summary to {summary_path}")

    tsbks_path = os.path.join(OUTPUT_DIR, f'{prefix}_tsbks.json')
    with open(tsbks_path, 'w') as f:
        json.dump(all_tsbks, f, indent=2)
    print(f"Saved {len(all_tsbks)} TSBKs to {tsbks_path}")


def main():
    tb = p25_c4fm_batch()
    print(f"C4FM (fsk4) Batch Decode: {IQ_FILE}")
    print(f"Sample rate: {SAMPLE_RATE}")
    print("=" * 90)

    tb.start()
    tsbk_count = 0
    opcode_counts = {}
    all_tsbks = []
    empty_count = 0

    try:
        while True:
            if not tb.msgq.empty_p():
                msg = tb.msgq.delete_head()
                if msg is None:
                    continue

                msg_type = msg.type()
                data = msg.to_string()
                empty_count = 0

                if msg_type == 7 and len(data) == 12:
                    tsbk_count += 1
                    parsed = parse_tsbk(data)
                    if parsed:
                        op = parsed['opcode']
                        opcode_counts[op] = opcode_counts.get(op, 0) + 1
                        all_tsbks.append(parsed)
                        fields = {k: v for k, v in parsed.items()
                                  if k not in ('nac', 'opcode', 'name')}
                        field_str = ' '.join(f'{k}={v}' for k, v in fields.items())
                        print(f"[{tsbk_count:4d}] {parsed['opcode']} "
                              f"{parsed['name']:<28} {field_str}")
            else:
                empty_count += 1
                if empty_count > 300:  # 3 seconds with no messages = done
                    break
                time.sleep(0.01)

    except KeyboardInterrupt:
        pass

    # Print and save results BEFORE calling tb.stop() (which crashes)
    print()
    print("=" * 90)
    print(f"Demod type:          fsk4 (C4FM)")
    print(f"Total TSBKs decoded: {tsbk_count}")
    print(f"Ident table:         {len(ident_table)} entries")
    for ident, entry in sorted(ident_table.items()):
        print(f"  [{ident}] base={fmt_freq(entry['base_freq'])} MHz  spacing={entry['spacing']/1000} kHz")
    print()
    print("Opcode distribution:")
    for op, count in sorted(opcode_counts.items()):
        name = OPCODE_NAMES.get(int(op, 16), op)
        print(f"  {op} {name:<28} {count:4d}")
    print()
    sys.stdout.flush()

    save_results(tsbk_count, opcode_counts, all_tsbks, 'c4fm')
    sys.stdout.flush()

    # OP25 websocket shutdown crashes -- just exit hard
    os._exit(0)


if __name__ == '__main__':
    main()
