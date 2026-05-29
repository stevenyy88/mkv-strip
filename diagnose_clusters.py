#!/usr/bin/env python3
"""Quick diagnostic: parse MKV cluster timestamps to verify ordering."""
import sys, struct

def read_vint(f):
    """Read an EBML variable-length integer."""
    first = f.read(1)
    if not first:
        return None, 0
    first = first[0]
    if first == 0:
        return None, 0
    leading = 0
    mask = 0x80
    while leading < 8 and not (first & mask):
        leading += 1
        mask >>= 1
    if leading >= 8:
        return None, 0
    vint_len = leading + 1
    data = bytes([first]) + f.read(vint_len - 1)
    if len(data) < vint_len:
        return None, 0
    # Strip marker bit
    marker_mask = (1 << (8 - vint_len)) - 1
    result = data[0] & marker_mask
    for b in data[1:]:
        result = (result << 8) | b
    return result, vint_len

def read_element_header(f):
    """Read (id, size, header_len) or None."""
    id_val, id_len = read_vint(f)
    if id_val is None:
        return None
    size_val, size_len = read_vint(f)
    if size_val is None:
        return None
    return id_val, size_val, id_len + size_len

def main():
    path = sys.argv[1] if len(sys.argv) > 1 else "output.mkv"
    with open(path, "rb") as f:
        # Skip EBML header
        _, ebml_size, _ = read_element_header(f)
        f.seek(ebml_size, 1)  # relative seek

        # Read Segment header
        seg_id, seg_size, seg_hdr = read_element_header(f)
        if seg_id != 0x18538067:
            print(f"Not a valid MKV (Segment ID: {hex(seg_id)})")
            return

        seg_start = f.tell()
        seg_end = seg_start + seg_size if seg_size != (2**63 - 1) else float('inf')

        cluster_num = 0
        timestamps = []
        subtitle_cluster_timestamps = []

        while f.tell() < seg_end:
            elem = read_element_header(f)
            if elem is None:
                break
            elem_id, elem_size, elem_hdr = elem

            if elem_id == 0x1F43B675:  # Cluster
                cluster_start = f.tell()
                cluster_end = cluster_start + elem_size

                # Read cluster children to find Timestamp (0xE7)
                ts = None
                has_subtitle = False
                while f.tell() < cluster_end:
                    child = read_element_header(f)
                    if child is None:
                        break
                    child_id, child_size, child_hdr = child
                    child_data_start = f.tell()

                    if child_id == 0xE7:  # Timestamp
                        ts_data = f.read(min(child_size, 8))
                        ts = int.from_bytes(ts_data, 'big')
                    elif child_id == 0xA3:  # SimpleBlock
                        block = f.read(min(child_size, 10))
                        # Parse track number from first VInt
                        if block:
                            tn, _ = read_vint_from_bytes(block)
                            if tn == 3:  # subtitle track
                                has_subtitle = True
                        f.seek(child_data_start + child_size)
                    elif child_id == 0xA0:  # BlockGroup
                        bg_data = f.read(min(child_size, 20))
                        # Check if it contains a block for track 3
                        # Parse: look for Block element (0xA1) within
                        bg_pos = 0
                        while bg_pos < len(bg_data):
                            if bg_data[bg_pos] == 0xA1:  # Block
                                # Next byte is size, then block data
                                if bg_pos + 2 < len(bg_data):
                                    block_size = bg_data[bg_pos + 1]
                                    if block_size < 0x80:  # 1-byte size
                                        block_start = bg_pos + 2
                                        if block_start < len(bg_data):
                                            tn, _ = read_vint_from_bytes(bg_data[block_start:])
                                            if tn == 3:
                                                has_subtitle = True
                                break
                            bg_pos += 1
                        f.seek(child_data_start + child_size)
                    else:
                        f.seek(child_data_start + child_size)

                cluster_num += 1
                if ts is not None:
                    timestamps.append(ts)
                    if has_subtitle:
                        subtitle_cluster_timestamps.append(ts)

            else:
                # Skip non-cluster elements
                f.seek(f.tell() + elem_size)

        # Analysis
        print(f"Total clusters: {cluster_num}")
        print(f"Clusters with timestamps: {len(timestamps)}")
        print(f"Subtitle clusters: {len(subtitle_cluster_timestamps)}")

        if subtitle_cluster_timestamps:
            print(f"Subtitle cluster timestamps: {subtitle_cluster_timestamps[:5]}{'...' if len(subtitle_cluster_timestamps) > 5 else ''}")

        # Check ordering
        out_of_order = 0
        for i in range(1, len(timestamps)):
            if timestamps[i] < timestamps[i-1]:
                out_of_order += 1
        print(f"Out-of-order clusters: {out_of_order}")

        if subtitle_cluster_timestamps:
            # Find where subtitle clusters appear in the sequence
            for st in subtitle_cluster_timestamps[:3]:
                pos = timestamps.index(st) if st in timestamps else -1
                before = timestamps[pos-1] if pos > 0 else None
                after = timestamps[pos+1] if pos < len(timestamps)-1 else None
                print(f"  Subtitle cluster at {st}ms (position {pos+1}): before={before}ms, after={after}ms")

def read_vint_from_bytes(data):
    """Read a VInt from bytes, return (value, bytes_consumed)."""
    if not data:
        return None, 0
    first = data[0]
    if first == 0:
        return None, 0
    leading = 0
    mask = 0x80
    while leading < 8 and not (first & mask):
        leading += 1
        mask >>= 1
    if leading >= 8:
        return None, 0
    vint_len = leading + 1
    if len(data) < vint_len:
        return None, 0
    marker_mask = (1 << (8 - vint_len)) - 1
    result = data[0] & marker_mask
    for b in data[1:vint_len]:
        result = (result << 8) | b
    return result, vint_len

if __name__ == "__main__":
    main()
