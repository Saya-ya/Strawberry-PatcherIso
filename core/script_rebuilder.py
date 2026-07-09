"""
script_rebuilder.py — Conservative rebuild of .dec scripts (local-slack mode).

Safe MVP:
  - Does NOT move structures.
  - Does NOT update pointers.
  - Only replaces text within the UTF-16LE null-terminated string that contains
    the CSV offset.
  - Uses the zero-padding after the terminator to allow longer translations.

This covers the SCRIPT_DIALOGUE family with header 0x020000XX and table at 0x2010,
according to data from work/analysis/*.json. If a text does not fit in its local slack,
it is reported as "needs_shift" and not applied.
"""

import argparse
import csv
import json
import struct
import sys
from dataclasses import dataclass, asdict
from pathlib import Path


from glyph_map import SPANISH_TO_GLYPH, game_string as _game_string, encode_game_utf16 as _encode_game_utf16, get_glyph_map

SPANISH_TO_GLYPH = SPANISH_TO_GLYPH 

TRAILING_PUNCT = set("…。、，,.!?！？")


@dataclass
class TextSegment:
    start: int
    text_end: int         
    slack_end: int        
    text: str
    has_length_prefix: bool = False
    length_prefix_offset: int | None = None

    @property
    def old_text_bytes(self) -> int:
        return self.text_end - self.start

    @property
    def capacity_bytes(self) -> int:
        return self.slack_end - self.start

    @property
    def max_text_bytes(self) -> int:
        return self.capacity_bytes - 2


@dataclass
class TranslationRow:
    source: str
    file_id: int
    offset: int
    original_text: str
    translated_text: str
    csv_line: int


def game_string(text: str, glyph_map: dict | None = None) -> str:
    """Converts readable text to the available game glyphs."""
    if glyph_map is None:
        return _game_string(text)
    return _game_string(text, glyph_map)


def encode_game_utf16(text: str, glyph_map: dict | None = None) -> bytes:
    if glyph_map is None:
        return _encode_game_utf16(text)
    return _encode_game_utf16(text, glyph_map)


def decode_utf16(data: bytes) -> str:
    return data.decode('utf-16-le')


def parse_offset(value: str) -> int:
    value = str(value).strip()
    return int(value, 16) if value.lower().startswith('0x') else int(value)


def is_probably_script_dialogue(data: bytes) -> bool:
    if len(data) < 0x20:
        return False
    h0, h1, h2, h3, table_ptr, count, h6, h7 = struct.unpack_from('<8I', data, 0)
    return (
        (h0 & 0xFFFFFF00) == 0x02000000
        and h1 == h2 == h3 == h6 == h7 == 0
        and 0 < table_ptr < len(data)
        and count > 0
    )


def find_utf16_null(data: bytes, start: int) -> int | None:
    """Finds UTF-16LE terminator 00 00 by stepping in increments of 2."""
    pos = start
    while pos + 1 < len(data):
        if data[pos:pos + 2] == b'\x00\x00':
            return pos
        pos += 2
    return None


def find_slack_end(data: bytes, null_at: int) -> int:
    """Returns the first non-zero byte after the terminator, or EOF."""
    pos = null_at + 2
    while pos < len(data) and data[pos] == 0:
        pos += 1
    return pos


def detect_length_prefix(data: bytes, start: int, text: str) -> tuple[bool, int | None]:
    """
    Detects a simple u16 prefix just before the text. Conservative: only if the
    value matches the exact length of the containing string.
    """
    if start < 2:
        return False, None
    val = struct.unpack_from('<H', data, start - 2)[0]
    if val == len(text) and val > 0:
        return True, start - 2
    return False, None


def find_segment_containing(data: bytes, offset: int, original_text: str = "") -> TextSegment:
    """
    Finds the UTF-16LE null-terminated string that contains `offset`.
    Supports two cases:
      1) Normal string: [00 padding] [text] [00 00] [padding]
      2) String with u16 prefix immediately before the text:
         [len:u16] [text] [00 00]
    """
    if offset < 0 or offset + 1 >= len(data) or offset % 2:
        raise ValueError(f"Invalid UTF-16 offset: 0x{offset:X}")

    if original_text and offset >= 2:
        orig_bytes = original_text.encode('utf-16-le')
        prev = struct.unpack_from('<H', data, offset - 2)[0]
        if prev == len(original_text) and data[offset:offset + len(orig_bytes)] == orig_bytes:
            start = offset
            null_at = find_utf16_null(data, start)
            if null_at is None:
                raise ValueError(f"No UTF-16 terminator from 0x{start:X}")
            text = decode_utf16(data[start:null_at])
            slack_end = find_slack_end(data, null_at)
            return TextSegment(start, null_at, slack_end, text, True, offset - 2)

    start = offset
    while start >= 2 and data[start - 2:start] != b'\x00\x00':
        start -= 2

    null_at = find_utf16_null(data, start)
    if null_at is None or null_at < offset:
        raise ValueError(f"Could not locate containing string for 0x{offset:X}")

    text = decode_utf16(data[start:null_at])
    slack_end = find_slack_end(data, null_at)
    has_prefix, prefix_off = detect_length_prefix(data, start, text)
    return TextSegment(start, null_at, slack_end, text, has_prefix, prefix_off)


def load_csv_rows(csv_path: Path, file_id: int) -> list[TranslationRow]:
    rows: list[TranslationRow] = []
    with open(csv_path, encoding='utf-8-sig', newline='') as f:
        reader = csv.DictReader(f)
        for line_no, row in enumerate(reader, start=2):
            if row.get('source', 'SCRIPT') != 'SCRIPT':
                continue
            if str(row.get('file_id', '')).strip() != str(file_id):
                continue
            translated = row.get('translated_text', '').strip()
            if not translated:
                continue
            rows.append(TranslationRow(
                source='SCRIPT',
                file_id=file_id,
                offset=parse_offset(row.get('offset', '0')),
                original_text=row.get('original_text', ''),
                translated_text=translated,
                csv_line=line_no,
            ))
    return rows


def maybe_consume_trailing_punctuation(segment_text: str, end_char: int, translated: str) -> int:
    """
    The CSV sometimes separates a phrase without including the final Japanese punctuation.
    If the translation already ends in punctuation and there is Japanese punctuation right
    after the original, consuming it avoids duplicates like "………".
    """
    if not translated or translated[-1] not in TRAILING_PUNCT:
        return end_char
    pos = end_char
    while pos < len(segment_text) and segment_text[pos] in TRAILING_PUNCT:
        pos += 1
    return pos


def apply_rows_to_segment(segment: TextSegment, rows: list[TranslationRow],
                          consume_punctuation: bool = True,
                          glyph_map: dict | None = None) -> tuple[str, list[dict]]:
    """
    Applies replacements to the decoded text of the segment, from right to left
    so that original offsets do not shift.
    """
    text = segment.text
    events = []
    replacements = []

    for row in rows:
        rel_bytes = row.offset - segment.start
        if rel_bytes < 0 or rel_bytes % 2:
            raise ValueError(f"CSV row {row.csv_line}: invalid offset/alignment")
        start_char = rel_bytes // 2
        old = row.original_text
        end_char = start_char + len(old)
        if text[start_char:end_char] != old:
            got = text[start_char:end_char]
            raise ValueError(
                f"CSV row {row.csv_line}: original does not match at 0x{row.offset:X}: "
                f"CSV={old!r} DEC={got!r}"
            )

        replace_end = end_char
        if consume_punctuation:
            replace_end = maybe_consume_trailing_punctuation(text, end_char, row.translated_text)

        replacements.append((start_char, replace_end, game_string(row.translated_text, glyph_map), row))
        events.append({
            'csv_line': row.csv_line,
            'offset': row.offset,
            'old': old,
            'new': row.translated_text,
            'start_char': start_char,
            'end_char': end_char,
            'consumed_chars': replace_end - end_char,
        })

    replacements.sort(key=lambda x: x[0])
    for a, b in zip(replacements, replacements[1:]):
        if a[1] > b[0]:
            raise ValueError(
                f"Overlapping replacements in segment 0x{segment.start:X}: "
                f"CSV lines {a[3].csv_line} and {b[3].csv_line}"
            )

    new_text = text
    for start_char, end_char, repl, _row in reversed(replacements):
        new_text = new_text[:start_char] + repl + new_text[end_char:]

    return new_text, events


def rebuild_local_slack(dec_data: bytes, rows: list[TranslationRow],
                        consume_punctuation: bool = True,
                        glyph_map: dict | None = None) -> tuple[bytes, dict]:
    """
    Applies translations using local padding. Returns (new_dec, report).
    """
    out = bytearray(dec_data)
    report = {
        'mode': 'local-slack',
        'input_size': len(dec_data),
        'output_size': len(dec_data),
        'rows_total': len(rows),
        'rows_applied': 0,
        'segments_modified': 0,
        'needs_shift': [],
        'segments': [],
    }

    groups: dict[int, tuple[TextSegment, list[TranslationRow]]] = {}
    for row in rows:
        seg = find_segment_containing(dec_data, row.offset, row.original_text)
        if row.offset < seg.start or row.offset >= seg.text_end:
            raise ValueError(f"CSV row {row.csv_line}: 0x{row.offset:X} does not fall in detected segment")
        if seg.start not in groups:
            groups[seg.start] = (seg, [])
        groups[seg.start][1].append(row)

    for seg_start in sorted(groups):
        segment, seg_rows = groups[seg_start]
        new_text, events = apply_rows_to_segment(segment, seg_rows, consume_punctuation, glyph_map)
        new_bytes = new_text.encode('utf-16-le')
        required = len(new_bytes) + 2

        seg_report = {
            'start': segment.start,
            'text_end': segment.text_end,
            'slack_end': segment.slack_end,
            'old_text_bytes': segment.old_text_bytes,
            'new_text_bytes': len(new_bytes),
            'capacity_bytes': segment.capacity_bytes,
            'required_bytes': required,
            'has_length_prefix': segment.has_length_prefix,
            'length_prefix_offset': segment.length_prefix_offset,
            'rows': events,
        }

        if required > segment.capacity_bytes:
            seg_report['status'] = 'needs_shift'
            report['needs_shift'].append(seg_report)
            report['segments'].append(seg_report)
            continue

        out[segment.start:segment.start + len(new_bytes)] = new_bytes
        out[segment.start + len(new_bytes):segment.start + len(new_bytes) + 2] = b'\x00\x00'
        pad_start = segment.start + len(new_bytes) + 2
        if pad_start < segment.slack_end:
            out[pad_start:segment.slack_end] = b'\x00' * (segment.slack_end - pad_start)

        if segment.has_length_prefix and segment.length_prefix_offset is not None:
            if len(new_text) > 0xFFFF:
                raise ValueError(f"Segment 0x{segment.start:X}: length exceeds u16")
            struct.pack_into('<H', out, segment.length_prefix_offset, len(new_text))

        seg_report['status'] = 'applied'
        report['segments'].append(seg_report)
        report['segments_modified'] += 1
        report['rows_applied'] += len(seg_rows)

    return bytes(out), report


def analyze_dec(dec_data: bytes, max_segments: int = 20) -> dict:
    """Light analysis: header + UTF-16 strings detectable from non-zero offsets."""
    header = []
    if len(dec_data) >= 0x20:
        header = [f"0x{struct.unpack_from('<I', dec_data, i)[0]:X}" for i in range(0, 0x20, 4)]

    segments = []
    pos = 0
    while pos + 1 < len(dec_data):
        if dec_data[pos:pos + 2] != b'\x00\x00' and (pos == 0 or dec_data[pos - 2:pos] == b'\x00\x00'):
            try:
                null_at = find_utf16_null(dec_data, pos)
                if null_at is not None and null_at > pos:
                    raw = dec_data[pos:null_at]
                    text = decode_utf16(raw)
                    jp = sum(1 for ch in text if '\u3000' <= ch <= '\u9fff')
                    if len(text) >= 3 and jp > 0:
                        slack_end = find_slack_end(dec_data, null_at)
                        has_prefix, prefix = detect_length_prefix(dec_data, pos, text)
                        segments.append(asdict(TextSegment(pos, null_at, slack_end, text[:80], has_prefix, prefix)))
                        if len(segments) >= max_segments:
                            break
                    pos = null_at + 2
                    continue
            except UnicodeDecodeError:
                pass
        pos += 2

    return {
        'size': len(dec_data),
        'is_probably_script_dialogue': is_probably_script_dialogue(dec_data),
        'header': header,
        'segments_sample': segments,
    }


def default_dec_path(file_id: int) -> Path:
    return Path('work/scripts_extraidos') / f'ID_{file_id:05d}.dec'


def main() -> int:
    parser = argparse.ArgumentParser(description='Conservative .dec script rebuilder')
    parser.add_argument('--id', type=int, required=True, help='File ID (e.g.: 7461)')
    parser.add_argument('--dec', type=str, default=None, help='Input .dec (default: work/scripts_extraidos/ID_xxxxx.dec)')
    parser.add_argument('--csv', type=str, default='textos/dialogo.csv', help='Translation CSV')
    parser.add_argument('--out', type=str, default=None, help='Output rebuilt .dec')
    parser.add_argument('--report', type=str, default=None, help='Save JSON report')
    parser.add_argument('--mode', choices=['local-slack'], default='local-slack')
    parser.add_argument('--dry-run', action='store_true', help='Do not write output, only validate and report')
    parser.add_argument('--analyze', action='store_true', help='Only show light analysis of the .dec')
    parser.add_argument('--no-consume-punctuation', action='store_true',
                        help='Do not consume trailing Japanese punctuation after replacement')
    args = parser.parse_args()

    dec_path = Path(args.dec) if args.dec else default_dec_path(args.id)
    if not dec_path.exists():
        print(f"ERROR: does not exist {dec_path}", file=sys.stderr)
        return 1
    dec_data = dec_path.read_bytes()

    if args.analyze:
        analysis = analyze_dec(dec_data)
        print(json.dumps(analysis, ensure_ascii=False, indent=2))
        if args.report:
            Path(args.report).parent.mkdir(parents=True, exist_ok=True)
            Path(args.report).write_text(json.dumps(analysis, ensure_ascii=False, indent=2), encoding='utf-8')
        return 0

    rows = load_csv_rows(Path(args.csv), args.id)
    new_data, report = rebuild_local_slack(
        dec_data,
        rows,
        consume_punctuation=not args.no_consume_punctuation,
    )

    print(f"ID {args.id}: CSV rows with translation: {report['rows_total']}")
    print(f"  Applied: {report['rows_applied']}")
    print(f"  Segments modified: {report['segments_modified']}")
    print(f"  Needs shift: {len(report['needs_shift'])}")

    if report['needs_shift']:
        for seg in report['needs_shift'][:10]:
            print(
                f"  [needs_shift] segment 0x{seg['start']:X}: "
                f"requires {seg['required_bytes']} bytes, capacity {seg['capacity_bytes']}"
            )

    if args.report:
        Path(args.report).parent.mkdir(parents=True, exist_ok=True)
        Path(args.report).write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding='utf-8')
        print(f"  Report: {args.report}")

    if args.dry_run:
        print("  Dry-run: .dec was not written")
        return 2 if report['needs_shift'] else 0

    out_path = Path(args.out) if args.out else Path('work/scripts_extraidos') / f'ID_{args.id:05d}_rebuilt.dec'
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_bytes(new_data)
    print(f"  Output: {out_path}")
    return 2 if report['needs_shift'] else 0


if __name__ == '__main__':
    raise SystemExit(main())
