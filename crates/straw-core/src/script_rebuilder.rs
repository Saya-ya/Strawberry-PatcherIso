use std::collections::BTreeMap;

use anyhow::{bail, Context, Result};

use crate::glyph_map::{game_string, GlyphMap};

const TRAILING_PUNCT: &[char] = &['…', '。', '、', '，', ',', '.', '!', '?', '！', '？'];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSegment {
    pub start: usize,
    pub text_end: usize,
    pub slack_end: usize,
    pub text: String,
    pub has_length_prefix: bool,
    pub length_prefix_offset: Option<usize>,
}

impl TextSegment {
    pub fn old_text_bytes(&self) -> usize {
        self.text_end - self.start
    }

    pub fn capacity_bytes(&self) -> usize {
        self.slack_end - self.start
    }

    pub fn max_text_bytes(&self) -> usize {
        self.capacity_bytes().saturating_sub(2)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslationRow {
    pub source: String,
    pub file_id: i32,
    pub offset: usize,
    pub original_text: String,
    pub translated_text: String,
    pub csv_line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplacementEvent {
    pub csv_line: usize,
    pub offset: usize,
    pub old: String,
    pub new: String,
    pub start_char: usize,
    pub end_char: usize,
    pub consumed_chars: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentStatus {
    Applied,
    NeedsShift,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentReport {
    pub start: usize,
    pub text_end: usize,
    pub slack_end: usize,
    pub old_text_bytes: usize,
    pub new_text_bytes: usize,
    pub capacity_bytes: usize,
    pub required_bytes: usize,
    pub has_length_prefix: bool,
    pub length_prefix_offset: Option<usize>,
    pub rows: Vec<ReplacementEvent>,
    pub status: SegmentStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebuildReport {
    pub mode: &'static str,
    pub input_size: usize,
    pub output_size: usize,
    pub rows_total: usize,
    pub rows_applied: usize,
    pub segments_modified: usize,
    pub needs_shift: Vec<SegmentReport>,
    pub segments: Vec<SegmentReport>,
}

pub fn find_utf16_null(data: &[u8], start: usize) -> Option<usize> {
    let mut pos = start;
    while pos + 1 < data.len() {
        if data[pos..pos + 2] == [0, 0] {
            return Some(pos);
        }
        pos += 2;
    }
    None
}

pub fn find_slack_end(data: &[u8], null_at: usize) -> usize {
    let mut pos = null_at + 2;
    while pos < data.len() && data[pos] == 0 {
        pos += 1;
    }
    pos
}

pub fn detect_length_prefix(data: &[u8], start: usize, text: &str) -> (bool, Option<usize>) {
    if start < 2 {
        return (false, None);
    }
    let value = u16::from_le_bytes([data[start - 2], data[start - 1]]) as usize;
    if value == text.chars().count() && value > 0 {
        (true, Some(start - 2))
    } else {
        (false, None)
    }
}

pub fn find_segment_containing(
    data: &[u8],
    offset: usize,
    original_text: &str,
) -> Result<TextSegment> {
    if offset + 1 >= data.len() || offset % 2 != 0 {
        bail!("invalid UTF-16 offset: 0x{offset:X}");
    }

    if !original_text.is_empty() && offset >= 2 {
        let original_bytes = encode_utf16le(original_text);
        let previous = u16::from_le_bytes([data[offset - 2], data[offset - 1]]) as usize;
        if previous == original_text.chars().count()
            && data.get(offset..offset + original_bytes.len()) == Some(original_bytes.as_slice())
        {
            let null_at = find_utf16_null(data, offset)
                .with_context(|| format!("missing UTF-16 terminator from 0x{offset:X}"))?;
            let text = decode_utf16le(&data[offset..null_at])?;
            return Ok(TextSegment {
                start: offset,
                text_end: null_at,
                slack_end: find_slack_end(data, null_at),
                text,
                has_length_prefix: true,
                length_prefix_offset: Some(offset - 2),
            });
        }
    }

    let mut start = offset;
    while start >= 2 && data[start - 2..start] != [0, 0] {
        start -= 2;
    }

    let null_at = find_utf16_null(data, start)
        .with_context(|| format!("could not locate containing string for 0x{offset:X}"))?;
    if null_at < offset {
        bail!("detected string terminates before offset 0x{offset:X}");
    }

    let text = decode_utf16le(&data[start..null_at])?;
    let (has_length_prefix, length_prefix_offset) = detect_length_prefix(data, start, &text);
    Ok(TextSegment {
        start,
        text_end: null_at,
        slack_end: find_slack_end(data, null_at),
        text,
        has_length_prefix,
        length_prefix_offset,
    })
}

pub fn rebuild_local_slack(
    dec_data: &[u8],
    rows: &[TranslationRow],
    consume_punctuation: bool,
    glyph_map: Option<&GlyphMap>,
) -> Result<(Vec<u8>, RebuildReport)> {
    let mut out = dec_data.to_vec();
    let mut report = RebuildReport {
        mode: "local-slack",
        input_size: dec_data.len(),
        output_size: dec_data.len(),
        rows_total: rows.len(),
        rows_applied: 0,
        segments_modified: 0,
        needs_shift: Vec::new(),
        segments: Vec::new(),
    };

    let mut groups: BTreeMap<usize, (TextSegment, Vec<TranslationRow>)> = BTreeMap::new();
    for row in rows {
        let segment = find_segment_containing(dec_data, row.offset, &row.original_text)?;
        if row.offset < segment.start || row.offset >= segment.text_end {
            bail!(
                "CSV row {} offset 0x{:X} is outside detected segment",
                row.csv_line,
                row.offset
            );
        }
        groups
            .entry(segment.start)
            .or_insert_with(|| (segment, Vec::new()))
            .1
            .push(row.clone());
    }

    for (_start, (segment, seg_rows)) in groups {
        let (new_text, events) =
            apply_rows_to_segment(&segment, &seg_rows, consume_punctuation, glyph_map)?;
        let new_bytes = encode_utf16le(&new_text);
        let required = new_bytes.len() + 2;

        let mut segment_report = SegmentReport {
            start: segment.start,
            text_end: segment.text_end,
            slack_end: segment.slack_end,
            old_text_bytes: segment.old_text_bytes(),
            new_text_bytes: new_bytes.len(),
            capacity_bytes: segment.capacity_bytes(),
            required_bytes: required,
            has_length_prefix: segment.has_length_prefix,
            length_prefix_offset: segment.length_prefix_offset,
            rows: events,
            status: SegmentStatus::Applied,
        };

        if required > segment.capacity_bytes() {
            segment_report.status = SegmentStatus::NeedsShift;
            report.needs_shift.push(segment_report.clone());
            report.segments.push(segment_report);
            continue;
        }

        out[segment.start..segment.start + new_bytes.len()].copy_from_slice(&new_bytes);
        out[segment.start + new_bytes.len()..segment.start + new_bytes.len() + 2]
            .copy_from_slice(&[0, 0]);
        let pad_start = segment.start + new_bytes.len() + 2;
        if pad_start < segment.slack_end {
            out[pad_start..segment.slack_end].fill(0);
        }

        if segment.has_length_prefix {
            if new_text.chars().count() > u16::MAX as usize {
                bail!("segment 0x{:X} length exceeds u16", segment.start);
            }
            if let Some(prefix_offset) = segment.length_prefix_offset {
                out[prefix_offset..prefix_offset + 2]
                    .copy_from_slice(&(new_text.chars().count() as u16).to_le_bytes());
            }
        }

        report.segments_modified += 1;
        report.rows_applied += seg_rows.len();
        report.segments.push(segment_report);
    }

    Ok((out, report))
}

pub fn rebuild_shift_suffix(
    dec_data: &[u8],
    rows: &[TranslationRow],
    consume_punctuation: bool,
    glyph_map: Option<&GlyphMap>,
) -> Result<(Vec<u8>, RebuildReport)> {
    let mut report = RebuildReport {
        mode: "shift-suffix",
        input_size: dec_data.len(),
        output_size: dec_data.len(),
        rows_total: rows.len(),
        rows_applied: 0,
        segments_modified: 0,
        needs_shift: Vec::new(),
        segments: Vec::new(),
    };

    let mut groups: BTreeMap<usize, (TextSegment, Vec<TranslationRow>)> = BTreeMap::new();
    for row in rows {
        let segment = find_segment_containing(dec_data, row.offset, &row.original_text)?;
        if row.offset < segment.start || row.offset >= segment.text_end {
            bail!(
                "CSV row {} offset 0x{:X} is outside detected segment",
                row.csv_line,
                row.offset
            );
        }
        groups
            .entry(segment.start)
            .or_insert_with(|| (segment, Vec::new()))
            .1
            .push(row.clone());
    }

    struct Action {
        segment: TextSegment,
        rows: Vec<TranslationRow>,
        new_bytes: Vec<u8>,
        report: SegmentReport,
        extra_bytes: usize,
        inline_record: Option<InlineRecord>,
    }

    let mut actions = Vec::new();
    for (_start, (segment, seg_rows)) in groups {
        let (new_text, events) =
            apply_rows_to_segment(&segment, &seg_rows, consume_punctuation, glyph_map)?;
        let new_bytes = encode_utf16le(&new_text);
        let required = new_bytes.len() + 2;
        let mut segment_report = SegmentReport {
            start: segment.start,
            text_end: segment.text_end,
            slack_end: segment.slack_end,
            old_text_bytes: segment.old_text_bytes(),
            new_text_bytes: new_bytes.len(),
            capacity_bytes: segment.capacity_bytes(),
            required_bytes: required,
            has_length_prefix: segment.has_length_prefix,
            length_prefix_offset: segment.length_prefix_offset,
            rows: events,
            status: SegmentStatus::Applied,
        };

        let (extra_bytes, inline_record) = if required <= segment.capacity_bytes() {
            (0, None)
        } else if let Some(record) = detect_inline_text_record(dec_data, &segment) {
            let extra = align_even(required - segment.capacity_bytes());
            (extra, Some(record))
        } else {
            segment_report.status = SegmentStatus::NeedsShift;
            report.needs_shift.push(segment_report.clone());
            report.segments.push(segment_report);
            continue;
        };

        actions.push(Action {
            segment,
            rows: seg_rows,
            new_bytes,
            report: segment_report,
            extra_bytes,
            inline_record,
        });
    }

    let mut out = dec_data.to_vec();
    actions.sort_by_key(|action| action.segment.start);
    for action in actions.into_iter().rev() {
        let segment = action.segment;
        if action.extra_bytes > 0 {
            let record = action
                .inline_record
                .context("missing inline record for shift action")?;
            out.splice(
                segment.slack_end..segment.slack_end,
                std::iter::repeat(0).take(action.extra_bytes),
            );
            let new_record_len = record
                .length
                .checked_add(action.extra_bytes)
                .context("inline text record length overflow")?;
            if new_record_len > u32::MAX as usize {
                bail!(
                    "inline text record at 0x{:X} exceeds u32 length",
                    record.start
                );
            }
            out[record.length_offset..record.length_offset + 4]
                .copy_from_slice(&(new_record_len as u32).to_le_bytes());
        }

        out[segment.start..segment.start + action.new_bytes.len()]
            .copy_from_slice(&action.new_bytes);
        out[segment.start + action.new_bytes.len()..segment.start + action.new_bytes.len() + 2]
            .copy_from_slice(&[0, 0]);
        let pad_start = segment.start + action.new_bytes.len() + 2;
        let pad_end = segment.slack_end + action.extra_bytes;
        if pad_start < pad_end {
            out[pad_start..pad_end].fill(0);
        }

        if segment.has_length_prefix {
            let char_count = String::from_utf16(
                &action
                    .new_bytes
                    .chunks_exact(2)
                    .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
                    .collect::<Vec<_>>(),
            )?
            .chars()
            .count();
            if char_count > u16::MAX as usize {
                bail!("segment 0x{:X} length exceeds u16", segment.start);
            }
            if let Some(prefix_offset) = segment.length_prefix_offset {
                out[prefix_offset..prefix_offset + 2]
                    .copy_from_slice(&(char_count as u16).to_le_bytes());
            }
        }

        report.segments_modified += 1;
        report.rows_applied += action.rows.len();
        report.segments.push(action.report);
    }

    report.segments.sort_by_key(|segment| segment.start);
    report.output_size = out.len();
    Ok((out, report))
}

#[derive(Debug, Clone, Copy)]
struct InlineRecord {
    start: usize,
    length_offset: usize,
    length: usize,
}

fn detect_inline_text_record(data: &[u8], segment: &TextSegment) -> Option<InlineRecord> {
    const TEXT_RECORD_SIG_PREFIX: [u8; 4] = [3, 0, 0, 0];
    const TEXT_RECORD_SIG_SUFFIX: [u8; 8] = [0, 0, 0, 0, 0, 0, 0, 0];

    let record_start = segment.start.checked_sub(16)?;
    let length_offset = record_start.checked_add(4)?;
    let suffix_offset = record_start.checked_add(8)?;
    if data.get(record_start..record_start + 4) != Some(&TEXT_RECORD_SIG_PREFIX) {
        return None;
    }
    if data.get(suffix_offset..suffix_offset + 8) != Some(&TEXT_RECORD_SIG_SUFFIX) {
        return None;
    }
    let length = u32::from_le_bytes(
        data.get(length_offset..length_offset + 4)?
            .try_into()
            .ok()?,
    ) as usize;
    let record_end = segment.start.checked_add(length)?;
    if record_end > data.len() || segment.slack_end > record_end || segment.text_end > record_end {
        return None;
    }
    Some(InlineRecord {
        start: record_start,
        length_offset,
        length,
    })
}

fn align_even(value: usize) -> usize {
    value + (value % 2)
}

fn apply_rows_to_segment(
    segment: &TextSegment,
    rows: &[TranslationRow],
    consume_punctuation: bool,
    glyph_map: Option<&GlyphMap>,
) -> Result<(String, Vec<ReplacementEvent>)> {
    let text_chars: Vec<char> = segment.text.chars().collect();
    let mut replacements = Vec::new();

    for row in rows {
        let rel_bytes = row
            .offset
            .checked_sub(segment.start)
            .with_context(|| format!("CSV row {} offset before segment", row.csv_line))?;
        if rel_bytes % 2 != 0 {
            bail!("CSV row {} has unaligned offset", row.csv_line);
        }
        let start_char = rel_bytes / 2;
        let old_chars: Vec<char> = row.original_text.chars().collect();
        let end_char = start_char + old_chars.len();
        if text_chars.get(start_char..end_char) != Some(old_chars.as_slice()) {
            let got: String = text_chars
                .get(start_char..end_char)
                .unwrap_or_default()
                .iter()
                .collect();
            bail!(
                "CSV row {} original mismatch at 0x{:X}: CSV={:?} DEC={:?}",
                row.csv_line,
                row.offset,
                row.original_text,
                got
            );
        }

        let replace_end = if consume_punctuation {
            maybe_consume_trailing_punctuation(&text_chars, end_char, &row.translated_text)
        } else {
            end_char
        };
        let replacement = game_string(&row.translated_text, glyph_map);
        replacements.push((
            start_char,
            replace_end,
            end_char,
            replacement,
            row.csv_line,
            row.offset,
            row.original_text.clone(),
            row.translated_text.clone(),
        ));
    }

    // Sort by start_char ascending, then by length (end_char) descending,
    // so broader containing intervals come before any subsumed sub-intervals.
    replacements.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| b.2.cmp(&a.2))
            .then_with(|| b.1.cmp(&a.1))
    });

    let mut accepted = Vec::new();
    for item in replacements {
        let (start, replace_end, end, repl, line, offset, old, new) = item;
        if let Some(prev) = accepted.iter().find(
            |p: &&(usize, usize, usize, String, usize, usize, String, String)| {
                p.0 <= start && end <= p.2
            },
        ) {
            eprintln!(
                "[script_rebuilder] Omitiendo reemplazo solapado en segmento 0x{:X}: fila {} (offset 0x{:X}) está contenida dentro de la fila {} (offset 0x{:X})",
                segment.start, line, offset, prev.4, prev.5
            );
            continue;
        }
        accepted.push((start, replace_end, end, repl, line, offset, old, new));
    }

    // Ensure trailing punctuation consumption does not spill over into the next accepted row
    for i in 0..accepted.len().saturating_sub(1) {
        let next_start = accepted[i + 1].0;
        if accepted[i].1 > next_start && accepted[i].2 <= next_start {
            accepted[i].1 = next_start;
        }
    }

    // Reject any remaining genuine, incompatible partial overlaps
    for pair in accepted.windows(2) {
        if pair[0].1 > pair[1].0 {
            bail!(
                "overlapping replacements in segment 0x{:X}: rows {} and {}",
                segment.start,
                pair[0].4,
                pair[1].4
            );
        }
    }

    let mut events = Vec::new();
    let mut final_replacements = Vec::new();
    for (start, replace_end, end, repl, line, offset, old, new) in accepted {
        events.push(ReplacementEvent {
            csv_line: line,
            offset,
            old,
            new,
            start_char: start,
            end_char: end,
            consumed_chars: replace_end - end,
        });
        final_replacements.push((start, replace_end, repl, line));
    }

    let mut new_chars = text_chars;
    for (start, end, replacement, _line) in final_replacements.into_iter().rev() {
        new_chars.splice(start..end, replacement.chars());
    }

    Ok((new_chars.into_iter().collect(), events))
}

fn maybe_consume_trailing_punctuation(
    segment_text: &[char],
    end_char: usize,
    translated: &str,
) -> usize {
    let Some(last) = translated.chars().last() else {
        return end_char;
    };
    if !TRAILING_PUNCT.contains(&last) {
        return end_char;
    }

    let mut pos = end_char;
    while pos < segment_text.len() && TRAILING_PUNCT.contains(&segment_text[pos]) {
        pos += 1;
    }
    pos
}

fn decode_utf16le(data: &[u8]) -> Result<String> {
    if data.len() % 2 != 0 {
        bail!("odd-length UTF-16LE data");
    }
    let units: Vec<u16> = data
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();
    String::from_utf16(&units).context("invalid UTF-16LE text")
}

fn encode_utf16le(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glyph_map::spanish_glyph_map;

    #[test]
    fn finds_segment_containing_offset() {
        let data = dec_with_text("こんにちは", 4);
        let segment = find_segment_containing(&data, 0, "こんにちは").unwrap();
        assert_eq!(segment.start, 0);
        assert_eq!(segment.text, "こんにちは");
        assert_eq!(
            segment.capacity_bytes(),
            "こんにちは".encode_utf16().count() * 2 + 2 + 4
        );
    }

    #[test]
    fn rebuilds_text_within_local_slack() {
        let data = dec_with_text("あい", 8);
        let row = TranslationRow {
            source: "SCRIPT".to_owned(),
            file_id: 1,
            offset: 0,
            original_text: "あい".to_owned(),
            translated_text: "Hola".to_owned(),
            csv_line: 2,
        };

        let (rebuilt, report) = rebuild_local_slack(&data, &[row], true, None).unwrap();
        assert_eq!(report.rows_applied, 1);
        assert_eq!(report.segments_modified, 1);
        assert!(report.needs_shift.is_empty());
        assert_eq!(decode_utf16le(&rebuilt[0..8]).unwrap(), "Hola");
    }

    #[test]
    fn reports_needs_shift_without_modifying_data() {
        let data = dec_with_text("あ", 0);
        let row = TranslationRow {
            source: "SCRIPT".to_owned(),
            file_id: 1,
            offset: 0,
            original_text: "あ".to_owned(),
            translated_text: "Demasiado largo".to_owned(),
            csv_line: 2,
        };

        let (rebuilt, report) = rebuild_local_slack(&data, &[row], true, None).unwrap();
        assert_eq!(rebuilt, data);
        assert_eq!(report.needs_shift.len(), 1);
        assert_eq!(report.rows_applied, 0);
    }

    #[test]
    fn updates_length_prefix() {
        let mut data = Vec::new();
        data.extend(2_u16.to_le_bytes());
        data.extend(encode_utf16le("あい"));
        data.extend([0, 0, 0, 0, 0, 0, 0, 0]);

        let row = TranslationRow {
            source: "SCRIPT".to_owned(),
            file_id: 1,
            offset: 2,
            original_text: "あい".to_owned(),
            translated_text: "Hey".to_owned(),
            csv_line: 2,
        };

        let (rebuilt, _report) = rebuild_local_slack(&data, &[row], true, None).unwrap();
        assert_eq!(u16::from_le_bytes([rebuilt[0], rebuilt[1]]), 3);
    }

    #[test]
    fn applies_glyph_map() {
        let data = dec_with_text("あい", 8);
        let row = TranslationRow {
            source: "SCRIPT".to_owned(),
            file_id: 1,
            offset: 0,
            original_text: "あい".to_owned(),
            translated_text: "año".to_owned(),
            csv_line: 2,
        };

        let map = spanish_glyph_map();
        let (rebuilt, _report) = rebuild_local_slack(&data, &[row], true, Some(&map)).unwrap();
        assert_eq!(decode_utf16le(&rebuilt[0..6]).unwrap(), "aИo");
    }

    #[test]
    fn local_slack_preserves_nonzero_suffix() {
        let data = inline_record_with_suffix("あい", 0x40, &[1, 0, 6, 0, 0xAA]);
        let suffix_start = data.iter().rposition(|byte| *byte == 0xAA).unwrap() - 4;
        let suffix = data[suffix_start..0x20 + 0x40].to_vec();
        let row = TranslationRow {
            source: "SCRIPT".to_owned(),
            file_id: 1,
            offset: 0x20,
            original_text: "あい".to_owned(),
            translated_text: "Hola".to_owned(),
            csv_line: 2,
        };

        let (rebuilt, report) = rebuild_local_slack(&data, &[row], true, None).unwrap();

        assert_eq!(report.needs_shift.len(), 0);
        assert_eq!(&rebuilt[suffix_start..0x20 + 0x40], suffix.as_slice());
        assert_eq!(
            u32::from_le_bytes(rebuilt[0x14..0x18].try_into().unwrap()),
            0x40
        );
    }

    #[test]
    fn shift_suffix_extends_inline_record_without_overwriting_suffix() {
        let data = inline_record_with_suffix("あい", 0x40, &[1, 0, 6, 0, 0xAA]);
        let old_suffix_start = data.iter().rposition(|byte| *byte == 0xAA).unwrap() - 4;
        let suffix = data[old_suffix_start..0x20 + 0x40].to_vec();
        let row = TranslationRow {
            source: "SCRIPT".to_owned(),
            file_id: 1,
            offset: 0x20,
            original_text: "あい".to_owned(),
            translated_text: "Texto mucho mas largo que supera el slack local".to_owned(),
            csv_line: 2,
        };

        let translated_text = row.translated_text.clone();
        let (rebuilt, report) = rebuild_shift_suffix(&data, &[row], true, None).unwrap();

        let new_len = u32::from_le_bytes(rebuilt[0x14..0x18].try_into().unwrap()) as usize;
        let new_suffix_start = rebuilt.iter().rposition(|byte| *byte == 0xAA).unwrap() - 4;
        let new_text_bytes = encode_utf16le(&translated_text);
        assert!(new_len > 0x40);
        assert_eq!(report.needs_shift.len(), 0);
        assert_eq!(
            &rebuilt[new_suffix_start..0x20 + new_len],
            suffix.as_slice()
        );
        assert_eq!(
            decode_utf16le(&rebuilt[0x20..0x20 + new_text_bytes.len()]).unwrap(),
            translated_text
        );
    }

    #[test]
    fn consumes_trailing_punctuation_when_translation_keeps_punctuation() {
        let data = dec_with_text("あい。", 8);
        let row = TranslationRow {
            source: "SCRIPT".to_owned(),
            file_id: 1,
            offset: 0,
            original_text: "あい".to_owned(),
            translated_text: "Hola.".to_owned(),
            csv_line: 2,
        };

        let (rebuilt, report) = rebuild_local_slack(&data, &[row], true, None).unwrap();
        assert_eq!(decode_utf16le(&rebuilt[0..10]).unwrap(), "Hola.");
        assert_eq!(report.segments[0].rows[0].consumed_chars, 1);
    }

    #[test]
    fn keeps_trailing_punctuation_when_translation_has_no_punctuation() {
        let data = dec_with_text("あい。", 8);
        let row = TranslationRow {
            source: "SCRIPT".to_owned(),
            file_id: 1,
            offset: 0,
            original_text: "あい".to_owned(),
            translated_text: "Hola".to_owned(),
            csv_line: 2,
        };

        let (rebuilt, report) = rebuild_local_slack(&data, &[row], true, None).unwrap();
        assert_eq!(decode_utf16le(&rebuilt[0..10]).unwrap(), "Hola。");
        assert_eq!(report.segments[0].rows[0].consumed_chars, 0);
    }

    #[test]
    fn rejects_overlapping_replacements() {
        let data = dec_with_text("abcdef", 8);
        let rows = [
            TranslationRow {
                source: "SCRIPT".to_owned(),
                file_id: 1,
                offset: 0,
                original_text: "abc".to_owned(),
                translated_text: "ABC".to_owned(),
                csv_line: 2,
            },
            TranslationRow {
                source: "SCRIPT".to_owned(),
                file_id: 1,
                offset: 4,
                original_text: "cde".to_owned(),
                translated_text: "CDE".to_owned(),
                csv_line: 3,
            },
        ];

        let err = rebuild_local_slack(&data, &rows, false, None).unwrap_err();
        assert!(err.to_string().contains("overlapping replacements"));
    }

    #[test]
    fn auto_resolves_subsumed_overlapping_replacements() {
        let data = dec_with_text("abcdef", 8);
        let rows = [
            TranslationRow {
                source: "SCRIPT".to_owned(),
                file_id: 1,
                offset: 0,
                original_text: "abcdef".to_owned(),
                translated_text: "ABCDEF".to_owned(),
                csv_line: 2,
            },
            TranslationRow {
                source: "SCRIPT".to_owned(),
                file_id: 1,
                offset: 4,
                original_text: "cde".to_owned(),
                translated_text: "CDE".to_owned(),
                csv_line: 3,
            },
        ];

        let (rebuilt_data, report) = rebuild_local_slack(&data, &rows, false, None)
            .expect("should auto-resolve subsumed row");
        let decoded = decode_utf16le(&rebuilt_data[..report.segments[0].text_end]).unwrap();
        assert!(decoded.starts_with("ABCDEF"));
        assert_eq!(report.segments[0].rows.len(), 1);
        assert_eq!(report.segments[0].rows[0].csv_line, 2);
    }

    #[test]
    fn rejects_unaligned_offsets() {
        let data = dec_with_text("abc", 8);

        let err = find_segment_containing(&data, 1, "abc").unwrap_err();
        assert!(err.to_string().contains("invalid UTF-16 offset"));
    }

    fn dec_with_text(text: &str, slack: usize) -> Vec<u8> {
        let mut data = encode_utf16le(text);
        data.extend([0, 0]);
        data.extend(std::iter::repeat(0).take(slack));
        data.push(0xFF);
        data
    }

    fn inline_record_with_suffix(text: &str, record_len: usize, suffix: &[u8]) -> Vec<u8> {
        let mut data = vec![0; 0x10];
        data.extend([3, 0, 0, 0]);
        data.extend((record_len as u32).to_le_bytes());
        data.extend([0; 8]);
        data.extend(encode_utf16le(text));
        data.extend([0, 0]);
        let record_end = 0x10 + 0x10 + record_len;
        let suffix_start = record_end - suffix.len();
        data.resize(suffix_start, 0);
        data.extend(suffix);
        data
    }
}
