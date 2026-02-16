#!/usr/bin/env python3
"""Compare OP25 CQPSK TSBK output against trunker CQPSK output.

Determines whether trunker's output is a superset of OP25's by:
1. Bag-based matching: count occurrences of each distinct signature,
   verify trunker has >= OP25 count for every signature.
2. Sequential verification: confirm the matching TSBKs appear in the
   same temporal order.

Field mapping differences:
  OP25                      Trunker
  ----                      -------
  sysid = "0x5F2"           system_id = "0x5F2"
  rfss = 1                  rfss_id = 1
  site = 12                 site_id = 12
  channel = "0x01B1"        channel = 433 (integer, different encoding)
  ch1/tg1/ch2/tg2           channel_a/talkgroup_a/channel_b/talkgroup_b
  0x33: ident/base_freq     0x33: UNKNOWN (raw data)
  0x3D: no fields           0x3D: identifier/base_frequency
  MOT_GRG_CMD (no fields)   GRP_V_CH_GRANT with mfrid=144
"""

import json
from collections import Counter


def load_op25_tsbks(path):
    with open(path) as f:
        return json.load(f)


def load_trunker_tsbks(path):
    tsbks = []
    with open(path) as f:
        for line in f:
            line = line.strip()
            if line.startswith('{'):
                tsbks.append(json.loads(line))
    return tsbks


def normalize_op25(tsbk):
    """Normalize an OP25 TSBK to a comparable signature tuple.

    Returns (opcode, key1, key2, ...) where keys are opcode-specific.
    For opcode-only messages, returns (opcode,).
    """
    op = tsbk.get('opcode', '?')
    name = tsbk.get('name', '')

    # 0x00: MOT_GRG_CMD (mfrid=0x90, Motorola group regrouping)
    if op == '0x00' and name == 'MOT_GRG_CMD':
        return (op, 'mot_grg')

    # 0x00: GRP_V_CH_GRANT (regular)
    if op == '0x00' and 'talkgroup' in tsbk:
        return (op, 'grant', tsbk['talkgroup'], tsbk['source'])

    # 0x02: GRP_V_CH_GRANT_UPDT
    if op == '0x02' and 'tg1' in tsbk:
        return (op, tsbk['tg1'], tsbk['tg2'])

    # 0x33: IDEN_UP_TDMA - use ident field for matching
    if op == '0x33' and 'ident' in tsbk:
        return (op, tsbk['ident'])

    # 0x34: IDEN_UP_VU
    if op == '0x34' and 'ident' in tsbk:
        return (op, tsbk['ident'])

    # 0x39: NET_STS_BCST
    if op == '0x39' and 'wacn' in tsbk:
        return (op, tsbk['wacn'], tsbk.get('sysid'))

    # 0x3A: RFSS_STS_BCST
    if op == '0x3A' and 'rfss' in tsbk:
        return (op, tsbk.get('sysid'), tsbk['rfss'], tsbk['site'])

    # 0x3B: NET_STS_BCST (alternate)
    if op == '0x3B' and 'wacn' in tsbk:
        return (op, tsbk['wacn'], tsbk.get('sysid'))

    # 0x3C: ADJ_STS_BCST
    if op == '0x3C' and 'rfss' in tsbk:
        return (op, tsbk.get('sysid'), tsbk['rfss'], tsbk['site'])

    # Everything else: opcode-only
    return (op,)


def normalize_trunker(tsbk):
    """Normalize a trunker TSBK to match OP25 signature format."""
    op = tsbk.get('opcode', '?')
    mfrid = tsbk.get('manufacturer_id', 0)

    # 0x00: with mfrid=144 (0x90) = Motorola -> MOT_GRG_CMD equivalent
    if op == '0x00' and mfrid == 144:
        return (op, 'mot_grg')

    # 0x00: regular GRP_V_CH_GRANT
    if op == '0x00' and 'talkgroup' in tsbk:
        return (op, 'grant', tsbk['talkgroup'], tsbk['source'])

    # 0x02: GRP_V_CH_GRANT_UPDT
    if op == '0x02' and 'talkgroup_a' in tsbk:
        return (op, tsbk['talkgroup_a'], tsbk['talkgroup_b'])

    # 0x33: trunker doesn't parse this yet (shows as UNKNOWN with raw data)
    # Opcode-only matching
    if op == '0x33':
        return (op,)

    # 0x34: IDEN_UP_VU
    if op == '0x34' and 'identifier' in tsbk:
        return (op, tsbk['identifier'])

    # 0x39: NET_STS_BCST
    if op == '0x39' and 'wacn' in tsbk:
        return (op, tsbk['wacn'], tsbk.get('system_id'))

    # 0x3A: RFSS_STS_BCST
    if op == '0x3A' and 'rfss_id' in tsbk:
        return (op, tsbk.get('system_id'), tsbk['rfss_id'], tsbk['site_id'])

    # 0x3B: NET2_STS_BCST
    if op == '0x3B' and 'wacn' in tsbk:
        return (op, tsbk['wacn'], tsbk.get('system_id'))

    # 0x3C: ADJ_STS_BCST
    if op == '0x3C' and 'rfss_id' in tsbk:
        return (op, tsbk.get('system_id'), tsbk['rfss_id'], tsbk['site_id'])

    # 0x3D: IDEN_UP_TDMA (trunker parses, OP25 doesn't)
    if op == '0x3D':
        return (op,)

    return (op,)


def signatures_match(sig_a, sig_b):
    """Check if two normalized signatures match.

    Handles length differences: when one decoder parses fields and the
    other doesn't, we match on opcode only.
    """
    if sig_a[0] != sig_b[0]:
        return False

    # If either is opcode-only, match on opcode
    if len(sig_a) == 1 or len(sig_b) == 1:
        return True

    # Both have fields: exact match
    return sig_a == sig_b


def compare_bag(op25_tsbks, trunker_tsbks):
    """Bag-based (multiset) comparison.

    For each OP25 TSBK signature, check that trunker has at least as
    many occurrences. This is order-independent and handles the case
    where both decoders lock on at different points in the IQ file.
    """
    # Build signature lists
    op25_sigs = [normalize_op25(t) for t in op25_tsbks]
    trunker_sigs = [normalize_trunker(t) for t in trunker_tsbks]

    # For opcode-only signatures like ('0x33',), we need special handling
    # because OP25 has ('0x33', 3) and trunker has ('0x33',).
    # Strategy: reduce both to opcode-level for opcodes where one side
    # doesn't parse fields.

    # Find which opcodes have mixed specificity
    op25_has_fields = set()
    trunker_has_fields = set()
    for s in op25_sigs:
        if len(s) > 1:
            op25_has_fields.add(s[0])
    for s in trunker_sigs:
        if len(s) > 1:
            trunker_has_fields.add(s[0])

    # Opcodes where one side parses fields but the other doesn't
    opcode_only_opcodes = set()
    for opcode in (op25_has_fields | trunker_has_fields):
        op25_all_have = all(len(s) > 1 for s in op25_sigs if s[0] == opcode)
        trunker_all_have = all(len(s) > 1 for s in trunker_sigs if s[0] == opcode)
        if not (op25_all_have and trunker_all_have):
            opcode_only_opcodes.add(opcode)

    # Reduce to opcode-only for mixed opcodes
    def reduce_sig(sig):
        if sig[0] in opcode_only_opcodes:
            return (sig[0],)
        return sig

    op25_reduced = [reduce_sig(s) for s in op25_sigs]
    trunker_reduced = [reduce_sig(s) for s in trunker_sigs]

    op25_counts = Counter(op25_reduced)
    trunker_counts = Counter(trunker_reduced)

    return op25_counts, trunker_counts, op25_reduced, trunker_reduced


def compare_sequential(op25_sigs, trunker_sigs):
    """Sequential matching to verify temporal ordering.

    For each OP25 signature in order, find the next matching trunker
    signature starting from the last match position. Uses unlimited
    look-ahead since trunker may have many extra TSBKs.
    """
    trunker_idx = 0
    matched = 0
    unmatched_indices = []

    for i, op25_sig in enumerate(op25_sigs):
        found = False
        for j in range(trunker_idx, len(trunker_sigs)):
            if signatures_match(op25_sig, trunker_sigs[j]):
                matched += 1
                trunker_idx = j + 1
                found = True
                break

        if not found:
            unmatched_indices.append(i)

    return matched, unmatched_indices


def main():
    op25_path = '/home/fuzz/source/trunker/samples/op25_reference/cqpsk_tsbks.json'
    trunker_path = '/home/fuzz/source/trunker/samples/op25_reference/trunker_cqpsk_output.jsonl'

    op25_tsbks = load_op25_tsbks(op25_path)
    trunker_tsbks = load_trunker_tsbks(trunker_path)

    print(f"OP25 TSBKs:    {len(op25_tsbks)}")
    print(f"Trunker TSBKs: {len(trunker_tsbks)}")

    # === BAG-BASED COMPARISON ===
    print(f"\n{'='*80}")
    print("BAG-BASED COMPARISON (order-independent)")
    print(f"{'='*80}")

    op25_counts, trunker_counts, op25_reduced, trunker_reduced = \
        compare_bag(op25_tsbks, trunker_tsbks)

    all_sigs = sorted(set(list(op25_counts.keys()) + list(trunker_counts.keys())))

    covered = 0
    missed = 0
    missed_sigs = {}

    print(f"\n{'Signature':<55s} {'OP25':>6s} {'Trunker':>8s} {'Status':>8s}")
    print("-" * 80)

    for sig in all_sigs:
        op25_n = op25_counts.get(sig, 0)
        trunker_n = trunker_counts.get(sig, 0)

        if op25_n == 0:
            status = "EXTRA"
        elif trunker_n >= op25_n:
            status = "OK"
            covered += op25_n
        else:
            status = f"MISS({op25_n - trunker_n})"
            deficit = op25_n - trunker_n
            missed += deficit
            covered += trunker_n
            missed_sigs[sig] = (op25_n, trunker_n, deficit)

        sig_str = str(sig)
        if len(sig_str) > 53:
            sig_str = sig_str[:50] + "..."
        print(f"  {sig_str:<53s} {op25_n:>6d} {trunker_n:>8d} {status:>8s}")

    total_op25 = sum(op25_counts.values())
    print(f"\n  OP25 total:     {total_op25}")
    print(f"  Covered:        {covered} ({100*covered/total_op25:.1f}%)")
    print(f"  Missed:         {missed} ({100*missed/total_op25:.1f}%)")

    if missed_sigs:
        print(f"\n  DEFICIT signatures:")
        for sig, (op25_n, trunker_n, deficit) in sorted(missed_sigs.items()):
            print(f"    {sig}: OP25={op25_n}, trunker={trunker_n}, deficit={deficit}")

    # === SEQUENTIAL COMPARISON ===
    print(f"\n{'='*80}")
    print("SEQUENTIAL COMPARISON (temporal ordering)")
    print(f"{'='*80}")

    seq_matched, seq_unmatched = compare_sequential(op25_reduced, trunker_reduced)
    print(f"\n  Sequential matches:   {seq_matched} / {total_op25} "
          f"({100*seq_matched/total_op25:.1f}%)")
    print(f"  Sequential misses:    {len(seq_unmatched)} / {total_op25}")

    if seq_unmatched:
        # Analyze where the misses are
        print(f"\n  Miss positions (OP25 indices): ", end="")
        if len(seq_unmatched) <= 20:
            print(seq_unmatched)
        else:
            print(f"{seq_unmatched[:10]} ... {seq_unmatched[-10:]}")

        # Which opcodes are missed sequentially?
        miss_opcodes = Counter()
        for idx in seq_unmatched:
            miss_opcodes[op25_reduced[idx][0]] += 1
        print(f"\n  Sequential misses by opcode:")
        for op, count in miss_opcodes.most_common():
            print(f"    {op}: {count}")

    # === OPCODE DISTRIBUTION ===
    print(f"\n{'='*80}")
    print("OPCODE DISTRIBUTION")
    print(f"{'='*80}")

    op25_opcodes = Counter(s[0] for s in op25_reduced)
    trunker_opcodes = Counter(s[0] for s in trunker_reduced)
    all_opcodes = sorted(set(list(op25_opcodes.keys()) + list(trunker_opcodes.keys())))

    print(f"\n  {'Opcode':<8s} {'OP25':>6s} {'Trunker':>8s} {'Ratio':>8s}")
    print("  " + "-" * 34)
    for op in all_opcodes:
        o = op25_opcodes.get(op, 0)
        t = trunker_opcodes.get(op, 0)
        ratio = f"{t/o:.2f}x" if o > 0 else "new"
        print(f"  {op:<8s} {o:>6d} {t:>8d} {ratio:>8s}")

    # === FINAL VERDICT ===
    print(f"\n{'='*80}")
    print("VERDICT")
    print(f"{'='*80}")

    if missed == 0 and len(seq_unmatched) == 0:
        extra = len(trunker_tsbks) - len(op25_tsbks)
        print(f"\n  SUPERSET CONFIRMED.")
        print(f"  Trunker decodes every TSBK that OP25 does, plus {extra} more.")
        print(f"  Temporal ordering is preserved.")
    elif missed == 0:
        extra = len(trunker_tsbks) - len(op25_tsbks)
        print(f"\n  BAG-LEVEL SUPERSET (count-wise, trunker has enough of each type).")
        print(f"  Sequential misses: {len(seq_unmatched)} "
              f"({100*len(seq_unmatched)/total_op25:.1f}%)")
        print(f"  This means trunker decodes the same content but possibly in")
        print(f"  slightly different temporal positions (different sync points).")
    elif missed <= total_op25 * 0.05:
        pct = 100 * missed / total_op25
        print(f"\n  NEAR-SUPERSET ({missed} deficits = {pct:.1f}%).")
        print(f"  Trunker covers {covered}/{total_op25} of OP25's output.")
    else:
        pct = 100 * missed / total_op25
        print(f"\n  PARTIAL OVERLAP ({missed} deficits = {pct:.1f}%).")
        print(f"  Trunker covers {covered}/{total_op25} of OP25's output.")
        print(f"  See deficit signatures above for what's missing.")


if __name__ == '__main__':
    main()
