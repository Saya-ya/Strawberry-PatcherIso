use std::{fs, path::Path};

use anyhow::{bail, Context, Result};

pub const FAT_OFFSET: usize = 0x8004;
pub const NUM_ENTRIES: usize = 27_411;
pub const ENTRY_SIZE: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FatEntry {
    pub row: usize,
    pub id: u32,
    pub size_field: u32,
    pub offset: u32,
    pub size: u32,
    pub is_file: bool,
}

pub fn read_entries(path: impl AsRef<Path>) -> Result<Vec<FatEntry>> {
    let data = fs::read(path.as_ref())
        .with_context(|| format!("failed to read Data.bin from {}", path.as_ref().display()))?;
    parse_entries_from_data(&data)
}

pub fn parse_entries_from_data(data: &[u8]) -> Result<Vec<FatEntry>> {
    let end = FAT_OFFSET + NUM_ENTRIES * ENTRY_SIZE;
    if data.len() < end {
        bail!(
            "Data.bin too small for FAT: {} bytes, need at least {}",
            data.len(),
            end
        );
    }
    parse_entries(&data[FAT_OFFSET..end])
}

pub fn parse_entries(fat_raw: &[u8]) -> Result<Vec<FatEntry>> {
    let expected_len = NUM_ENTRIES * ENTRY_SIZE;
    if fat_raw.len() < expected_len {
        bail!(
            "FAT too small: {} bytes, need at least {}",
            fat_raw.len(),
            expected_len
        );
    }

    let mut rows = Vec::with_capacity(NUM_ENTRIES);
    for row in 0..NUM_ENTRIES {
        let base = row * ENTRY_SIZE;
        let id = read_u32_le(fat_raw, base);
        let size_field = read_u32_le(fat_raw, base + 4);
        let offset = read_u32_le(fat_raw, base + 8);
        rows.push(FatEntry {
            row,
            id,
            size_field,
            offset,
            size: 0,
            is_file: offset > 0,
        });
    }

    for row in 0..NUM_ENTRIES {
        let size = if row + 1 < NUM_ENTRIES {
            rows[row + 1].size_field
        } else {
            rows[row].size_field
        };
        rows[row].size = size;
    }

    Ok(rows)
}

pub fn find_row(rows: &[FatEntry], file_id: u32) -> Option<&FatEntry> {
    rows.iter()
        .find(|entry| entry.id == file_id && entry.offset > 0)
}

pub fn slot_capacity(rows: &[FatEntry], row: &FatEntry) -> Option<u32> {
    let mut offsets: Vec<u32> = rows
        .iter()
        .filter(|entry| entry.offset > 0)
        .map(|entry| entry.offset)
        .collect();
    offsets.sort_unstable();

    let idx = offsets.iter().position(|offset| *offset == row.offset)?;
    if idx + 1 < offsets.len() {
        Some(offsets[idx + 1].saturating_sub(row.offset))
    } else {
        Some(row.size)
    }
}

pub fn size_field_write_offset(row: &FatEntry) -> usize {
    FAT_OFFSET + (row.row + 1) * ENTRY_SIZE + 4
}

fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        data[offset..offset + 4]
            .try_into()
            .expect("slice length checked"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_size_from_next_row() {
        let mut fat = vec![0_u8; NUM_ENTRIES * ENTRY_SIZE];
        write_row(&mut fat, 0, 10, 111, 0x1000);
        write_row(&mut fat, 1, 11, 222, 0x1100);

        let rows = parse_entries(&fat).unwrap();
        assert_eq!(rows[0].id, 10);
        assert_eq!(rows[0].size_field, 111);
        assert_eq!(rows[0].size, 222);
        assert_eq!(
            size_field_write_offset(&rows[0]),
            FAT_OFFSET + ENTRY_SIZE + 4
        );
    }

    #[test]
    fn finds_rows_and_slot_capacity() {
        let mut fat = vec![0_u8; NUM_ENTRIES * ENTRY_SIZE];
        write_row(&mut fat, 0, 10, 111, 0x1000);
        write_row(&mut fat, 1, 11, 222, 0x1200);
        write_row(&mut fat, 2, 12, 333, 0x1800);

        let rows = parse_entries(&fat).unwrap();
        let row = find_row(&rows, 11).unwrap();
        assert_eq!(slot_capacity(&rows, row), Some(0x600));
    }

    #[test]
    fn reads_real_databin_when_available() {
        let path = std::path::Path::new("originales/Data.bin");
        if !path.exists() {
            return;
        }

        let rows = read_entries(path).unwrap();
        assert_eq!(rows.len(), NUM_ENTRIES);
        assert_eq!(rows[0].id, 0);
        assert!(rows.iter().any(|entry| entry.offset > 0));
    }

    fn write_row(fat: &mut [u8], row: usize, id: u32, size_field: u32, offset: u32) {
        let base = row * ENTRY_SIZE;
        fat[base..base + 4].copy_from_slice(&id.to_le_bytes());
        fat[base + 4..base + 8].copy_from_slice(&size_field.to_le_bytes());
        fat[base + 8..base + 12].copy_from_slice(&offset.to_le_bytes());
    }
}
