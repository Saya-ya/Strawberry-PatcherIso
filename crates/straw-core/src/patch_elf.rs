use std::{
    collections::HashMap,
    fs,
    io::{Seek, SeekFrom, Write},
    path::Path,
};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::{copy_file_creating_parent, encode_game_sjis, extract_elf_strings, GlyphMap};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchElfReport {
    pub rows_processed: usize,
    pub rows_patched: usize,
    pub skipped_too_large: usize,
    pub skipped_other: usize,
    pub bytes_written: usize,
}

#[derive(Debug, Deserialize)]
struct TranslationRow {
    source: String,
    offset: String,
    original_text: String,
    translated_text: String,
}

pub fn patch_translated_elf(
    original_elf: impl AsRef<Path>,
    translated_elf: impl AsRef<Path>,
    csv_path: impl AsRef<Path>,
    glyph_map: Option<&GlyphMap>,
) -> Result<PatchElfReport> {
    let original_elf = original_elf.as_ref();
    let translated_elf = translated_elf.as_ref();
    let csv_path = csv_path.as_ref();

    if !original_elf.exists() {
        bail!("original ELF not found: {}", original_elf.display());
    }
    if !csv_path.exists() {
        bail!("build CSV not found: {}", csv_path.display());
    }

    // Always begin with the original; an older output of the same length may
    // contain patches that were removed from the translation database.
    copy_file_creating_parent(original_elf, translated_elf)?;
    let original = fs::read(original_elf)?;
    let valid_fields: HashMap<_, _> = extract_elf_strings(original_elf)?
        .into_iter()
        .map(|text| (text.byte_offset as u64, text))
        .collect();

    let mut reader = csv::Reader::from_path(csv_path)
        .with_context(|| format!("failed to read {}", csv_path.display()))?;
    let rows = reader
        .deserialize::<TranslationRow>()
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let mut report = PatchElfReport {
        rows_processed: 0,
        rows_patched: 0,
        skipped_too_large: 0,
        skipped_other: 0,
        bytes_written: 0,
    };
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(translated_elf)
        .with_context(|| format!("failed to open {}", translated_elf.display()))?;

    for row in rows {
        if row.source != "ELF" || row.translated_text.trim().is_empty() {
            continue;
        }
        report.rows_processed += 1;

        let Some(offset) = parse_hex_offset(&row.offset) else {
            report.skipped_other += 1;
            continue;
        };
        let Some(field) = valid_fields.get(&offset) else {
            report.skipped_other += 1;
            continue;
        };
        if field.original_text != row.original_text {
            report.skipped_other += 1;
            continue;
        }
        let new_bytes = encode_game_sjis(row.translated_text.trim(), glyph_map);
        let field_size = field.original_bytes as usize;
        if new_bytes.len() > field_size {
            report.skipped_too_large += 1;
            continue;
        }
        let start = offset as usize;
        if original.get(start..start + field_size).is_none() {
            report.skipped_other += 1;
            continue;
        }

        file.seek(SeekFrom::Start(offset))?;
        file.write_all(&new_bytes)?;
        let padding = field_size - new_bytes.len();
        if padding > 0 {
            file.write_all(&vec![0x20; padding])?;
        }
        report.rows_patched += 1;
        report.bytes_written += field_size;
    }

    file.flush()?;
    Ok(report)
}

fn parse_hex_offset(value: &str) -> Option<u64> {
    let value = value.trim();
    let value = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    u64::from_str_radix(value, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patches_elf_rows_from_csv() {
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("SLPS_256.11");
        let translated = temp.path().join("SLPS_256.11_translated");
        let csv = temp.path().join("dialogo.csv");

        // Use the actual Shift-JIS bytes, with the padding of a string table.
        let original_bytes: Vec<_> = [0, 0]
            .into_iter()
            .chain(encode_game_sjis("メニューへ戻りますか？", None))
            .chain([0, 0])
            .collect();
        fs::write(&original, &original_bytes).unwrap();
        fs::write(
            &csv,
            "source,file_id,offset,original_text,translated_text\r\nELF,ELF,0x2,メニューへ戻りますか？,Menu\r\nSCRIPT,1,0x0,a,b\r\n",
        )
        .unwrap();

        let report = patch_translated_elf(&original, &translated, &csv, None).unwrap();
        assert_eq!(report.rows_processed, 1);
        assert_eq!(report.rows_patched, 1);
        let patched = fs::read(&translated).unwrap();
        assert_eq!(&patched[2..6], b"Menu");
    }

    #[test]
    fn reports_skipped_elf_rows_without_modifying_file() {
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("SLPS_256.11");
        let translated = temp.path().join("SLPS_256.11_translated");
        let csv = temp.path().join("dialogo.csv");

        let original_bytes: Vec<_> = [0, 0]
            .into_iter()
            .chain(encode_game_sjis("メニューへ戻りますか？", None))
            .chain([0, 0])
            .collect();
        fs::write(&original, &original_bytes).unwrap();
        fs::write(
            &csv,
            "source,file_id,offset,original_text,translated_text\r\n\
             ELF,ELF,not_hex,メニューへ戻りますか？,Ok\r\n\
             ELF,ELF,0x2,メニューへ戻りますか？,This translation is much longer than the original text field\r\n\
             SCRIPT,1,0x0,a,b\r\n",
        )
        .unwrap();

        let report = patch_translated_elf(&original, &translated, &csv, None).unwrap();
        assert_eq!(report.rows_processed, 2);
        assert_eq!(report.rows_patched, 0);
        assert_eq!(report.skipped_other, 1);
        assert_eq!(report.skipped_too_large, 1);
        assert_eq!(fs::read(&translated).unwrap(), original_bytes);
    }

    #[test]
    fn rejects_instructions_disguised_as_shift_jis_even_from_stale_csv() {
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("SLPS_256.11");
        let translated = temp.path().join("translated");
        let csv = temp.path().join("dialogo.csv");
        let mut data = vec![0; 0x30];
        data.extend_from_slice(&[
            0x54, 0x83, 0x85, 0x93, 0x55, 0x83, 0x86, 0x93, 0x56, 0x83, 0x87, 0x93, 0xa0, 0xb0,
            0x05, 0x0c,
        ]);
        fs::write(&original, &data).unwrap();
        fs::write(&translated, vec![0x41; data.len()]).unwrap();
        fs::write(&csv, "source,file_id,offset,original_text,translated_text\nELF,ELF,0x31,ュ填ユ天ョ蕩,Cargando...\n").unwrap();
        let report = patch_translated_elf(&original, &translated, &csv, None).unwrap();
        assert_eq!(report.skipped_other, 1);
        assert_eq!(fs::read(&translated).unwrap(), data);
    }

    #[test]
    fn real_elf_excludes_false_code_strings_when_available() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let original_path = workspace.join("originales/SLPS_256.11");
        let path = original_path.as_path();
        if !path.exists() {
            return;
        }
        let entries = extract_elf_strings(path).unwrap();
        assert!(entries.iter().any(|entry| entry.byte_offset == 0x88170));
        assert!(entries.iter().any(|entry| entry.byte_offset == 0x90ac5
            && entry.original_text == "抜き差ししないでください。"));
        assert!(entries.iter().any(|entry| entry.byte_offset == 0x92b75));
        let temp = tempfile::tempdir().unwrap();
        let patched = temp.path().join("ELF");
        let csv = temp.path().join("dialogo.csv");
        fs::write(&csv, "source,file_id,offset,original_text,translated_text\nELF,ELF,0x90ac5,抜き差ししないでください。,No extraer\nELF,ELF,0x6502d,ュ填ユ天ョ蕩,Cargando...\n").unwrap();
        let report = patch_translated_elf(path, &patched, &csv, None).unwrap();
        assert_eq!(report.rows_patched, 1);
        assert_eq!(report.skipped_other, 1);
        assert_ne!(
            &fs::read(&patched).unwrap()[0x90ac5..0x90ad0],
            &fs::read(path).unwrap()[0x90ac5..0x90ad0]
        );
        for invalid in [0x35020, 0x35704, 0x6502d, 0x8bcf2, 0x8becc, 0x8bfc4] {
            assert!(
                !entries.iter().any(|entry| entry.byte_offset == invalid),
                "false positive at 0x{invalid:X}"
            );
        }
    }
}
