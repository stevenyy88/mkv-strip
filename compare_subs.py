#!/usr/bin/env python3
"""Compare original and rebuilt MKV subtitle clusters."""
import sys, struct

def read_vint(f):
    first = f.read(1)
    if not first: return None, 0
    first = first[0]
    if first == 0: return None, 0
    leading = 0
    mask = 0x80
    while leading < 8 and not (first & mask):
        leading += 1
        mask >>= 1
    if leading >= 8: return None, 0
    vint_len = leading + 1
    data = bytes([first]) + f.read(vint_len - 1)
    if len(data) < vint_len: return None, 0
    marker_mask = (1 << (8 - vint_len)) - 1
    result = data[0] & marker_mask
    for b in data[1:]: result = (result << 8) | b
    return result, vint_len

def read_vint_from(data, pos=0):
    if pos >= len(data): return None, pos
    first = data[pos]
    if first == 0: return None, pos + 1
    leading = 0
    mask = 0x80
    while leading < 8 and not (first & mask):
        leading += 1
        mask >>= 1
    if leading >= 8: return None, pos
    vint_len = leading + 1
    if pos + vint_len > len(data): return None, pos
    marker_mask = (1 << (8 - vint_len)) - 1
    result = data[pos] & marker_mask
    for b in data[pos+1:pos+vint_len]: result = (result << 8) | b
    return result, pos + vint_len

def read_element_header(f):
    id_val, id_len = read_vint(f)
    if id_val is None: return None
    size_val, size_len = read_vint(f)
    if size_val is None: return None
    return id_val, size_val, id_len + size_len

def analyze_file(path, label):
    print(f"\n{'='*60}")
    print(f"  {label}: {path}")
    print(f"{'='*60}")
    
    with open(path, "rb") as f:
        # Skip EBML header
        _, ebml_size, _ = read_element_header(f)
        f.seek(ebml_size, 1)

        seg_id, seg_size, seg_hdr = read_element_header(f)
        seg_start = f.tell()
        seg_end = seg_start + seg_size if seg_size != (2**63 - 1) else float('inf')

        cluster_num = 0
        subtitle_blocks = []

        while f.tell() < seg_end:
            elem = read_element_header(f)
            if elem is None: break
            elem_id, elem_size, elem_hdr = elem

            if elem_id == 0x1F43B675:  # Cluster
                cluster_start = f.tell()
                cluster_end = cluster_start + elem_size
                cluster_num += 1

                # Read cluster children
                ts = None
                while f.tell() < cluster_end:
                    child = read_element_header(f)
                    if child is None: break
                    child_id, child_size, child_hdr = child
                    child_data_start = f.tell()

                    if child_id == 0xE7:  # Timestamp
                        ts_data = f.read(min(child_size, 8))
                        ts = int.from_bytes(ts_data, 'big')
                        # Skip remaining if any
                        remaining = child_size - len(ts_data)
                        if remaining > 0: f.seek(remaining, 1)
                    elif child_id == 0xA3:  # SimpleBlock
                        block_data = f.read(child_size)
                        # Parse track number
                        tn, pos = read_vint_from(block_data, 0)
                        if tn == 3 and ts is not None:
                            # Parse relative timestamp
                            if pos + 2 <= len(block_data):
                                rel_ts = struct.unpack('>h', block_data[pos:pos+2])[0]
                                flags = block_data[pos+2] if pos+2 < len(block_data) else 0
                                subtitle_blocks.append({
                                    'cluster': cluster_num,
                                    'cluster_ts': ts,
                                    'rel_ts': rel_ts,
                                    'abs_ts': ts + rel_ts,
                                    'flags': flags,
                                    'type': 'SimpleBlock',
                                    'data_size': child_size,
                                })
                    elif child_id == 0xA0:  # BlockGroup
                        bg_data = f.read(child_size)
                        # Parse BlockGroup: find Block (0xA1) and BlockDuration (0x9B)
                        block = None
                        block_duration = None
                        bg_pos = 0
                        while bg_pos < len(bg_data):
                            bh = read_vint_from(bg_data, bg_pos)
                            if bh is None: break
                            bh_id, bh_pos = bh
                            bs, bh_pos2 = read_vint_from(bg_data, bh_pos)
                            if bs is None: break
                            bh_end = bh_pos2 + bs
                            if bh_end > len(bg_data): break
                            
                            if bh_id == 0xA1:  # Block
                                block = bg_data[bh_pos2:bh_end]
                            elif bh_id == 0x9B:  # BlockDuration
                                block_duration = int.from_bytes(bg_data[bh_pos2:bh_end], 'big')
                            
                            bg_pos = bh_end
                        
                        if block is not None:
                            tn, pos = read_vint_from(block, 0)
                            if tn == 3 and ts is not None:
                                if pos + 2 <= len(block):
                                    rel_ts = struct.unpack('>h', block[pos:pos+2])[0]
                                    flags = block[pos+2] if pos+2 < len(block) else 0
                                    subtitle_blocks.append({
                                        'cluster': cluster_num,
                                        'cluster_ts': ts,
                                        'rel_ts': rel_ts,
                                        'abs_ts': ts + rel_ts,
                                        'flags': flags,
                                        'type': 'BlockGroup',
                                        'duration': block_duration,
                                        'data_size': child_size,
                                    })
                    else:
                        f.seek(child_data_start + child_size)
            else:
                f.seek(f.tell() + elem_size)

        # Print results
        print(f"  Total clusters: {cluster_num}")
        print(f"  Subtitle blocks (track 3): {len(subtitle_blocks)}")
        
        if subtitle_blocks:
            print(f"\n  First 5 subtitle blocks:")
            for b in subtitle_blocks[:5]:
                dur_str = f", duration={b.get('duration')}" if 'duration' in b else ""
                print(f"    Cluster {b['cluster']} (ts={b['cluster_ts']}): "
                      f"rel_ts={b['rel_ts']}, abs_ts={b['abs_ts']}, "
                      f"type={b['type']}{dur_str}, "
                      f"flags=0x{b['flags']:02x}, "
                      f"data_size={b['data_size']}")
            
            print(f"\n  Last 5 subtitle blocks:")
            for b in subtitle_blocks[-5:]:
                dur_str = f", duration={b.get('duration')}" if 'duration' in b else ""
                print(f"    Cluster {b['cluster']} (ts={b['cluster_ts']}): "
                      f"rel_ts={b['rel_ts']}, abs_ts={b['abs_ts']}, "
                      f"type={b['type']}{dur_str}, "
                      f"flags=0x{b['flags']:02x}, "
                      f"data_size={b['data_size']}")
            
            # Check for issues
            print(f"\n  Diagnostics:")
            
            # Check if any blocks have negative relative timestamps
            neg_ts = [b for b in subtitle_blocks if b['rel_ts'] < 0]
            if neg_ts:
                print(f"    WARNING: {len(neg_ts)} blocks have negative relative timestamps!")
                for b in neg_ts[:3]:
                    print(f"      Cluster {b['cluster']}: rel_ts={b['rel_ts']}")
            
            # Check if any blocks have abs_ts = 0
            zero_ts = [b for b in subtitle_blocks if b['abs_ts'] == 0]
            if zero_ts:
                print(f"    WARNING: {len(zero_ts)} blocks have absolute timestamp = 0!")
            
            # Check if BlockDuration is present
            bg_blocks = [b for b in subtitle_blocks if b['type'] == 'BlockGroup']
            sb_blocks = [b for b in subtitle_blocks if b['type'] == 'SimpleBlock']
            print(f"    BlockGroup blocks: {len(bg_blocks)}")
            print(f"    SimpleBlock blocks: {len(sb_blocks)}")
            
            if bg_blocks:
                durations = [b.get('duration') for b in bg_blocks]
                none_durations = [d for d in durations if d is None]
                zero_durations = [d for d in durations if d == 0]
                print(f"    BlockDuration present: {len([d for d in durations if d is not None])}/{len(bg_blocks)}")
                if none_durations:
                    print(f"    WARNING: {len(none_durations)} BlockGroups have NO BlockDuration!")
                if zero_durations:
                    print(f"    WARNING: {len(zero_durations)} BlockGroups have BlockDuration = 0!")
            
            # Check absolute timestamp ordering
            abs_ts_list = [b['abs_ts'] for b in subtitle_blocks]
            out_of_order = sum(1 for i in range(1, len(abs_ts_list)) if abs_ts_list[i] < abs_ts_list[i-1])
            if out_of_order:
                print(f"    WARNING: {out_of_order} subtitle blocks have out-of-order absolute timestamps!")

if __name__ == "__main__":
    if len(sys.argv) < 3:
        print("Usage: python3 compare_subs.py <original.mkv> <rebuilt.mkv>")
        sys.exit(1)
    analyze_file(sys.argv[1], "ORIGINAL")
    analyze_file(sys.argv[2], "REBUILT")
