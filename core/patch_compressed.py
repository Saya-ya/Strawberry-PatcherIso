"""
patch_compressed.py — Patches text directly in the LZ77 compressed stream
without needing to recompress. Only works for bytes that are LITERAL.

Strategy:
  1. Trace decompression to map each output byte to its position
     in the compressed stream (LIT = direct position, MATCH = reference)
  2. For LITERAL bytes, modify compressed[comp_pos] directly
  3. For MATCH bytes, the change must be made at the match source

Usage:
    python patch_compressed.py <file_id> <dec_offset> <new_bytes>
    python patch_compressed.py 7461 0x1AB5 "奧深園のくに"
"""

import struct
import sys
from pathlib import Path

from lz77 import decompress
from datafat import read_entries, find_row


def trace_decompression(comp_data, expected_size):
    """
    Traces decompression and returns:
    - output: decompressed bytes
    - mapping: list of (type, comp_offset, ...) per output byte
      type='LIT': (comp_offset, value)
      type='MATCH': (comp_offset, window_offset, sub_index)
    """
    out = bytearray()
    window = bytearray([0x00] * 4096) 
    window_pos = 0xFEE
    src_pos = 0
    mapping = []

    while src_pos < len(comp_data) and len(out) < expected_size:
        flags = comp_data[src_pos]
        src_pos += 1

        for bit in range(8):
            if len(out) >= expected_size:
                break
            if src_pos >= len(comp_data):
                break

            is_literal = (flags & (1 << bit)) != 0

            if is_literal:
                val = comp_data[src_pos]
                mapping.append(('LIT', src_pos))
                src_pos += 1
                out.append(val)
                window[window_pos] = val
                window_pos = (window_pos + 1) & 0xFFF
            else:
                b1, b2 = comp_data[src_pos], comp_data[src_pos + 1]
                match_src = src_pos
                src_pos += 2
                offset = b1 | ((b2 & 0xF0) << 4)
                length = (b2 & 0x0F) + 3

                for k in range(length):
                    if len(out) >= expected_size:
                        break
                    val = window[offset]
                    mapping.append(('MATCH', match_src, offset, k))
                    out.append(val)
                    window[window_pos] = val
                    window_pos = (window_pos + 1) & 0xFFF
                    offset = (offset + 1) & 0xFFF

    return bytes(out), mapping


def patch_file(data_bin_path, file_id, dec_offset, new_bytes):
    """
    Patches bytes in the compressed stream of an LZ77 file.
    Modifies Data.bin in-place without loading it entirely into RAM.
    """
    bin_path = Path(data_bin_path)
    file_size = bin_path.stat().st_size

    rows = read_entries(bin_path)
    row = find_row(rows, file_id)
    if row is None:
        return False, f"ID {file_id} not found in FAT", []
    data_offset = row['off']
    orig_size = row['size'] 

    with open(bin_path, 'rb') as f:
        f.seek(data_offset)
        raw = f.read(orig_size)

    hdr = raw[:4]
    if hdr != b'LZ77':
        return False, f"ID {file_id} is not LZ77", []

    expected_size = struct.unpack_from('<I', raw, 4)[0]
    comp_data = raw[12:] 

    out, mapping = trace_decompression(comp_data, expected_size)

    if dec_offset + len(new_bytes) > len(out):
        return False, f"Offset {dec_offset}+{len(new_bytes)} out of range ({len(out)} bytes)", []

    for i in range(dec_offset, dec_offset + len(new_bytes)):
        if mapping[i][0] != 'LIT':
            return False, f"Byte {i} (0x{i:04X}) is MATCH. Cannot patch directly.", []

    comp_offsets = []
    with open(bin_path, 'r+b') as f:
        for i, new_byte in enumerate(new_bytes):
            dec_i = dec_offset + i
            comp_pos = mapping[dec_i][1]
            comp_offsets.append(comp_pos)
            abs_pos = data_offset + 12 + comp_pos
            f.seek(abs_pos)
            f.write(bytes([new_byte]))

    return True, f"Patched {len(new_bytes)} bytes in {len(comp_offsets)} positions", comp_offsets


def main():
    if len(sys.argv) < 4:
        print("Usage: python patch_compressed.py <file_id> <dec_offset_hex> <new_text>")
        print("  python patch_compressed.py 7461 0x1AB5 '奧深園のくに'")
        sys.exit(1)

    file_id = int(sys.argv[1])
    dec_offset = int(sys.argv[2], 16)
    new_text = sys.argv[3]

    new_bytes = new_text.encode('utf-16-le')


    data_bin_path = "work/Data_patched.bin"

    import shutil
    src = "originales/Data.bin"
    if not Path(data_bin_path).exists():
        print(f"Copying {src} -> {data_bin_path} (first time only)...")
        shutil.copy2(src, data_bin_path)

    success, msg, comp_offsets = patch_file(data_bin_path, file_id, dec_offset, new_bytes)

    if success:
        print(f"[OK] {msg}")
        print(f"  Compressed offsets: {comp_offsets}")
        print(f"\n  Modified Data.bin: {data_bin_path}")
        print(f"  To test: rebuild ISO with this Data.bin")
    else:
        print(f"[FAIL] {msg}")


if __name__ == "__main__":
    main()
