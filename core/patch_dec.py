"""
patch_dec.py — Script Rebuilder for Strawberry Panic! (PS2)

Full pipeline:
  1. Reads the modified .dec (or applies translations from CSV)
  2. Recompresses with lz77.compress() (window initialized with 0x00)
  3. Calculates the REAL slot size (distance to next offset in FAT)
  4. Injects within the slot without corrupting neighbor files

IMPORTANT: The 'size_field' of an ID's row is NOT the size of that
file: it is the size of the previous file. The real size read by the game
is in the size_field of the NEXT row. See tools/datafat.py.

Usage:
    python patch_dec.py --id 7461 --dec work/scripts_extraidos/ID_07461.dec
    python patch_dec.py --id 7461 --dec work/scripts_extraidos/ID_07461.dec --verify
    python patch_dec.py --id 7461 --rebuild --csv textos/dialogo.csv --verify
"""

import struct
import sys
import shutil
import argparse
from pathlib import Path

from lz77 import decompress, compress
from datafat import (
    FAT_OFFSET,
    NUM_ENTRIES as FAT_ENTRIES,
    ENTRY_SIZE,
    read_entries,
    find_row,
    slot_capacity,
    size_field_write_offset,
)
from script_rebuilder import (
    default_dec_path,
    load_csv_rows,
    rebuild_local_slack,
)

DATA_BIN_ORIG = Path('originales/Data.bin')
DATA_BIN_WORK = Path('work/Data_patched.bin')
def load_fat(bin_path):
    """Reads the full FAT and returns canonical entries for real files."""
    rows = read_entries(bin_path)
    entries = [
        {
            'row': r['row'],
            'fid': r['id'],
            'size_field': r['size_field'],
            'size': r['size'],
            'foff': r['off'],
        }
        for r in rows if r['is_file']
    ]
    entries_by_offset = sorted(entries, key=lambda e: e['foff'])
    return entries, entries_by_offset, rows


def get_slot_info(target_fid, bin_path):
    """
    Returns (foff, real_slot_size) for a file ID.
    real_slot_size = distance to the next entry in the file.
    """
    rows = read_entries(bin_path)

    target = find_row(rows, target_fid)
    if target is None:
        return None, None

    return target['off'], slot_capacity(rows, target)


def get_file_info(target_fid, bin_path):
    """
    Returns (row, foff, file_size, slot_size) for an ID.
    file_size is the REAL size that the loader reads (size_field of next row).
    slot_size is the physical capacity up to the next offset.
    """
    rows = read_entries(bin_path)
    target = find_row(rows, target_fid)
    if target is None:
        return None, None, None, None
    return target, target['off'], target['size'], slot_capacity(rows, target)


def decompress_from_data_bin(target_fid, bin_path):
    """Decompresses the script of a file ID from Data.bin."""
    _, foff, file_size, slot_size = get_file_info(target_fid, bin_path)
    if foff is None:
        raise ValueError(f"ID {target_fid} not found in FAT")

    with open(bin_path, 'rb') as f:
        f.seek(foff)
        raw = f.read(file_size)

    if raw[:4] != b'LZ77':
        raise ValueError(f"ID {target_fid} at 0x{foff:08X} is not LZ77 (magic: {raw[:4].hex()})")

    return decompress(raw)


def inject_compressed(target_fid, comp_data, bin_path):
    """
    Injects compressed data into the slot of a file ID.
    Only writes within the real slot (does not corrupt neighbors).
    Updates the size_field in the FAT so the PS2 reads the correct bytes.
    """
    target, foff, old_file_size, slot_size = get_file_info(target_fid, bin_path)
    if foff is None:
        raise ValueError(f"ID {target_fid} not found in FAT")

    if len(comp_data) > slot_size:
        raise ValueError(
            f"Data ({len(comp_data):,} bytes) does not fit in slot ({slot_size:,} bytes). "
            f"Relocation needed."
        )

    file_size = len(comp_data) 

    with open(bin_path, 'r+b') as f:
        f.seek(foff)
        f.write(comp_data)
        f.write(b'\x00' * (slot_size - len(comp_data)))

        f.seek(size_field_write_offset(target))
        f.write(struct.pack('<I', file_size))

    return foff, slot_size


def patch_script(target_fid, dec_path, bin_path, verify=False, verbose=True, all_literal=False):
    """
    Full pipeline: read .dec → recompress → inject.
    If all_literal=True, uses 100% literals (no matches).
    """
    if verbose:
        print(f"[*] Processing ID {target_fid} from {dec_path}")

    _, foff, file_size, slot_size = get_file_info(target_fid, bin_path)
    if foff is None:
        print(f"[!] ID {target_fid} not found")
        return False

    if verbose:
        print(f"    Offset: 0x{foff:08X}, slot: {slot_size:,} bytes ({slot_size // 1024}KB)")

    with open(dec_path, 'rb') as f:
        dec_data = f.read()

    if verify:
        comp_test = compress(dec_data, all_literal=all_literal)
        redec_test = decompress(comp_test)
        diffs = sum(a != b for a, b in zip(dec_data, redec_test))
        if diffs > 0 or len(dec_data) != len(redec_test):
            print(f"[!] Round-trip FAILED: {diffs} diffs, "
                  f"orig={len(dec_data)}, redec={len(redec_test)}")
            return False
        if verbose:
            print(f"    Round-trip: OK (0 diffs)")

    comp_data = compress(dec_data, all_literal=all_literal)
    if verbose:
        our_decomp = struct.unpack_from('<I', comp_data, 4)[0]
        our_comp_size = struct.unpack_from('<I', comp_data, 8)[0]
        our_meta = struct.unpack_from('<I', comp_data, 12)[0]
        print(f"    Compressed: {len(dec_data):,} → {len(comp_data):,} bytes "
              f"({len(comp_data) / slot_size * 100:.1f}% of slot)")
        print(f"    Header: decomp={our_decomp:,} comp={our_comp_size:,} meta=0x{our_meta:08X}")

    if len(comp_data) > slot_size:
        print(f"[!] Does not fit in slot ({len(comp_data):,} > {slot_size:,}). "
              f"Relocation needed.")
        return False

    foff_written, _ = inject_compressed(target_fid, comp_data, bin_path)
    if verbose:
        print(f"    Injected at 0x{foff_written:08X} ✓")

    return True


def ensure_work_copy():
    """Creates the working copy if it does not exist or is outdated."""
    DATA_BIN_WORK.parent.mkdir(parents=True, exist_ok=True)
    if (not DATA_BIN_WORK.exists() or
            DATA_BIN_WORK.stat().st_size != DATA_BIN_ORIG.stat().st_size):
        print(f"Creating working copy ({DATA_BIN_ORIG.stat().st_size // 1024 // 1024} MB)...")
        shutil.copy2(DATA_BIN_ORIG, DATA_BIN_WORK)
        print("Copy ready.")


def main():
    parser = argparse.ArgumentParser(description='Script Rebuilder for Strawberry Panic!')
    parser.add_argument('--id',     type=int, required=True, help='File ID in FAT (e.g.: 7461)')
    parser.add_argument('--dec',    type=str, default=None, help='Path to the modified/base .dec')
    parser.add_argument('--out',    type=str, default=None,  help='Target Data.bin (default: work/Data_patched.bin)')
    parser.add_argument('--verify', action='store_true',     help='Verify round-trip before injecting')
    parser.add_argument('--all-literal', action='store_true', help='Use only literals (no matches, 100% safe)')
    parser.add_argument('--rebuild', action='store_true', help='Rebuild .dec from CSV using local-slack before recompressing')
    parser.add_argument('--csv', type=str, default='textos/dialogo.csv', help='CSV for --rebuild')
    parser.add_argument('--mode', choices=['local-slack'], default='local-slack', help='Rebuilder mode')
    parser.add_argument('--rebuilt-out', type=str, default=None, help='Output path for the rebuilt .dec')
    parser.add_argument('--no-consume-punctuation', action='store_true',
                        help='Do not consume trailing Japanese punctuation in --rebuild')
    parser.add_argument('--info',   action='store_true',     help='Only show slot info, do not modify')
    args = parser.parse_args()

    bin_path = Path(args.out) if args.out else DATA_BIN_WORK

    if args.info:
        ensure_work_copy()
        row, foff, file_size, slot_size = get_file_info(args.id, DATA_BIN_ORIG)
        if foff is None:
            print(f"ID {args.id} not found")
            return
        print(f"ID {args.id}:")
        print(f"  Offset in Data.bin: 0x{foff:08X}")
        print(f"  Real FAT size:       {file_size:,} bytes")
        print(f"  Size written to:     row {row['row'] + 1} @ 0x{size_field_write_offset(row):08X}")
        print(f"  Real slot:           {slot_size:,} bytes ({slot_size // 1024}KB)")
        with open(DATA_BIN_ORIG, 'rb') as f:
            f.seek(foff)
            hdr = f.read(16)
        if hdr[:4] == b'LZ77':
            decomp_sz = struct.unpack_from('<I', hdr, 4)[0]
            comp_sz   = struct.unpack_from('<I', hdr, 8)[0]
            meta      = struct.unpack_from('<I', hdr, 12)[0]
            print(f"  LZ77 header (12+4):    decomp={decomp_sz:,} comp={comp_sz:,} meta=0x{meta:08X}")
        return

    ensure_work_copy()

    dec_path = Path(args.dec) if args.dec else default_dec_path(args.id)
    if not dec_path.exists():
        print(f"ERROR: {dec_path} does not exist")
        sys.exit(1)

    if args.rebuild:
        print(f"[*] Rebuilder {args.mode}: ID {args.id} from {dec_path}")
        dec_data = dec_path.read_bytes()
        rows = load_csv_rows(Path(args.csv), args.id)
        rebuilt, report = rebuild_local_slack(
            dec_data,
            rows,
            consume_punctuation=not args.no_consume_punctuation,
        )
        print(f"    CSV rows: {report['rows_total']}, applied: {report['rows_applied']}, "
              f"segments: {report['segments_modified']}, needs_shift: {len(report['needs_shift'])}")
        if report['needs_shift']:
            for seg in report['needs_shift'][:10]:
                print(f"    [needs_shift] 0x{seg['start']:X}: "
                      f"requires {seg['required_bytes']} / capacity {seg['capacity_bytes']}")
            print("ERROR: some texts require shift mode; not injecting.")
            sys.exit(1)

        rebuilt_path = Path(args.rebuilt_out) if args.rebuilt_out else (
            Path('work/scripts_extraidos') / f'ID_{args.id:05d}_rebuilt.dec'
        )
        rebuilt_path.parent.mkdir(parents=True, exist_ok=True)
        rebuilt_path.write_bytes(rebuilt)
        print(f"    Rebuilt .dec: {rebuilt_path}")
        dec_path = rebuilt_path

    ok = patch_script(args.id, dec_path, bin_path, verify=args.verify, all_literal=args.all_literal)
    if ok:
        print(f"\n✓ Done. Now rebuild the ISO with:")
        print(f"    python build_iso.py")
    else:
        print(f"\n✗ Patch failed.")
        sys.exit(1)


if __name__ == '__main__':
    main()
