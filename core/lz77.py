"""
lz77.py — PS2 LZSS decompressor (verified against MIPS in SLPS_256.11).

Header format (12 bytes, NOT 16):
  [magic "LZ77"(4)] [decomp_size:u32] [comp_size:u32]

The "metadata" is NOT a separate header field. The PS2 decompressor
starts reading at byte 12. The first 4 bytes of the compressed stream
are what we called "metadata". The comp_size field indicates the total
stream size from byte 12 (including those first 4 bytes).

Fix 2026-06-28: The native decompressor uses skip=12, not skip=16.
This is the root cause of the black screen bug.

Algorithm (confirmed by ELF disassembly at 0x3B500):
  - Window: 4096 bytes, start: 0xFEE, init: 0x00
  - Flags: LSB-first (bit 0 first)
  - Bit=1 → LITERAL (1 byte)
  - Bit=0 → MATCH (2 bytes):
      offset = b1 | ((b2 & 0xF0) << 4)   (12 bits, absolute in window)
      length = (b2 & 0x0F) + 3           (3-18 bytes)
"""

import struct

MAGIC = b"LZ77"
WINDOW_SIZE = 4096
WINDOW_START = 0xFEE
MIN_MATCH = 3
MAX_MATCH = 18


def decompress(compressed_data, expected_size=None, strict=True):
    """
    Decompresses data in PS2 LZSS format.
    If expected_size is None, reads it from the header.
    If LZ77 header is present, respects comp_size and discards trailing padding.
    In strict mode, fails if the stream is truncated and doesn't produce decomp_size.
    """
    data = compressed_data
    pos = 0

    if data[:4] == MAGIC:
        if len(data) < 12:
            raise ValueError("LZ77 header truncated")
        expected_size = struct.unpack_from("<I", data, 4)[0]
        comp_size = struct.unpack_from("<I", data, 8)[0]
        stream_end = 12 + comp_size
        if len(data) < stream_end:
            if strict:
                raise ValueError(
                    f"LZ77 stream truncated: have {max(0, len(data) - 12):,} bytes, "
                    f"header comp_size={comp_size:,}"
                )
            stream_end = len(data)
        data = data[12:stream_end]
    elif expected_size is None:
        raise ValueError("expected_size is required or a 12-byte LZ77 header")

    out = bytearray()
    window = bytearray([0x00] * WINDOW_SIZE)
    window_pos = WINDOW_START

    while pos < len(data) and len(out) < expected_size:
        flags = data[pos]
        pos += 1

        for bit in range(8):
            if len(out) >= expected_size:
                break

            is_literal = (flags & (1 << bit)) != 0

            if is_literal:
                if pos >= len(data):
                    break
                val = data[pos]
                pos += 1
                out.append(val)
                window[window_pos] = val
                window_pos = (window_pos + 1) & 0xFFF
            else:
                if pos + 1 >= len(data):
                    break
                b1 = data[pos]
                b2 = data[pos + 1]
                pos += 2

                offset = b1 | ((b2 & 0xF0) << 4)
                length = (b2 & 0x0F) + 3

                for _ in range(length):
                    if len(out) >= expected_size:
                        break
                    val = window[offset]
                    out.append(val)
                    window[window_pos] = val
                    window_pos = (window_pos + 1) & 0xFFF
                    offset = (offset + 1) & 0xFFF

    if strict and len(out) != expected_size:
        raise ValueError(
            f"Incomplete decompression: output={len(out):,} bytes, "
            f"expected={expected_size:,}"
        )

    return bytes(out)


def compress(uncompressed_data, all_literal=False):
    """
    Compresses using PS2 LZSS with full-window greedy search.

    The header is 12 bytes. The compressed stream starts at byte 12.
    The first 4 bytes of the stream are what was historically called "metadata"
    — the compressor generates a continuous stream and whatever lands in
    bytes 12-15 is the actual value.
    """

    data_len = len(uncompressed_data)
    compressed = bytearray()

    window = bytearray([0x00] * WINDOW_SIZE)
    window_pos = WINDOW_START

    src_pos = 0
    block_flags = 0
    bit_count = 0
    block_tokens = bytearray()

    def flush_block():
        nonlocal block_flags, bit_count, block_tokens
        if bit_count > 0:
            compressed.append(block_flags)
            compressed.extend(block_tokens)
            block_flags = 0
            bit_count = 0
            block_tokens = bytearray()

    while src_pos < data_len:
        match_len = 0
        match_offset = 0
        max_len = min(MAX_MATCH, data_len - src_pos)

        if not all_literal and max_len >= MIN_MATCH:
            best_len = 0
            best_offset = 0

            search_start = max(0, src_pos - WINDOW_SIZE)
            search_end = src_pos - MIN_MATCH
            search_range = min(search_end - search_start, WINDOW_SIZE)

            for dist in range(MIN_MATCH, min(search_range + 1, WINDOW_SIZE)):
                w_idx = (window_pos - dist) & 0xFFF
                search_max = min(max_len, dist)
                curr_len = 0
                while curr_len < search_max:
                    wpos = (w_idx + curr_len) & 0xFFF
                    if window[wpos] != uncompressed_data[src_pos + curr_len]:
                        break
                    curr_len += 1
                if curr_len > best_len:
                    best_len = curr_len
                    best_offset = w_idx
                    if best_len == max_len:
                        break

            match_len = best_len
            match_offset = best_offset

        if match_len < MIN_MATCH:
            match_len = 1
            val = uncompressed_data[src_pos]
            block_flags |= (1 << bit_count) 
            block_tokens.append(val)
        else:
            b1 = match_offset & 0xFF
            b2 = ((match_offset >> 4) & 0xF0) | ((match_len - 3) & 0x0F)
            block_tokens.extend([b1, b2])

        for k in range(match_len):
            window[window_pos] = uncompressed_data[src_pos + k]
            window_pos = (window_pos + 1) & 0xFFF

        src_pos += match_len
        bit_count += 1
        if bit_count == 8:
            flush_block()

    flush_block()

    comp_stream = bytes(compressed)
    comp_size = len(comp_stream)
    header = struct.pack("<4sII", MAGIC, data_len, comp_size)
    return header + comp_stream


def decompress_file(input_path, output_path=None):
    """Decompresses an LZ77 file to disk."""
    from pathlib import Path
    data = Path(input_path).read_bytes()
    decomp = decompress(data)
    if output_path is None:
        output_path = str(input_path) + ".dec"
    Path(output_path).write_bytes(decomp)
    return decomp


def compress_file(input_path, output_path=None, all_literal=False):
    """Compresses a raw file to 12-byte header LZ77 format."""
    from pathlib import Path
    data = Path(input_path).read_bytes()
    comp = compress(data, all_literal=all_literal)
    if output_path is None:
        output_path = str(input_path) + ".lz77"
    Path(output_path).write_bytes(comp)
    return comp


if __name__ == "__main__":
    import sys
    from pathlib import Path

    if len(sys.argv) < 2:
        print("Usage: python lz77.py [d|decompress] <input> [output]")
        print("      python lz77.py [c|compress] <input> [output]  (local tests only)")
        sys.exit(1)

    mode = sys.argv[1]
    input_path = sys.argv[2]
    output_path = sys.argv[3] if len(sys.argv) > 3 else None

    if mode in ("d", "decompress"):
        result = decompress_file(input_path, output_path)
        print(f"Decompressed: {len(result):,} bytes")
    elif mode in ("c", "compress"):
        result = compress_file(input_path, output_path)
        print(f"Compressed: {len(result):,} bytes  [WARNING: do not use for ISO]")
    else:
        print(f"Unknown mode: {mode}")
