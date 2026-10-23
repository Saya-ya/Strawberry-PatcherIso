use std::{
    collections::BTreeMap,
    fs,
    io::{Seek, SeekFrom, Write},
    path::Path,
    thread,
};

use anyhow::{bail, Context, Result};

use crate::{
    datafat::{find_row, parse_entries_from_data, size_field_write_offset, slot_capacity},
    dialogue::classify_script_rebuild_mode,
    glyph_map::GlyphMap,
    lz77::{compress_lz77, decompress_lz77},
    script_rebuilder::{rebuild_local_slack, rebuild_shift_suffix, SegmentReport, TranslationRow},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchScriptsReport {
    pub scripts_total: usize,
    pub scripts_patched: usize,
    pub rows_total: usize,
    pub rows_applied: usize,
    pub errors: Vec<String>,
}

pub fn patch_translated_scripts(
    data_bin_path: impl AsRef<Path>,
    dec_dir: impl AsRef<Path>,
    csv_path: impl AsRef<Path>,
    glyph_map: Option<&GlyphMap>,
) -> Result<PatchScriptsReport> {
    patch_translated_scripts_with_options(data_bin_path, dec_dir, csv_path, glyph_map, 1)
}

/// Same as [`patch_translated_scripts`], but spreads the CPU-heavy
/// decompress/rebuild/compress work across `workers` threads.
///
/// Scripts are independent units of work, so each worker rebuilds and compresses
/// a script without touching `Data.bin`. The compressed results are then applied
/// to the shared buffer sequentially, which keeps output byte-for-byte identical
/// to the sequential implementation and avoids any aliasing between slots.
/// `workers <= 1` falls back to the sequential path.
pub fn patch_translated_scripts_with_options(
    data_bin_path: impl AsRef<Path>,
    dec_dir: impl AsRef<Path>,
    csv_path: impl AsRef<Path>,
    glyph_map: Option<&GlyphMap>,
    workers: usize,
) -> Result<PatchScriptsReport> {
    let data_bin_path = data_bin_path.as_ref();
    let dec_dir = dec_dir.as_ref();
    let csv_path = csv_path.as_ref();
    let rows_by_script = load_translation_rows(csv_path)?;
    let mut data = fs::read(data_bin_path)
        .with_context(|| format!("failed to read {}", data_bin_path.display()))?;
    let fat_rows = parse_entries_from_data(&data)?;

    let mut report = PatchScriptsReport {
        scripts_total: rows_by_script.len(),
        scripts_patched: 0,
        rows_total: rows_by_script.values().map(Vec::len).sum(),
        rows_applied: 0,
        errors: Vec::new(),
    };

    // Phase 1 (parallel): rebuild + compress each script independently. Workers
    // only read their own `.dec` file; nothing touches `data` yet.
    let jobs: Vec<(i32, Vec<TranslationRow>)> = rows_by_script.into_iter().collect();
    let mut rebuilt: BTreeMap<i32, Result<PatchedScript>> = BTreeMap::new();
    let chunk_size = jobs.len().div_ceil(workers.max(1)).max(1);
    // Owned chunks let each worker `move` its slice without aliasing `data`.
    let chunks: Vec<Vec<(i32, Vec<TranslationRow>)>> =
        jobs.chunks(chunk_size).map(<[(i32, Vec<TranslationRow>)]>::to_vec).collect();
    thread::scope(|scope| {
        let mut handles = Vec::new();
        for chunk in chunks {
            handles.push(scope.spawn(move || {
                let mut local: Vec<(i32, Result<PatchedScript>)> = Vec::new();
                for (script_id, rows) in &chunk {
                    let result = rebuild_script(dec_dir, *script_id, rows, glyph_map);
                    local.push((*script_id, result));
                }
                local
            }));
        }
        for handle in handles {
            match handle.join() {
                Ok(partial) => {
                    for (script_id, result) in partial {
                        rebuilt.insert(script_id, result);
                    }
                }
                Err(_) => {
                    report.errors.push("script worker thread panicked".to_owned());
                }
            }
        }
    });

    // Phase 2 (sequential): write compressed streams into the shared buffer.
    for (script_id, result) in rebuilt {
        match result {
            Ok(patched) => match apply_patched_script(&mut data, &fat_rows, script_id, patched) {
                Ok(rows_applied) => {
                    report.scripts_patched += 1;
                    report.rows_applied += rows_applied;
                }
                Err(err) => report.errors.push(format!("ID {script_id}: {err}")),
            },
            Err(err) => report.errors.push(format!("ID {script_id}: {err}")),
        }
    }

    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(data_bin_path)
        .with_context(|| format!("failed to open {}", data_bin_path.display()))?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&data)?;
    file.flush()?;

    Ok(report)
}

/// A compressed script ready to be written into `Data.bin`.
struct PatchedScript {
    compressed: Vec<u8>,
    rows_applied: usize,
}

/// Rebuilds and compresses one script. Pure function of its `.dec` file and
/// translation rows, so it is safe to run on worker threads.
fn rebuild_script(
    dec_dir: &Path,
    script_id: i32,
    rows: &[TranslationRow],
    glyph_map: Option<&GlyphMap>,
) -> Result<PatchedScript> {
    if script_id < 0 {
        bail!("ELF rows are not script rows");
    }
    let file_id = script_id as u32;
    let dec_path = dec_dir.join(format!("ID_{file_id:05}.dec"));
    let dec_data =
        fs::read(&dec_path).with_context(|| format!("failed to read {}", dec_path.display()))?;
    let rebuild_mode = classify_script_rebuild_mode(&dec_data).unwrap_or_default();
    let (rebuilt, rebuild_report) = if rebuild_mode == "shift_suffix_safe" {
        rebuild_shift_suffix(&dec_data, rows, true, glyph_map)?
    } else {
        rebuild_local_slack(&dec_data, rows, true, glyph_map)?
    };
    if !rebuild_report.needs_shift.is_empty() {
        bail!(format_needs_shift(&rebuild_report.needs_shift));
    }

    let compressed = compress_lz77(&rebuilt, false);
    let redecompressed = decompress_lz77(&compressed, None, true)?;
    if redecompressed != rebuilt {
        bail!("compressed roundtrip mismatch");
    }

    Ok(PatchedScript {
        compressed,
        rows_applied: rebuild_report.rows_applied,
    })
}

/// Writes a pre-compressed script into the shared buffer and updates its size
/// field. Runs sequentially so slot writes never race each other.
fn apply_patched_script(
    data: &mut [u8],
    fat_rows: &[crate::datafat::FatEntry],
    script_id: i32,
    patched: PatchedScript,
) -> Result<usize> {
    let file_id = script_id as u32;
    let target = find_row(fat_rows, file_id).with_context(|| "file ID not found in FAT")?;
    let capacity =
        slot_capacity(fat_rows, target).with_context(|| "could not compute slot capacity")?;
    if patched.compressed.len() > capacity as usize {
        bail!(
            "compressed stream does not fit ({} > {})",
            patched.compressed.len(),
            capacity
        );
    }

    let start = target.offset as usize;
    let end = start + capacity as usize;
    if end > data.len() {
        bail!("target slot outside Data.bin");
    }
    data[start..start + patched.compressed.len()].copy_from_slice(&patched.compressed);
    data[start + patched.compressed.len()..end].fill(0);

    let size_offset = size_field_write_offset(target);
    data[size_offset..size_offset + 4]
        .copy_from_slice(&(patched.compressed.len() as u32).to_le_bytes());

    Ok(patched.rows_applied)
}

fn load_translation_rows(csv_path: &Path) -> Result<BTreeMap<i32, Vec<TranslationRow>>> {
    let mut reader = csv::Reader::from_path(csv_path)
        .with_context(|| format!("failed to read {}", csv_path.display()))?;
    let mut rows_by_script: BTreeMap<i32, Vec<TranslationRow>> = BTreeMap::new();

    for (idx, row) in reader.records().enumerate() {
        let row = row?;
        let source = row.get(0).unwrap_or("SCRIPT").to_owned();
        if source != "SCRIPT" {
            continue;
        }
        let file_id = row.get(1).unwrap_or_default();
        let Ok(script_id) = file_id.parse::<i32>() else {
            continue;
        };
        let translated_text = row.get(4).unwrap_or_default().trim().to_owned();
        if translated_text.is_empty() {
            continue;
        }
        let offset = parse_offset(row.get(2).unwrap_or("0"))?;
        rows_by_script
            .entry(script_id)
            .or_default()
            .push(TranslationRow {
                source,
                file_id: script_id,
                offset,
                original_text: row.get(3).unwrap_or_default().to_owned(),
                translated_text,
                csv_line: idx + 2,
            });
    }

    Ok(rows_by_script)
}

fn parse_offset(value: &str) -> Result<usize> {
    let value = value.trim();
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        Ok(usize::from_str_radix(hex, 16)?)
    } else {
        Ok(value.parse()?)
    }
}

fn format_needs_shift(segments: &[SegmentReport]) -> String {
    let mut details = format!("{} segment(s) need shift", segments.len());
    for segment in segments {
        details.push_str(&format!(
            "\n  Segmento 0x{:X}..0x{:X}: requerido {} bytes (texto {} + terminador), capacidad local {} bytes; texto original {} bytes; prefijo de longitud: {}",
            segment.start,
            segment.slack_end,
            segment.required_bytes,
            segment.new_text_bytes,
            segment.capacity_bytes,
            segment.old_text_bytes,
            segment.has_length_prefix
        ));
        for row in &segment.rows {
            details.push_str(&format!(
                "\n    CSV línea {}, offset 0x{:X}: original={:?}; traducción={:?}",
                row.csv_line, row.offset, row.old, row.new
            ));
        }
    }
    details
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script_rebuilder::{ReplacementEvent, SegmentStatus};

    #[test]
    fn formats_shift_failures_with_locations_and_text() {
        let segments = [SegmentReport {
            start: 0x20,
            text_end: 0x30,
            slack_end: 0x34,
            old_text_bytes: 16,
            new_text_bytes: 20,
            capacity_bytes: 20,
            required_bytes: 22,
            has_length_prefix: false,
            length_prefix_offset: None,
            rows: vec![ReplacementEvent {
                csv_line: 7,
                offset: 0x24,
                old: "前の文".to_owned(),
                new: "Texto nuevo".to_owned(),
                start_char: 2,
                end_char: 5,
                consumed_chars: 0,
            }],
            status: SegmentStatus::NeedsShift,
        }];

        let message = format_needs_shift(&segments);
        assert!(message.contains("Segmento 0x20..0x34"));
        assert!(message.contains("requerido 22 bytes"));
        assert!(message.contains("CSV línea 7, offset 0x24"));
        assert!(message.contains("original=\"前の文\""));
        assert!(message.contains("traducción=\"Texto nuevo\""));
    }

    #[test]
    fn parses_translation_rows_from_csv() {
        let temp = tempfile::tempdir().unwrap();
        let csv_path = temp.path().join("dialogo.csv");
        fs::write(
            &csv_path,
            "source,file_id,offset,original_text,translated_text\r\nSCRIPT,7,0x10,a,b\r\nELF,ELF,0x20,c,d\r\n",
        )
        .unwrap();

        let rows = load_translation_rows(&csv_path).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[&7][0].offset, 0x10);
        assert_eq!(rows[&7][0].translated_text, "b");
    }
}
