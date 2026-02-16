#!/usr/bin/env python3
"""OP25 CQPSK reference decode of SRRCS control channel IQ recording.

This uses the same p25_demodulator and p25_frame_assembler as OP25's
main application, but with demod_type='cqpsk' instead of 'fsk4'.

Outputs:
  - TSBK count and CRC stats
  - Parsed TSBK messages (same format as decode_test.py)
  - Summary statistics for gold reference comparison
"""

import sys
import os
import time
import json
sys.path.insert(0, '/home/fuzz/source/op25/op25/gr-op25_repeater/apps')
sys.path.insert(0, '/home/fuzz/source/op25/op25/gr-op25_repeater/apps/tx')

from gnuradio import gr, blocks
from gnuradio import op25_repeater
import p25_demodulator

IQ_FILE = '/home/fuzz/source/trunker/samples/iq/srrcs_cc_852350000_2400k_cf32.iq'
SAMPLE_RATE = 2400000

# --- TSBK Parsing (matches OP25 tk_p25.py bit layout) ---

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

ident_table = {}


def resolve_channel(channel_id):
    ident = (channel_id >> 12) & 0xF
    channel_number = channel_id & 0xFFF
    if ident in ident_table:
        entry = ident_table[ident]
        freq = entry['base_freq'] + (entry['spacing'] * channel_number)
        return freq
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

    result = {
        'nac': f'0x{nac:03X}',
        'opcode': f'0x{opcode:02X}',
        'name': name,
    }

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
        offset_hz = toff * spac * 125
        base_freq_hz = freq * 5

        ident_table[ident] = {'base_freq': base_freq_hz, 'spacing': spacing_hz, 'offset': offset_hz}
        result.update({
            'ident': ident,
            'base_freq_mhz': fmt_freq(base_freq_hz),
            'spacing_khz': spacing_hz / 1000,
        })

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
        offset_hz = toff * spac * 125
        base_freq_hz = freq * 5

        ident_table[ident] = {'base_freq': base_freq_hz, 'spacing': spacing_hz, 'offset': offset_hz}
        result.update({
            'ident': ident,
            'base_freq_mhz': fmt_freq(base_freq_hz),
            'spacing_khz': spacing_hz / 1000,
        })

    elif opcode == 0x00:  # GRP_V_CH_GRANT
        if mfrid == 0x90:
            result['name'] = 'MOT_GRG_CMD'
        else:
            channel = (tsbk >> 56) & 0xFFFF
            talkgroup = (tsbk >> 40) & 0xFFFF
            source = (tsbk >> 16) & 0xFFFFFF
            freq = resolve_channel(channel)
            result.update({
                'channel': f'0x{channel:04X}',
                'freq_mhz': fmt_freq(freq),
                'talkgroup': talkgroup,
                'source': source,
            })

    elif opcode == 0x02:  # GRP_V_CH_GRANT_UPDT
        ch1 = (tsbk >> 64) & 0xFFFF
        tg1 = (tsbk >> 48) & 0xFFFF
        ch2 = (tsbk >> 32) & 0xFFFF
        tg2 = (tsbk >> 16) & 0xFFFF
        result.update({
            'ch1': f'0x{ch1:04X}', 'freq1_mhz': fmt_freq(resolve_channel(ch1)),
            'tg1': tg1,
            'ch2': f'0x{ch2:04X}', 'freq2_mhz': fmt_freq(resolve_channel(ch2)),
            'tg2': tg2,
        })

    elif opcode in (0x39, 0x3B):  # NET_STS_BCST
        wacn = (tsbk >> 52) & 0xFFFFF
        sysid = (tsbk >> 40) & 0xFFF
        channel = (tsbk >> 24) & 0xFFFF
        result.update({
            'wacn': f'0x{wacn:05X}',
            'sysid': f'0x{sysid:03X}',
            'channel': f'0x{channel:04X}',
            'freq_mhz': fmt_freq(resolve_channel(channel)),
        })

    elif opcode == 0x3A:  # RFSS_STS_BCST
        sysid = (tsbk >> 56) & 0xFFF
        rfss = (tsbk >> 48) & 0xFF
        site = (tsbk >> 40) & 0xFF
        channel = (tsbk >> 24) & 0xFFFF
        result.update({
            'sysid': f'0x{sysid:03X}',
            'rfss': rfss,
            'site': site,
            'channel': f'0x{channel:04X}',
            'freq_mhz': fmt_freq(resolve_channel(channel)),
        })

    return result


# --- Flowgraph ---

class p25_cqpsk_decoder(gr.top_block):
    def __init__(self):
        gr.top_block.__init__(self, "P25 CQPSK Decode Test")

        self.file_src = blocks.file_source(gr.sizeof_gr_complex, IQ_FILE, False)
        # No throttle -- run as fast as possible for batch processing

        self.demod = p25_demodulator.p25_demod_cb(
            msgq_id=0,
            debug=0,
            input_rate=SAMPLE_RATE,
            demod_type='cqpsk',       # <-- CQPSK mode
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

        self.connect(self.file_src, self.demod, self.decoder)


def main():
    tb = p25_cqpsk_decoder()
    print(f"CQPSK Decode: {IQ_FILE}")
    print(f"Sample rate: {SAMPLE_RATE}")
    print(f"Demod type: cqpsk")
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
                if empty_count > 200:  # 2 seconds with no messages = done
                    break
                time.sleep(0.01)

    except KeyboardInterrupt:
        pass

    # Print and save results BEFORE shutdown (OP25 websocket crash on exit)
    print()
    print("=" * 90)
    print(f"Demod type:          cqpsk")
    print(f"Total TSBKs decoded: {tsbk_count}")
    print(f"Ident table:         {len(ident_table)} entries")
    for ident, entry in sorted(ident_table.items()):
        print(f"  [{ident}] base={fmt_freq(entry['base_freq'])} MHz  "
              f"spacing={entry['spacing']/1000} kHz")
    print()
    print("Opcode distribution:")
    for op, count in sorted(opcode_counts.items()):
        name = OPCODE_NAMES.get(int(op, 16), op)
        print(f"  {op} {name:<28} {count:4d}")
    print()
    sys.stdout.flush()

    # Save JSON for downstream comparison
    summary = {
        'demod_type': 'cqpsk',
        'total_tsbks': tsbk_count,
        'opcode_counts': opcode_counts,
        'ident_table': {str(k): v for k, v in ident_table.items()},
    }
    with open('/home/fuzz/source/trunker/samples/op25_reference/cqpsk_summary.json', 'w') as f:
        json.dump(summary, f, indent=2)
    print("Saved summary to cqpsk_summary.json")

    with open('/home/fuzz/source/trunker/samples/op25_reference/cqpsk_tsbks.json', 'w') as f:
        json.dump(all_tsbks, f, indent=2)
    print(f"Saved {len(all_tsbks)} TSBKs to cqpsk_tsbks.json")
    sys.stdout.flush()

    # OP25 websocket shutdown crashes -- just exit hard
    os._exit(0)


if __name__ == '__main__':
    main()
