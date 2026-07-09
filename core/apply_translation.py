"""
apply_translation.py — Applies translations from CSV to Data.bin.

Reads the CSV with columns [source, file_id, offset, original_text, translated_text].
For each row with a translation:
  1. Converts Spanish → Cyrillic (font mapping)
  2. Encodes to UTF-16LE (scripts) or Shift-JIS (ELF)
  3. If bytes fit in original space, patches directly in compressed stream
  4. If they don't fit, reports warning and skips

The Spanish→Cyrillic mapping lives HERE in code, not in the CSV.

Usage:
    python apply_translation.py traduccion_tools/dialogue_scripts.csv
"""

import csv
import struct
import sys
from pathlib import Path

from lz77 import decompress
from patch_compressed import trace_decompression
from datafat import read_entries, find_row
from glyph_map import SPANISH_TO_GLYPH, game_string, encode_game_utf16 as _enc_utf16, encode_game_sjis as _enc_sjis, get_glyph_map

SPANISH_TO_GLYPH_UTF16 = SPANISH_TO_GLYPH


def encode_for_game_utf16(text, glyph_map=None):
    """Converts text to UTF-16LE with Cyrillic mapping."""
    if glyph_map is None:
        return _enc_utf16(text)
    return _enc_utf16(text, glyph_map)


def encode_for_game_sjis(text, glyph_map=None):
    """Converts text to Shift-JIS with Cyrillic mapping."""
    if glyph_map is None:
        return _enc_sjis(text)
    return _enc_sjis(text, glyph_map)


def patch_script_text(data_bin_path, file_id, dec_offset, new_bytes_utf16le):
    """
    Patches text in an LZ77 script.
    Only works if the bytes are LITERAL in the compressed stream.
    """
    bin_path = Path(data_bin_path)
    
    rows = read_entries(bin_path)
    row = find_row(rows, file_id)
    if row is None:
        return False, f"ID {file_id} not found"
    data_offset = row['off']
    orig_size = row['size'] 
    
    with open(bin_path, 'rb') as f:
        f.seek(data_offset)
        raw = f.read(orig_size)
    
    if raw[:4] != b'LZ77':
        return False, "Not LZ77"
    
    expected_size = struct.unpack_from('<I', raw, 4)[0]
    comp_data = raw[12:] 
    out, mapping = trace_decompression(comp_data, expected_size)
    
    if dec_offset + len(new_bytes_utf16le) > len(out):
        return False, f"Out of range"
    
    for i in range(dec_offset, dec_offset + len(new_bytes_utf16le)):
        if mapping[i][0] != 'LIT':
            return False, f"Byte {i} is MATCH, cannot patch"
    
    comp_offsets = []
    with open(bin_path, 'r+b') as f:
        for i, new_byte in enumerate(new_bytes_utf16le):
            dec_i = dec_offset + i
            comp_pos = mapping[dec_i][1]
            comp_offsets.append(comp_pos)
            abs_pos = data_offset + 12 + comp_pos
            f.seek(abs_pos)
            f.write(bytes([new_byte]))
    
    return True, f"OK ({len(new_bytes_utf16le)} bytes in {len(comp_offsets)} positions)"


def apply_translations(csv_path, data_bin_path, elf_path=None, target_lang="es"):
    """Reads the CSV and applies all translations."""
    glyph_map = get_glyph_map(target_lang) if target_lang != "es" else None
    bin_path = Path(data_bin_path)
    if not bin_path.exists():
        print(f"ERROR: {bin_path} does not exist")
        return
    
    work_path = bin_path.parent.parent / 'work' / 'Data_patched.bin'
    work_elf_path = bin_path.parent.parent / 'work' / 'SLPS_256.11_translated'
    
    import shutil
    work_path.parent.mkdir(parents=True, exist_ok=True)
    
    if not work_path.exists() or work_path.stat().st_size != bin_path.stat().st_size:
        print(f"Creating working copy: {work_path}")
        shutil.copy2(bin_path, work_path)
        
    elf_orig = Path(elf_path) if elf_path else bin_path.parent / 'SLPS_256.11'
    if elf_orig.exists():
        if not work_elf_path.exists() or work_elf_path.stat().st_size != elf_orig.stat().st_size:
            print(f"Creating ELF working copy: {work_elf_path}")
            shutil.copy2(elf_orig, work_elf_path)
    
    stats = {'ok': 0, 'skip_size': 0, 'skip_match': 0, 'skip_other': 0}
    
    with open(csv_path, encoding='utf-8-sig') as f:
        reader = csv.DictReader(f)
        total = 0
        for row in reader:
            translated = row.get('translated_text', '').strip()
            if not translated:
                continue
            
            total += 1
            source = row.get('source', 'SCRIPT')
            file_id_str = row.get('file_id', '')
            offset_str = row.get('offset', '0x0')
            original = row.get('original_text', '')
            
            try:
                dec_offset = int(offset_str, 16)
            except:
                stats['skip_other'] += 1
                continue
            
            if source == 'ELF':
                new_bytes = encode_for_game_sjis(translated, glyph_map)
                orig_bytes = original.encode('shift-jis')
                
                if len(new_bytes) <= len(orig_bytes):
                    with open(work_elf_path, 'r+b') as elf:
                        elf.seek(dec_offset)
                        elf.write(new_bytes)
                        padding = len(orig_bytes) - len(new_bytes)
                        if padding > 0:
                            elf.write(b'\x20' * padding)
                    stats['ok'] += 1
                else:
                    stats['skip_size'] += 1
                    if total <= 20:
                        print(f"  [!] '{translated[:40]}': {len(new_bytes)} > {len(orig_bytes)} bytes")
            
            elif source == 'SCRIPT':
                file_id = int(file_id_str)
                new_bytes = encode_for_game_utf16(translated, glyph_map)
                orig_bytes = original.encode('utf-16-le')
                
                if len(new_bytes) > len(orig_bytes):
                    stats['skip_size'] += 1
                    if total <= 20:
                        print(f"  [!] ID {file_id}: '{translated[:40]}' -> {len(new_bytes)} > {len(orig_bytes)} bytes")
                    continue
                
                if len(new_bytes) < len(orig_bytes):
                    new_bytes = new_bytes + b'\x00' * (len(orig_bytes) - len(new_bytes))
                
                success, msg = patch_script_text(str(work_path), file_id, dec_offset, new_bytes)
                if success:
                    stats['ok'] += 1
                elif 'MATCH' in msg:
                    stats['skip_match'] += 1
                    if total <= 20:
                        print(f"  [!] ID {file_id}: '{translated[:40]}' -> MATCH (not patchable)")
                else:
                    stats['skip_other'] += 1
                    if total <= 20:
                        print(f"  [!] ID {file_id}: {msg}")
            
            if total % 200 == 0:
                print(f"  Processed {total} translations...")
    
    print(f"\n=== Results ===")
    print(f"  Total processed:    {total}")
    print(f"  Applied:            {stats['ok']}")
    print(f"  Skipped (size):     {stats['skip_size']}")
    print(f"  Skipped (Match):    {stats['skip_match']}")
    print(f"  Skipped (other):    {stats['skip_other']}")
    print(f"\nModified Data.bin: {work_path}")
    return work_path


def main():
    import argparse
    parser = argparse.ArgumentParser(
        description='Apply translations from CSV to Data.bin and ELF')
    parser.add_argument('csv_path', nargs='?', default=None,
                        help='Path to translation CSV')
    parser.add_argument('--target-lang', default='es',
                        choices=['es', 'en', 'custom'],
                        help='Target language (default: es)')
    args = parser.parse_args()

    if args.csv_path is None:
        parser.print_help()
        sys.exit(1)

    data_bin = "originales/Data.bin"
    apply_translations(args.csv_path, data_bin, target_lang=args.target_lang)


if __name__ == '__main__':
    main()
