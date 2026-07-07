"""
build_iso.py — Rebuilds the ISO with the modified Data.bin.

Searches for the Data.bin signature in the original ISO and replaces it.

Usage:
    python build_iso.py                          
    python build_iso.py <data_bin_patched>       
"""

import os
import sys
from pathlib import Path


SIGNATURE = b"\x00\xC8\x78\x78\x13\x6B\x00\x00\x00\x80\x00\x00"


def build_iso(patched_bin_path, iso_in=None, iso_out=None):
    """Injects modified Data.bin into the ISO."""
    if iso_in is None:
        iso_in = Path('input') / 'original.iso'
    if iso_out is None:
        iso_out = Path('output') / 'patched.iso'
    
    iso_in = Path(iso_in)
    iso_out = Path(iso_out)
    patched_bin = Path(patched_bin_path)
    
    if not iso_in.exists():
        print(f"ERROR: ISO not found: {iso_in}")
        return None
    if not patched_bin.exists():
        print(f"ERROR: Data.bin not found: {patched_bin}")
        return None
    
    if not iso_out.exists():
        import shutil
        print(f"Copying base ISO...")
        shutil.copy2(iso_in, iso_out)
    
    print(f"Searching for Data.bin in ISO...")
    target_offset = -1
    chunk_size = 16 * 1024 * 1024
    overlap = len(SIGNATURE) - 1
    
    with open(iso_out, "rb") as f:
        offset = 0
        while True:
            f.seek(offset)
            chunk = f.read(chunk_size)
            if not chunk:
                break
            idx = chunk.find(SIGNATURE)
            if idx != -1:
                target_offset = offset + idx
                break
            offset += len(chunk) - overlap
    
    if target_offset == -1:
        print("ERROR: Data.bin not found in ISO")
        return None
    
    print(f"  Data.bin at offset 0x{target_offset:X}")
    
    bin_size = os.path.getsize(patched_bin)
    print(f"Injecting {bin_size:,} bytes...")
    
    chunk_write = 8 * 1024 * 1024
    with open(iso_out, "r+b") as f_out:
        f_out.seek(target_offset)
        with open(patched_bin, "rb") as f_in:
            written = 0
            while True:
                block = f_in.read(chunk_write)
                if not block:
                    break
                f_out.write(block)
                written += len(block)
                if written % (200 * 1024 * 1024) == 0:
                    pct = written / bin_size * 100
                    print(f"  {pct:.0f}%")
    
    final_size = os.path.getsize(iso_out)
    print(f"\nISO created: {iso_out}")
    print(f"Size: {final_size:,} bytes ({final_size / 1024 / 1024:.0f} MB)")
    return iso_out


def main():
    patched_bin = "work/Data_patched.bin"
    if len(sys.argv) > 1:
        patched_bin = sys.argv[1]
    
    result = build_iso(patched_bin)
    if result:
        print(f"\nReady to test in PCSX2: {result}")


if __name__ == '__main__':
    main()
