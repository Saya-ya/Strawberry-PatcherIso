use std::{fs, path::Path};

use anyhow::{bail, Context, Result};

use crate::{datafat::parse_entries_from_data, lz77::decompress_lz77};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedScript {
    pub row: usize,
    pub id: u32,
    pub offset: u32,
    pub compressed_size: u32,
    pub decompressed: Vec<u8>,
}

pub fn extract_lz77_scripts(data_bin: &[u8]) -> Result<Vec<ExtractedScript>> {
    let entries = parse_entries_from_data(data_bin)?;
    let mut scripts = Vec::new();

    for entry in entries.iter().filter(|entry| entry.is_file) {
        let start = entry.offset as usize;
        let size = entry.size as usize;
        if start + 4 > data_bin.len() {
            continue;
        }
        if data_bin.get(start..start + 4) != Some(&b"LZ77"[..]) {
            continue;
        }
        let end = start
            .checked_add(size)
            .filter(|end| *end <= data_bin.len())
            .with_context(|| format!("ID {} LZ77 range outside Data.bin", entry.id))?;
        let decompressed = decompress_lz77(&data_bin[start..end], None, true)
            .with_context(|| format!("failed to decompress LZ77 ID {}", entry.id))?;
        scripts.push(ExtractedScript {
            row: entry.row,
            id: entry.id,
            offset: entry.offset,
            compressed_size: entry.size,
            decompressed,
        });
    }

    Ok(scripts)
}

pub fn write_extracted_scripts(
    scripts: &[ExtractedScript],
    out_dir: impl AsRef<Path>,
) -> Result<usize> {
    let out_dir = out_dir.as_ref();
    fs::create_dir_all(out_dir)
        .with_context(|| format!("failed to create {}", out_dir.display()))?;

    for script in scripts {
        let path = out_dir.join(format!("ID_{:05}.dec", script.id));
        fs::write(&path, &script.decompressed)
            .with_context(|| format!("failed to write {}", path.display()))?;
    }

    Ok(scripts.len())
}

pub fn extract_lz77_scripts_from_file(path: impl AsRef<Path>) -> Result<Vec<ExtractedScript>> {
    let data = fs::read(path.as_ref())
        .with_context(|| format!("failed to read {}", path.as_ref().display()))?;
    extract_lz77_scripts(&data)
}

pub fn extract_lz77_scripts_to_dir(
    data_bin_path: impl AsRef<Path>,
    out_dir: impl AsRef<Path>,
) -> Result<usize> {
    let scripts = extract_lz77_scripts_from_file(data_bin_path)?;
    if scripts.is_empty() {
        bail!("no LZ77 scripts found");
    }
    write_extracted_scripts(&scripts, out_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_real_lz77_scripts_when_databin_exists() {
        let path = Path::new("originales/Data.bin");
        if !path.exists() {
            return;
        }

        let scripts = extract_lz77_scripts_from_file(path).unwrap();
        assert_eq!(scripts.len(), 997);
        assert!(scripts.iter().all(|script| !script.decompressed.is_empty()));
    }

    #[test]
    fn extracted_script_matches_existing_dec_when_available() {
        let data_path = Path::new("originales/Data.bin");
        let dec_dir = Path::new("work/scripts_extraidos");
        if !data_path.exists() || !dec_dir.exists() {
            return;
        }

        let scripts = extract_lz77_scripts_from_file(data_path).unwrap();
        let Some(script) = scripts
            .iter()
            .find(|script| dec_dir.join(format!("ID_{:05}.dec", script.id)).exists())
        else {
            return;
        };

        let dec_path = dec_dir.join(format!("ID_{:05}.dec", script.id));
        let existing = fs::read(&dec_path).unwrap();
        assert_eq!(
            script.decompressed,
            existing,
            "mismatch for {}",
            dec_path.display()
        );
    }
}
