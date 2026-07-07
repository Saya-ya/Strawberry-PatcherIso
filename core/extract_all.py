"""
extract_all.py — Extracts files from Data.bin using the correct FAT.

FAT: offset 0x8004, 27411 entries of 12 bytes.
IMPORTANT: the size_field of a row is the size of the PREVIOUS file; the
real size of an ID is in the size_field of the next row. See datafat.py.
LZ77: 12-byte header, corrected decompressor (LSB-first, 4096 window)

Usage:
    python extract_all.py                         
    python extract_all.py --type lz77             
    python extract_all.py --type lz77 --limit 10  
    python extract_all.py --id 7461               
"""

import sys
from pathlib import Path

from lz77 import decompress
from datafat import FAT_OFFSET, NUM_ENTRIES, ENTRY_SIZE, read_entries

BIN_PATH = Path("originales/Data.bin")
INDEX_PATH = Path("work/index.json")
OUT_BASE = Path("work")


def parse_fat(data):
    """Parse FAT from raw Data.bin bytes."""
    return [
        {
            "index": r["row"],
            "id": r["id"],
            "size": r["size"],            
            "size_field": r["size_field"],
            "offset": r["off"],
        }
        for r in read_entries(data) if r["is_file"]
    ]


def extract_entry(data_bin, entry, out_base):
    off = entry["offset"]
    size = entry["size"]
    fid = entry["id"]

    if off + 4 > len(data_bin):
        return f"ID {fid}: offset out of range"

    raw = data_bin[off:off + size]
    hdr = raw[:4]

    if hdr == b"LZ77":
        out_dir = out_base / "scripts_extraidos"
        out_dir.mkdir(parents=True, exist_ok=True)
        out_path = out_dir / f"ID_{fid:05d}.dec"
        try:
            dec = decompress(raw)
            out_path.write_bytes(dec)
            return f"LZ77 -> {out_path.name} ({len(dec):,} bytes dec)"
        except Exception as e:
            return f"LZ77 -> ERROR: {e}"

    elif hdr == b"SShd":
        out_dir = out_base / "audio"
        out_dir.mkdir(parents=True, exist_ok=True)
        out_path = out_dir / f"ID_{fid:05d}.ss2"
        out_path.write_bytes(raw)
        return f"SS2 -> {out_path.name}"

    elif raw[:4] == b"\x00\x00\x00\x00":
        return f"ZERO -> (omitted)"

    else:
        ext = ".bin"
        if raw[:4] == b"TIM2":
            ext = ".tm2"
        out_dir = out_base / "raw"
        out_dir.mkdir(parents=True, exist_ok=True)
        out_path = out_dir / f"ID_{fid:05d}{ext}"
        out_path.write_bytes(raw)
        return f"RAW -> {out_path.name}"


def main():
    import argparse
    parser = argparse.ArgumentParser(description="Extract files from Data.bin")
    parser.add_argument("--type", choices=["lz77", "ss2", "raw", "all"], default="all")
    parser.add_argument("--limit", type=int, default=0)
    parser.add_argument("--id", type=int, default=0)
    parser.add_argument("--bin", default=str(BIN_PATH))
    parser.add_argument("--fat-only", action="store_true", help="Only show FAT entries without extracting")
    args = parser.parse_args()

    bin_path = Path(args.bin)
    if not bin_path.exists():
        print(f"ERROR: {bin_path} does not exist.")
        sys.exit(1)

    print(f"Loading Data.bin ({bin_path.stat().st_size / 1024 / 1024:.0f} MB)...")
    with open(bin_path, "rb") as f:
        data_bin = f.read()

    print(f"Parsing FAT from 0x{FAT_OFFSET:08X}...")
    entries = parse_fat(data_bin)
    print(f"Entries: {len(entries)}")

    if args.fat_only:
        for e in entries[:50]:
            hdr = data_bin[e["offset"]:e["offset"]+4] if e["offset"] + 4 <= len(data_bin) else b"???"
            ftype = "LZ77" if hdr == b"LZ77" else ("SS2" if hdr == b"SShd" else hdr.hex())
            print(f"  [{e['index']:5d}] ID={e['id']:6d}  size={e['size']:>10,d}  off=0x{e['offset']:08X}  type={ftype}")
        return

    to_extract = []
    if args.id:
        for e in entries:
            if e["id"] == args.id:
                to_extract.append(e)
                break
    else:
        for e in entries:
            if args.type == "all":
                to_extract.append(e)
            elif args.type == "lz77" and data_bin[e["offset"]:e["offset"]+4] == b"LZ77":
                to_extract.append(e)
            elif args.type == "ss2" and data_bin[e["offset"]:e["offset"]+4] == b"SShd":
                to_extract.append(e)
            elif args.type == "raw":
                hdr = data_bin[e["offset"]:e["offset"]+4]
                if hdr not in (b"LZ77", b"SShd", b"\x00\x00\x00\x00"):
                    to_extract.append(e)
            if args.limit and len(to_extract) >= args.limit:
                break

    if args.id and not to_extract:
        print(f"ERROR: ID {args.id} not found.")
        sys.exit(1)

    print(f"Extracting {len(to_extract)} files...")
    count = 0
    for e in to_extract:
        try:
            msg = extract_entry(data_bin, e, OUT_BASE)
            count += 1
            if count % 100 == 0 or count == len(to_extract) or args.id:
                print(f"  [{count}/{len(to_extract)}] {msg}")
        except Exception as ex:
            print(f"  ERROR ID {e['id']}: {ex}")

    print(f"\nExtraction completed: {count} files in {OUT_BASE}/")


if __name__ == "__main__":
    main()
