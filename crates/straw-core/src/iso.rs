use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

use anyhow::{bail, Context, Result};

use crate::copy_file_creating_parent;

pub const DATA_BIN_SIGNATURE: &[u8] = b"\x00\xC8\x78\x78\x13\x6B\x00\x00\x00\x80\x00\x00";

pub fn build_iso_with_patched_data(
    base_iso: impl AsRef<Path>,
    patched_data: impl AsRef<Path>,
    out_iso: impl AsRef<Path>,
) -> Result<u64> {
    let base_iso = base_iso.as_ref();
    let patched_data = patched_data.as_ref();
    let out_iso = out_iso.as_ref();

    if !base_iso.exists() {
        bail!("base ISO not found: {}", base_iso.display());
    }
    if !patched_data.exists() {
        bail!("patched Data.bin not found: {}", patched_data.display());
    }
    if out_iso.exists() && fs::canonicalize(base_iso)? == fs::canonicalize(out_iso)? {
        bail!("base ISO and output ISO must be different files");
    }
    copy_file_creating_parent(base_iso, out_iso)?;

    let target_offset = find_signature_offset(out_iso, DATA_BIN_SIGNATURE)?
        .with_context(|| "Data.bin signature not found in ISO")?;
    inject_file_at_offset(out_iso, patched_data, target_offset)
}

pub fn inject_elf_into_iso(
    iso_path: impl AsRef<Path>,
    original_elf: impl AsRef<Path>,
    translated_elf: impl AsRef<Path>,
) -> Result<u64> {
    let iso_path = iso_path.as_ref();
    let original_elf = original_elf.as_ref();
    let translated_elf = translated_elf.as_ref();

    if !iso_path.exists() {
        bail!("ISO not found: {}", iso_path.display());
    }
    if !original_elf.exists() {
        bail!("original ELF not found: {}", original_elf.display());
    }
    if !translated_elf.exists() {
        bail!("translated ELF not found: {}", translated_elf.display());
    }

    let signature = read_prefix(original_elf, 4096)?;
    let target_offset = find_signature_offset(iso_path, &signature)?
        .with_context(|| "ELF signature not found in ISO")?;
    inject_file_at_offset(iso_path, translated_elf, target_offset)
}

pub fn find_signature_offset(path: impl AsRef<Path>, signature: &[u8]) -> Result<Option<u64>> {
    if signature.is_empty() {
        bail!("signature cannot be empty");
    }

    let mut file = fs::File::open(path.as_ref())
        .with_context(|| format!("failed to open {}", path.as_ref().display()))?;
    let chunk_size = 16 * 1024 * 1024;
    let overlap = signature.len().saturating_sub(1);
    let mut offset = 0_u64;
    let mut chunk = vec![0_u8; chunk_size];

    loop {
        file.seek(SeekFrom::Start(offset))?;
        let read = file.read(&mut chunk)?;
        if read == 0 {
            return Ok(None);
        }
        if let Some(idx) = find_bytes(&chunk[..read], signature) {
            return Ok(Some(offset + idx as u64));
        }
        if read <= overlap {
            return Ok(None);
        }
        offset += (read - overlap) as u64;
    }
}

fn inject_file_at_offset(target: &Path, source: &Path, offset: u64) -> Result<u64> {
    let mut target_file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(target)
        .with_context(|| format!("failed to open {}", target.display()))?;
    let mut source_file =
        fs::File::open(source).with_context(|| format!("failed to open {}", source.display()))?;
    let target_len = target_file.metadata()?.len();
    let source_len = source_file.metadata()?.len();
    let end = offset
        .checked_add(source_len)
        .with_context(|| "injection range overflows u64")?;
    if end > target_len {
        bail!(
            "source {} does not fit in target {} at offset {offset}: end {end} > target size {target_len}",
            source.display(),
            target.display()
        );
    }

    target_file.seek(SeekFrom::Start(offset))?;
    let written = std::io::copy(&mut source_file, &mut target_file)?;
    target_file.flush()?;
    Ok(written)
}

fn read_prefix(path: &Path, size: usize) -> Result<Vec<u8>> {
    let mut file =
        fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut signature = vec![0_u8; size];
    let read = file.read(&mut signature)?;
    signature.truncate(read);
    if signature.is_empty() {
        bail!("signature source is empty: {}", path.display());
    }
    Ok(signature)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_signature_across_file() {
        let temp = tempfile::tempdir().unwrap();
        let iso = temp.path().join("base.iso");
        let mut data = vec![0xAA; 128];
        data.extend(DATA_BIN_SIGNATURE);
        data.extend([0xBB; 128]);
        fs::write(&iso, data).unwrap();

        assert_eq!(
            find_signature_offset(&iso, DATA_BIN_SIGNATURE).unwrap(),
            Some(128)
        );
    }

    #[test]
    fn builds_iso_by_injecting_patched_data() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().join("base.iso");
        let patched = temp.path().join("Data_patched.bin");
        let out = temp.path().join("out.iso");

        let mut iso_data = vec![0xAA; 64];
        iso_data.extend(DATA_BIN_SIGNATURE);
        iso_data.extend([0xBB; 64]);
        fs::write(&base, iso_data).unwrap();
        fs::write(&patched, b"PATCHED").unwrap();

        let written = build_iso_with_patched_data(&base, &patched, &out).unwrap();
        assert_eq!(written, 7);
        let out_data = fs::read(&out).unwrap();
        assert_eq!(&out_data[64..71], b"PATCHED");
    }

    #[test]
    fn rebuild_discards_previous_iso_changes() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().join("base.iso");
        let patched = temp.path().join("Data.bin");
        let out = temp.path().join("out.iso");
        let mut original = vec![0xAA; 64];
        original.extend(DATA_BIN_SIGNATURE);
        original.extend([0xBB; 64]);
        fs::write(&base, &original).unwrap();
        fs::write(&patched, b"FIRST").unwrap();
        build_iso_with_patched_data(&base, &patched, &out).unwrap();
        let mut stale = fs::read(&out).unwrap();
        stale[0] = 0xCC;
        fs::write(&out, stale).unwrap();
        fs::write(&patched, b"SECOND").unwrap();
        build_iso_with_patched_data(&base, &patched, &out).unwrap();
        original[64..70].copy_from_slice(b"SECOND");
        assert_eq!(fs::read(&out).unwrap(), original);
        assert!(build_iso_with_patched_data(&base, &patched, &base).is_err());
    }

    #[test]
    fn injects_elf_by_original_prefix() {
        let temp = tempfile::tempdir().unwrap();
        let iso = temp.path().join("out.iso");
        let original_elf = temp.path().join("SLPS_256.11");
        let translated_elf = temp.path().join("SLPS_256.11_translated");

        let original = b"ORIGINAL_ELF_PREFIX_and_body";
        let translated = b"TRANSLATED_ELF";
        let mut iso_data = vec![0xAA; 48];
        iso_data.extend(original);
        iso_data.extend([0xBB; 48]);
        fs::write(&iso, iso_data).unwrap();
        fs::write(&original_elf, original).unwrap();
        fs::write(&translated_elf, translated).unwrap();

        let written = inject_elf_into_iso(&iso, &original_elf, &translated_elf).unwrap();
        assert_eq!(written, translated.len() as u64);
        let out_data = fs::read(&iso).unwrap();
        assert_eq!(&out_data[48..48 + translated.len()], translated);
    }

    #[test]
    fn rejects_empty_signature() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("file.bin");
        fs::write(&file, b"data").unwrap();

        let err = find_signature_offset(&file, b"").unwrap_err();
        assert!(err.to_string().contains("signature cannot be empty"));
    }

    #[test]
    fn build_iso_errors_when_data_signature_is_missing() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().join("base.iso");
        let patched = temp.path().join("Data_patched.bin");
        let out = temp.path().join("out.iso");
        fs::write(&base, b"no data signature here").unwrap();
        fs::write(&patched, b"PATCHED").unwrap();

        let err = build_iso_with_patched_data(&base, &patched, &out).unwrap_err();
        assert!(err.to_string().contains("Data.bin signature not found"));
        assert_eq!(fs::read(&out).unwrap(), b"no data signature here");
    }

    #[test]
    fn rejects_data_injection_that_would_extend_iso() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().join("base.iso");
        let patched = temp.path().join("Data_patched.bin");
        let out = temp.path().join("out.iso");

        let mut iso_data = vec![0xAA; 8];
        iso_data.extend(DATA_BIN_SIGNATURE);
        fs::write(&base, &iso_data).unwrap();
        fs::write(&patched, vec![0xCC; 64]).unwrap();

        let err = build_iso_with_patched_data(&base, &patched, &out).unwrap_err();
        assert!(err.to_string().contains("does not fit in target"));
        assert_eq!(fs::read(&out).unwrap(), iso_data);
    }
}
