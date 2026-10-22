use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    sync::mpsc,
    thread,
    time::Instant,
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    compress_lz77,
    datafat::{parse_entries_from_data, FatEntry, ENTRY_SIZE, FAT_OFFSET, NUM_ENTRIES},
    decompress_lz77, find_row, size_field_write_offset, slot_capacity, GlyphMap,
};

/// Synthetic ID for the resource referenced at Data.bin header +0x0c.
/// It has no FAT row; its length is stored in its own LZ77 header.
pub const HEADER_TEXTURE_ID: u32 = u32::MAX;
const HEADER_RESOURCE_POINTER: usize = 0x0c;
const FONT_TABLE_POINTER: usize = 0x0c;
const FONT_TABLE_HEADER_SIZE: usize = 0x10;
const FONT_GLYPH_RECORD_SIZE: usize = 0x10;
const AUTO_METRIC_PADDING: usize = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextureRecord {
    pub id: u32,
    pub fat_row: usize,
    pub file_offset: u32,
    pub raw_size: u32,
    pub compressed: bool,
    pub nested_lz77_offset: Option<usize>,
    pub nested_lz77_size: Option<usize>,
    pub tim2_index: usize,
    pub tim2_offset: usize,
    pub picture_index: usize,
    pub width: u16,
    pub height: u16,
    pub image_type: u8,
    pub image_type_name: String,
    pub image_size: u32,
    pub clut_size: u32,
    pub clut_color_count: u16,
    pub score_ui: i32,
    pub score_font: i32,
    #[serde(default)]
    pub texture_hash: String,
    pub png: String,
}

impl TextureRecord {
    pub fn is_header_resource(&self) -> bool {
        self.id == HEADER_TEXTURE_ID
    }
}

/// Add the pre-FAT-files resource to the texture-only view of the archive.
/// Keeping the real FAT parser unchanged avoids treating fonts as scripts.
fn texture_resource_rows(data: &[u8]) -> Result<Vec<FatEntry>> {
    let mut rows = parse_entries_from_data(data)?;
    if rows
        .iter()
        .any(|row| row.id == HEADER_TEXTURE_ID && row.is_file)
    {
        bail!("FAT ID {HEADER_TEXTURE_ID} conflicts with the reserved header texture ID");
    }
    let offset = read_u32(data, HEADER_RESOURCE_POINTER).unwrap_or(0);
    if offset == 0 {
        return Ok(rows);
    }
    // If the header references an ordinary FAT file, it is already covered.
    if rows.iter().any(|row| row.is_file && row.offset == offset) {
        return Ok(rows);
    }
    let first_file = rows
        .iter()
        .filter(|row| row.is_file)
        .map(|row| row.offset)
        .min()
        .context("header texture resource has no following FAT file")?;
    let start = offset as usize;
    let fat_end = FAT_OFFSET + NUM_ENTRIES * ENTRY_SIZE;
    if start < fat_end || offset >= first_file || first_file as usize > data.len() {
        bail!("header texture resource at 0x{offset:X} is outside the pre-FAT-files region");
    }
    let raw = &data[start..first_file as usize];
    let size = lz77_stream_size(raw, 0).context("invalid header texture resource")?;
    // Validate the compressed stream before exposing a patchable resource.
    decompress_lz77(&raw[..size], None, true).context("invalid header texture LZ77")?;
    rows.push(FatEntry {
        row: NUM_ENTRIES, // Sentinel: never pass this entry to size_field_write_offset.
        id: HEADER_TEXTURE_ID,
        size_field: 0,
        offset,
        size: u32::try_from(size)?,
        is_file: true,
    });
    Ok(rows)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TexturePatchReport {
    pub files_processed: usize,
    pub patches_applied: usize,
    pub streams_written: usize,
    pub glyph_metrics_adjusted: usize,
    pub glyph_metric_changes: Vec<GlyphMetricChange>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlyphMetricChange {
    pub donor: char,
    pub source_characters: String,
    pub tim2_index: usize,
    pub old_x: u16,
    pub new_x: u16,
    pub old_width: u8,
    pub new_width: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureInjectReport {
    pub streams_injected: usize,
    pub bytes_written: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct TexturePatchEntry {
    file_id: u32,
    tim2_index: usize,
    picture_index: usize,
    png: String,
    mode: Option<String>,
    lz77_offset: Option<usize>,
}

#[derive(Debug, Clone)]
struct Tim2Picture {
    index: usize,
    picture_offset: usize,
    image_size: u32,
    clut_size: u32,
    header_size: usize,
    clut_color_count: u16,
    image_type: u8,
    width: u16,
    height: u16,
    image_data: Vec<u8>,
    clut_data: Vec<u8>,
}

#[derive(Debug, Clone)]
struct Tim2File {
    offset: usize,
    pictures: Vec<Tim2Picture>,
}

pub fn write_texture_inventory(
    data_bin: impl AsRef<Path>,
    out_dir: impl AsRef<Path>,
) -> Result<Vec<TextureRecord>> {
    write_texture_inventory_with_progress(data_bin, out_dir, |_| {})
}

pub fn write_texture_inventory_with_progress(
    data_bin: impl AsRef<Path>,
    out_dir: impl AsRef<Path>,
    mut progress: impl FnMut(String),
) -> Result<Vec<TextureRecord>> {
    let data_bin = data_bin.as_ref();
    progress(format!("Leyendo {}", data_bin.display()));
    let data =
        fs::read(data_bin).with_context(|| format!("failed to read {}", data_bin.display()))?;
    progress(format!("Data.bin leido: {} bytes", data.len()));
    let rows = texture_resource_rows(&data)?;
    if let Some(header) = find_row(&rows, HEADER_TEXTURE_ID) {
        progress(format!(
            "Recurso de cabecera fuera de FAT: ID {}, offset 0x{:X}, {} bytes LZ77",
            header.id, header.offset, header.size
        ));
    }
    let file_rows = rows
        .into_iter()
        .filter(|row| row.is_file && row.size > 0)
        .collect::<Vec<_>>();
    let total_rows = file_rows.len();
    progress(format!(
        "Recursos de textura: {total_rows} (FAT y cabecera)"
    ));
    let out_dir = out_dir.as_ref();
    fs::create_dir_all(out_dir)
        .with_context(|| format!("failed to create {}", out_dir.display()))?;
    let png_dir = out_dir.join("png");
    fs::create_dir_all(&png_dir)
        .with_context(|| format!("failed to create {}", png_dir.display()))?;

    progress("Escaneando archivos y exportando PNGs TIM2".to_owned());
    let records = texture_inventory_from_data(&data, &file_rows, &png_dir, &mut progress)?;

    let json_path = out_dir.join("textures.json");
    progress(format!("Escribiendo {}", json_path.display()));
    fs::write(&json_path, serde_json::to_string_pretty(&records)?)
        .with_context(|| format!("failed to write {}", json_path.display()))?;

    let csv_path = out_dir.join("textures.csv");
    progress(format!("Escribiendo {}", csv_path.display()));
    let mut writer = csv::Writer::from_path(&csv_path)
        .with_context(|| format!("failed to write {}", csv_path.display()))?;
    for record in &records {
        writer.serialize(record)?;
    }
    writer.flush()?;
    let pngs = records
        .iter()
        .filter(|record| !record.png.is_empty())
        .count();
    progress(format!(
        "Inventario completado: {} texturas detectadas, {pngs} PNGs exportados",
        records.len()
    ));
    Ok(records)
}

pub fn patch_textures_from_manifest(
    data_bin: impl AsRef<Path>,
    manifest_path: impl AsRef<Path>,
    out_dir: impl AsRef<Path>,
) -> Result<TexturePatchReport> {
    patch_textures_from_manifest_with_glyph_map(data_bin, manifest_path, out_dir, None)
}

pub fn patch_textures_from_manifest_with_glyph_map(
    data_bin: impl AsRef<Path>,
    manifest_path: impl AsRef<Path>,
    out_dir: impl AsRef<Path>,
    glyph_map: Option<&GlyphMap>,
) -> Result<TexturePatchReport> {
    let data_bin = data_bin.as_ref();
    let manifest_path = manifest_path.as_ref();
    let out_dir = out_dir.as_ref();
    let manifest = fs::read_to_string(manifest_path)
        .with_context(|| format!("failed to read {}", manifest_path.display()))?;
    let entries: Vec<TexturePatchEntry> = serde_json::from_str(&manifest)
        .with_context(|| format!("failed to parse {}", manifest_path.display()))?;
    fs::create_dir_all(out_dir)
        .with_context(|| format!("failed to create {}", out_dir.display()))?;

    let mut by_file: BTreeMap<u32, Vec<TexturePatchEntry>> = BTreeMap::new();
    for entry in entries {
        by_file.entry(entry.file_id).or_default().push(entry);
    }

    let mut report = TexturePatchReport {
        files_processed: 0,
        patches_applied: 0,
        streams_written: 0,
        glyph_metrics_adjusted: 0,
        glyph_metric_changes: Vec::new(),
        errors: Vec::new(),
    };
    let data =
        fs::read(data_bin).with_context(|| format!("failed to read {}", data_bin.display()))?;
    let rows = texture_resource_rows(&data)?;

    for (file_id, patches) in by_file {
        report.files_processed += 1;
        let Some(row) = find_row(&rows, file_id) else {
            report.errors.push(format!("ID {file_id} not found"));
            continue;
        };
        let start = row.offset as usize;
        let end = start.saturating_add(row.size as usize).min(data.len());
        if start >= end {
            report
                .errors
                .push(format!("ID {file_id} has invalid resource range"));
            continue;
        }
        let auto_metrics = (file_id == HEADER_TEXTURE_ID)
            .then_some(glyph_map)
            .flatten();
        match process_texture_file(&data[start..end], &patches, auto_metrics) {
            Ok((stream, applied, metric_changes)) => {
                fs::write(out_dir.join(format!("ID_{file_id:05}.lz77")), stream)?;
                report.patches_applied += applied;
                report.streams_written += 1;
                report.glyph_metrics_adjusted += metric_changes.len();
                report.glyph_metric_changes.extend(metric_changes);
            }
            Err(err) => report.errors.push(format!("ID {file_id}: {err}")),
        }
    }

    Ok(report)
}

pub fn inject_patched_texture_streams(
    data_bin_path: impl AsRef<Path>,
    streams_dir: impl AsRef<Path>,
) -> Result<TextureInjectReport> {
    let data_bin_path = data_bin_path.as_ref();
    let streams_dir = streams_dir.as_ref();
    let mut data = fs::read(data_bin_path)
        .with_context(|| format!("failed to read {}", data_bin_path.display()))?;
    let rows = texture_resource_rows(&data)?;
    let mut report = TextureInjectReport {
        streams_injected: 0,
        bytes_written: 0,
        errors: Vec::new(),
    };

    if !streams_dir.exists() {
        bail!(
            "patched texture stream dir not found: {}",
            streams_dir.display()
        );
    }

    for entry in fs::read_dir(streams_dir)
        .with_context(|| format!("failed to read {}", streams_dir.display()))?
    {
        let path = entry?.path();
        if path.extension().is_none_or(|ext| ext != "lz77") {
            continue;
        }
        let Some(file_id) = parse_patched_stream_id(&path) else {
            report
                .errors
                .push(format!("invalid stream name: {}", path.display()));
            continue;
        };
        let stream =
            fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
        let Some(target) = find_row(&rows, file_id) else {
            report
                .errors
                .push(format!("texture resource ID {file_id} not found"));
            continue;
        };
        let Some(capacity) = slot_capacity(&rows, target) else {
            report
                .errors
                .push(format!("could not compute capacity for ID {file_id}"));
            continue;
        };
        if stream.len() > capacity as usize {
            report.errors.push(format!(
                "ID {file_id} does not fit: {} > {capacity}",
                stream.len()
            ));
            continue;
        }
        let start = target.offset as usize;
        let Some(end) = start.checked_add(capacity as usize) else {
            report
                .errors
                .push(format!("ID {file_id} slot range overflows"));
            continue;
        };
        if end > data.len() {
            report
                .errors
                .push(format!("ID {file_id} slot outside Data.bin"));
            continue;
        }
        let size_offset = if file_id == HEADER_TEXTURE_ID {
            // The header pointer stays fixed. Length comes from the new LZ77 header,
            // not from the next FAT row as with ordinary resources.
            if !stream.starts_with(b"LZ77") || decompress_lz77(&stream, None, true).is_err() {
                report
                    .errors
                    .push("invalid patched header texture LZ77".to_owned());
                continue;
            }
            None
        } else {
            let offset = size_field_write_offset(target);
            if offset.checked_add(4).is_none_or(|end| end > data.len()) {
                report
                    .errors
                    .push(format!("ID {file_id} size field outside Data.bin"));
                continue;
            }
            Some(offset)
        };
        data[start..start + stream.len()].copy_from_slice(&stream);
        data[start + stream.len()..end].fill(0);
        if let Some(size_offset) = size_offset {
            data[size_offset..size_offset + 4]
                .copy_from_slice(&(stream.len() as u32).to_le_bytes());
        }
        report.streams_injected += 1;
        report.bytes_written += stream.len();
    }

    fs::write(data_bin_path, data)
        .with_context(|| format!("failed to write {}", data_bin_path.display()))?;
    Ok(report)
}

fn parse_patched_stream_id(path: &Path) -> Option<u32> {
    path.file_stem()?
        .to_str()?
        .strip_prefix("ID_")?
        .parse()
        .ok()
}

fn texture_inventory_from_data(
    data: &[u8],
    rows: &[FatEntry],
    png_dir: &Path,
    progress: &mut impl FnMut(String),
) -> Result<Vec<TextureRecord>> {
    let total_rows = rows.len();
    if total_rows == 0 {
        progress(
            "Escaneo terminado: 0/0 archivos; 0 texturas detectadas; 0 PNGs exportados".to_owned(),
        );
        return Ok(Vec::new());
    }

    let workers = thread::available_parallelism()
        .map(|workers| workers.get())
        .unwrap_or(1)
        .clamp(1, total_rows);
    progress(format!(
        "Usando {workers} hilos para escanear/exportar texturas"
    ));

    if workers == 1 {
        let (records, stats) = process_texture_rows_chunk(data, rows, png_dir, None)?;
        progress(format!(
            "Escaneo terminado: {}/{} archivos; {} texturas detectadas; {} PNGs exportados",
            stats.scanned,
            total_rows,
            records.len(),
            stats.exported_pngs
        ));
        return Ok(records);
    }

    let (progress_tx, progress_rx) = mpsc::channel::<TextureProgress>();
    let chunk_size = total_rows.div_ceil(workers);
    let mut results = Vec::new();
    let mut worker_panicked = false;
    thread::scope(|scope| {
        let mut handles = Vec::new();
        for chunk in rows.chunks(chunk_size) {
            let progress_tx = progress_tx.clone();
            handles.push(scope.spawn(move || {
                process_texture_rows_chunk(data, chunk, png_dir, Some(&progress_tx))
            }));
        }
        drop(progress_tx);

        let mut scanned = 0usize;
        let mut found = 0usize;
        let mut exported_pngs = 0usize;
        let mut last_message = Instant::now();
        for update in progress_rx {
            scanned += update.scanned;
            found += update.found;
            exported_pngs += update.exported_pngs;
            if scanned == total_rows || last_message.elapsed().as_secs() >= 3 {
                progress(format!(
                    "Escaneando archivos: {scanned}/{total_rows}; texturas detectadas: {found}; PNGs exportados: {exported_pngs}"
                ));
                last_message = Instant::now();
            }
        }

        for handle in handles {
            match handle.join() {
                Ok(result) => results.push(result),
                Err(_) => worker_panicked = true,
            }
        }
    });
    if worker_panicked {
        bail!("texture worker panicked");
    }

    let mut records = Vec::new();
    let mut exported_pngs = 0usize;
    for result in results {
        let (mut chunk_records, stats) = result?;
        exported_pngs += stats.exported_pngs;
        records.append(&mut chunk_records);
    }
    records.sort_by_key(|record| {
        (
            record.fat_row,
            record.nested_lz77_offset.unwrap_or(0),
            record.tim2_index,
            record.picture_index,
        )
    });

    progress(format!(
        "Escaneo terminado: {total_rows}/{total_rows} archivos; {} texturas detectadas; {exported_pngs} PNGs exportados",
        records.len()
    ));
    Ok(records)
}

#[derive(Debug, Default)]
struct TextureProgress {
    scanned: usize,
    found: usize,
    exported_pngs: usize,
}

fn process_texture_rows_chunk(
    data: &[u8],
    rows: &[FatEntry],
    png_dir: &Path,
    progress_tx: Option<&mpsc::Sender<TextureProgress>>,
) -> Result<(Vec<TextureRecord>, TextureProgress)> {
    let mut records = Vec::new();
    let mut stats = TextureProgress::default();
    let mut pending = TextureProgress::default();

    for row in rows {
        stats.scanned += 1;
        pending.scanned += 1;
        let start = row.offset as usize;
        let end = start.saturating_add(row.size as usize).min(data.len());
        if start >= end || end > data.len() {
            send_texture_progress(progress_tx, &mut pending);
            continue;
        }
        let raw = &data[start..end];
        let mut candidates = Vec::new();
        if raw.starts_with(b"LZ77") {
            if let Ok(blob) = decompress_lz77(raw, None, false) {
                candidates.push((Cow::Owned(blob), true, None, None));
            }
        } else {
            candidates.push((Cow::Borrowed(raw), false, None, None));
            for (nested_off, nested_size, blob) in iter_nested_lz77(raw) {
                candidates.push((Cow::Owned(blob), true, Some(nested_off), Some(nested_size)));
            }
        }

        for (blob, compressed, nested_lz77_offset, nested_lz77_size) in candidates {
            for (tim2_index, tim2) in find_tim2_files(blob.as_ref()).into_iter().enumerate() {
                let tim2_offset = tim2.offset;
                for pic in tim2.pictures {
                    let unique = unique_index_count(&pic);
                    let texture_hash = texture_hash(&pic);
                    let mut record = TextureRecord {
                        id: row.id,
                        fat_row: row.row,
                        file_offset: row.offset,
                        raw_size: row.size,
                        compressed,
                        nested_lz77_offset,
                        nested_lz77_size,
                        tim2_index,
                        tim2_offset,
                        picture_index: pic.index,
                        width: pic.width,
                        height: pic.height,
                        image_type: pic.image_type,
                        image_type_name: image_type_name(pic.image_type).to_owned(),
                        image_size: pic.image_size,
                        clut_size: pic.clut_size,
                        clut_color_count: pic.clut_color_count,
                        score_ui: score_ui(&pic, unique),
                        score_font: score_font(&pic, unique),
                        texture_hash,
                        png: String::new(),
                    };
                    if let Some(png_path) = write_picture_png(&pic, png_dir, &record)? {
                        record.png = format!(
                            "png/{}",
                            png_path.file_name().unwrap_or_default().to_string_lossy()
                        );
                        stats.exported_pngs += 1;
                        pending.exported_pngs += 1;
                    }
                    stats.found += 1;
                    pending.found += 1;
                    records.push(record);
                }
            }
        }

        if pending.scanned >= 100 || pending.exported_pngs >= 100 {
            send_texture_progress(progress_tx, &mut pending);
        }
    }

    send_texture_progress(progress_tx, &mut pending);
    Ok((records, stats))
}

fn send_texture_progress(
    progress_tx: Option<&mpsc::Sender<TextureProgress>>,
    pending: &mut TextureProgress,
) {
    if pending.scanned == 0 && pending.found == 0 && pending.exported_pngs == 0 {
        return;
    }
    if let Some(progress_tx) = progress_tx {
        let _ = progress_tx.send(TextureProgress {
            scanned: pending.scanned,
            found: pending.found,
            exported_pngs: pending.exported_pngs,
        });
    }
    *pending = TextureProgress::default();
}

fn process_texture_file(
    raw: &[u8],
    patches: &[TexturePatchEntry],
    glyph_map: Option<&GlyphMap>,
) -> Result<(Vec<u8>, usize, Vec<GlyphMetricChange>)> {
    let nested = patches.iter().any(|patch| patch.lz77_offset.is_some());
    if nested {
        if patches.iter().any(|patch| patch.lz77_offset.is_none()) {
            bail!("cannot mix direct and nested texture patches in one file");
        }
        let mut raw = raw.to_vec();
        let mut by_offset: BTreeMap<usize, Vec<&TexturePatchEntry>> = BTreeMap::new();
        for patch in patches {
            by_offset
                .entry(patch.lz77_offset.unwrap_or(0))
                .or_default()
                .push(patch);
        }
        let mut applied = 0;
        for (nested_off, group) in by_offset {
            let old_size = lz77_stream_size(&raw, nested_off)?;
            let mut blob = decompress_lz77(&raw[nested_off..nested_off + old_size], None, false)?;
            for patch in group {
                apply_texture_patch(&mut blob, patch)?;
                applied += 1;
            }
            let new_stream = compress_lz77(&blob, false);
            if decompress_lz77(&new_stream, None, false)? != blob {
                bail!("nested LZ77 roundtrip failed at 0x{nested_off:X}");
            }
            if new_stream.len() > old_size {
                bail!(
                    "nested LZ77 at 0x{nested_off:X} does not fit: {} > {old_size}",
                    new_stream.len()
                );
            }
            raw[nested_off..nested_off + new_stream.len()].copy_from_slice(&new_stream);
            raw[nested_off + new_stream.len()..nested_off + old_size].fill(0);
        }
        return Ok((raw, applied, Vec::new()));
    }

    let was_lz77 = raw.starts_with(b"LZ77");
    let mut blob = if was_lz77 {
        decompress_lz77(raw, None, false)?
    } else {
        raw.to_vec()
    };
    let metric_changes = if let Some(glyph_map) = glyph_map {
        apply_auto_font_metrics(&mut blob, patches, glyph_map)?
    } else {
        Vec::new()
    };
    for patch in patches {
        apply_texture_patch(&mut blob, patch)?;
    }
    if was_lz77 {
        let stream = compress_lz77(&blob, false);
        if decompress_lz77(&stream, None, false)? != blob {
            bail!("LZ77 roundtrip failed");
        }
        Ok((stream, patches.len(), metric_changes))
    } else {
        Ok((blob, patches.len(), metric_changes))
    }
}

#[derive(Debug, Clone, Copy)]
struct FontGlyphRecord {
    offset: usize,
    character: char,
    x: u16,
    y: u16,
    width: u8,
    height: u8,
    tim2_index: usize,
}

fn apply_auto_font_metrics(
    blob: &mut [u8],
    patches: &[TexturePatchEntry],
    glyph_map: &GlyphMap,
) -> Result<Vec<GlyphMetricChange>> {
    if glyph_map.is_empty() {
        return Ok(Vec::new());
    }
    let records = parse_font_glyph_records(blob)?;
    let by_character = records
        .into_iter()
        .map(|record| (record.character, record))
        .collect::<BTreeMap<_, _>>();
    let tim2_files = find_tim2_files(blob);
    let mut edited_pages = BTreeMap::new();
    for patch in patches {
        if patch.lz77_offset.is_some() {
            continue;
        }
        let tm2 = tim2_files
            .get(patch.tim2_index)
            .with_context(|| format!("TIM2 {} not found for font metrics", patch.tim2_index))?;
        let picture = tm2
            .pictures
            .iter()
            .find(|picture| picture.index == patch.picture_index)
            .with_context(|| {
                format!(
                    "picture {} not found in TIM2 {} for font metrics",
                    patch.picture_index, patch.tim2_index
                )
            })?;
        let (width, height, edited) = read_png_rgba(Path::new(&patch.png))?;
        if width != u32::from(picture.width) || height != u32::from(picture.height) {
            continue; // The regular texture patch reports the dimension error.
        }
        edited_pages.insert(
            (patch.tim2_index, patch.picture_index),
            (
                picture_to_rgba(picture)?,
                edited,
                width as usize,
                height as usize,
            ),
        );
    }

    let mut sources_by_donor: BTreeMap<char, BTreeSet<char>> = BTreeMap::new();
    for (&source, &donor) in glyph_map {
        sources_by_donor.entry(donor).or_default().insert(source);
    }

    let mut changes = Vec::new();
    for (donor, sources) in sources_by_donor {
        let record = by_character
            .get(&donor)
            .with_context(|| format!("font glyph donor {donor:?} not found"))?;
        let Some((original, edited, image_width, image_height)) =
            edited_pages.get(&(record.tim2_index, 0))
        else {
            continue;
        };
        validate_glyph_rect(record, *image_width, *image_height)?;
        if !rgba_rect_changed(original, edited, *image_width, record) {
            continue;
        }
        let Some((left, right)) = alpha_horizontal_bounds(edited, *image_width, record) else {
            bail!("edited glyph donor {donor:?} has no visible pixels");
        };
        let left = left.saturating_sub(AUTO_METRIC_PADDING);
        let right = (right + AUTO_METRIC_PADDING).min(record.width as usize);
        let new_x = usize::from(record.x)
            .checked_add(left)
            .context("font glyph x overflow")?;
        let new_width = right.saturating_sub(left);
        if new_width == 0 || new_x > u16::MAX as usize || new_width > u8::MAX as usize {
            bail!("invalid automatic metrics for glyph donor {donor:?}");
        }
        let new_x = new_x as u16;
        let new_width = new_width as u8;
        if new_x == record.x && new_width == record.width {
            continue;
        }
        blob[record.offset + 2..record.offset + 4].copy_from_slice(&new_x.to_le_bytes());
        blob[record.offset + 6] = new_width;
        changes.push(GlyphMetricChange {
            donor,
            source_characters: sources.into_iter().collect(),
            tim2_index: record.tim2_index,
            old_x: record.x,
            new_x,
            old_width: record.width,
            new_width,
        });
    }
    Ok(changes)
}

fn parse_font_glyph_records(blob: &[u8]) -> Result<Vec<FontGlyphRecord>> {
    let header = read_u32(blob, FONT_TABLE_POINTER).context("font table pointer missing")? as usize;
    let records_start = header
        .checked_add(FONT_TABLE_HEADER_SIZE)
        .context("font table offset overflow")?;
    let count = read_u16(blob, header + 2).context("font glyph count missing")? as usize;
    let records_end = records_start
        .checked_add(
            count
                .checked_mul(FONT_GLYPH_RECORD_SIZE)
                .context("font table size overflow")?,
        )
        .context("font table end overflow")?;
    let first_tim2 = find_tim2_files(blob)
        .first()
        .map(|tim2| tim2.offset)
        .context("font resource has no TIM2 pages")?;
    if count == 0 || records_end > first_tim2 || records_end > blob.len() {
        bail!("invalid font glyph table");
    }
    let mut records = Vec::with_capacity(count);
    for offset in (records_start..records_end).step_by(FONT_GLYPH_RECORD_SIZE) {
        let codepoint = read_u16(blob, offset).context("font glyph codepoint missing")? as u32;
        let Some(character) = char::from_u32(codepoint) else {
            continue;
        };
        records.push(FontGlyphRecord {
            offset,
            character,
            x: read_u16(blob, offset + 2).unwrap_or(0),
            y: read_u16(blob, offset + 4).unwrap_or(0),
            width: blob[offset + 6],
            height: blob[offset + 7],
            tim2_index: blob[offset + 8] as usize,
        });
    }
    Ok(records)
}

fn validate_glyph_rect(
    record: &FontGlyphRecord,
    image_width: usize,
    image_height: usize,
) -> Result<()> {
    let right = usize::from(record.x)
        .checked_add(record.width as usize)
        .context("font glyph horizontal range overflow")?;
    let bottom = usize::from(record.y)
        .checked_add(record.height as usize)
        .context("font glyph vertical range overflow")?;
    if record.width == 0 || record.height == 0 || right > image_width || bottom > image_height {
        bail!(
            "font glyph {:?} is outside TIM2 {}",
            record.character,
            record.tim2_index
        );
    }
    Ok(())
}

fn rgba_rect_changed(
    original: &[u8],
    edited: &[u8],
    image_width: usize,
    record: &FontGlyphRecord,
) -> bool {
    (0..record.height as usize).any(|dy| {
        (0..record.width as usize).any(|dx| {
            let alpha = ((record.y as usize + dy) * image_width + record.x as usize + dx) * 4 + 3;
            original.get(alpha) != edited.get(alpha)
        })
    })
}

fn alpha_horizontal_bounds(
    rgba: &[u8],
    image_width: usize,
    record: &FontGlyphRecord,
) -> Option<(usize, usize)> {
    let mut left = record.width as usize;
    let mut right = 0;
    for dy in 0..record.height as usize {
        for dx in 0..record.width as usize {
            let alpha =
                rgba[((record.y as usize + dy) * image_width + record.x as usize + dx) * 4 + 3];
            if alpha != 0 {
                left = left.min(dx);
                right = right.max(dx + 1);
            }
        }
    }
    (left < right).then_some((left, right))
}

fn apply_texture_patch(blob: &mut [u8], patch: &TexturePatchEntry) -> Result<()> {
    if patch.mode.as_deref().unwrap_or("preserve_palette") != "preserve_palette" {
        bail!("only preserve_palette texture patches are supported");
    }
    let tm2 = find_tim2_files(blob)
        .into_iter()
        .nth(patch.tim2_index)
        .with_context(|| format!("TIM2 {} not found", patch.tim2_index))?;
    let pic = tm2
        .pictures
        .into_iter()
        .find(|pic| pic.index == patch.picture_index)
        .with_context(|| format!("picture {} not found", patch.picture_index))?;
    let (width, height, rgba) = read_png_rgba(Path::new(&patch.png))?;
    if width != pic.width as u32 || height != pic.height as u32 {
        bail!(
            "PNG dimensions {width}x{height} do not match TIM2 {}x{}",
            pic.width,
            pic.height
        );
    }
    let image_data = encode_picture_preserve_palette(&pic, &rgba)?;
    if image_data.len() != pic.image_size as usize {
        bail!(
            "encoded image size {} does not match original {}",
            image_data.len(),
            pic.image_size
        );
    }
    let image_offset = pic.picture_offset + pic.header_size;
    blob[image_offset..image_offset + image_data.len()].copy_from_slice(&image_data);
    Ok(())
}

fn encode_picture_preserve_palette(pic: &Tim2Picture, rgba: &[u8]) -> Result<Vec<u8>> {
    match pic.image_type {
        5 => encode_indexed8_preserve(pic, rgba),
        3 | 4 => encode_indexed4_preserve(pic, rgba),
        other => bail!("image_type {other} is not supported for preserve_palette"),
    }
}

fn encode_indexed8_preserve(pic: &Tim2Picture, rgba: &[u8]) -> Result<Vec<u8>> {
    let palette = decode_clut_rgba32(pic, true);
    let pixels = pic.width as usize * pic.height as usize;
    let mut out = Vec::with_capacity(pixels);
    for i in 0..pixels {
        out.push(closest_index(&rgba[i * 4..i * 4 + 4], &palette) as u8);
    }
    Ok(out)
}

fn encode_indexed4_preserve(pic: &Tim2Picture, rgba: &[u8]) -> Result<Vec<u8>> {
    let mut palette = decode_clut_rgba32(pic, false);
    palette.truncate(
        usize::from(if pic.clut_color_count == 0 {
            16
        } else {
            pic.clut_color_count
        })
        .min(16),
    );
    let pixels = pic.width as usize * pic.height as usize;
    let mut out = Vec::with_capacity((pixels + 1) / 2);
    for i in (0..pixels).step_by(2) {
        let low = closest_index(&rgba[i * 4..i * 4 + 4], &palette) as u8 & 0x0f;
        let high = if i + 1 < pixels {
            (closest_index(&rgba[(i + 1) * 4..(i + 1) * 4 + 4], &palette) as u8 & 0x0f) << 4
        } else {
            0
        };
        out.push(low | high);
    }
    Ok(out)
}

fn closest_index(color: &[u8], palette: &[[u8; 4]]) -> usize {
    if color[3] == 0 {
        if let Some(idx) = palette.iter().position(|c| c[3] == 0) {
            return idx;
        }
    }
    let mut best = 0;
    let mut best_distance = i64::MAX;
    for (idx, candidate) in palette.iter().enumerate() {
        let dr = color[0] as i64 - candidate[0] as i64;
        let dg = color[1] as i64 - candidate[1] as i64;
        let db = color[2] as i64 - candidate[2] as i64;
        let da = color[3] as i64 - candidate[3] as i64;
        let distance = dr * dr + dg * dg + db * db + da * da * 4;
        if distance < best_distance {
            best_distance = distance;
            best = idx;
            if distance == 0 {
                break;
            }
        }
    }
    best
}

pub fn normalize_transparent_pixels(rgba: &mut [u8]) -> usize {
    let mut modified = 0;
    for chunk in rgba.chunks_exact_mut(4) {
        if chunk[3] == 0 && (chunk[0] != 0 || chunk[1] != 0 || chunk[2] != 0) {
            chunk[0] = 0;
            chunk[1] = 0;
            chunk[2] = 0;
            modified += 1;
        }
    }
    modified
}

pub fn clean_png_transparency(path: &Path) -> Result<bool> {
    let (width, height, mut rgba) = read_png_raw_rgba(path)?;
    let modified = normalize_transparent_pixels(&mut rgba);
    if modified > 0 {
        write_png_rgba(path, width, height, &rgba)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

pub fn clean_all_pngs_in_dir(dir: &Path) -> Result<(usize, usize)> {
    let mut scanned = 0;
    let mut cleaned = 0;
    if !dir.exists() {
        return Ok((0, 0));
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("png"))
                .unwrap_or(false)
        {
            scanned += 1;
            if clean_png_transparency(&path)? {
                cleaned += 1;
            }
        }
    }
    Ok((scanned, cleaned))
}

fn read_png_raw_rgba(path: &Path) -> Result<(u32, u32, Vec<u8>)> {
    let file =
        fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let decoder = png::Decoder::new(file);
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf)?;
    let data = &buf[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => data.to_vec(),
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity(info.width as usize * info.height as usize * 4);
            for chunk in data.chunks_exact(3) {
                out.extend([chunk[0], chunk[1], chunk[2], 255]);
            }
            out
        }
        png::ColorType::Grayscale => data.iter().flat_map(|v| [*v, *v, *v, 255]).collect(),
        png::ColorType::GrayscaleAlpha => data
            .chunks_exact(2)
            .flat_map(|v| [v[0], v[0], v[0], v[1]])
            .collect(),
        other => bail!("unsupported PNG color type: {other:?}"),
    };
    Ok((info.width, info.height, rgba))
}

fn read_png_rgba(path: &Path) -> Result<(u32, u32, Vec<u8>)> {
    let (width, height, mut rgba) = read_png_raw_rgba(path)?;
    normalize_transparent_pixels(&mut rgba);
    Ok((width, height, rgba))
}

fn lz77_stream_size(stream: &[u8], offset: usize) -> Result<usize> {
    let Some(header_end) = offset.checked_add(4) else {
        bail!("no valid LZ77 stream at 0x{offset:X}");
    };
    if stream.get(offset..header_end) != Some(b"LZ77")
        || offset.checked_add(12).is_none_or(|end| end > stream.len())
    {
        bail!("no valid LZ77 stream at 0x{offset:X}");
    }
    let comp_size = u32::from_le_bytes([
        stream[offset + 8],
        stream[offset + 9],
        stream[offset + 10],
        stream[offset + 11],
    ]) as usize;
    let size = 12_usize
        .checked_add(comp_size)
        .with_context(|| format!("LZ77 stream size overflow at 0x{offset:X}"))?;
    if comp_size == 0
        || offset
            .checked_add(size)
            .is_none_or(|end| end > stream.len())
    {
        bail!("truncated LZ77 stream at 0x{offset:X}");
    }
    Ok(size)
}

fn write_picture_png(
    pic: &Tim2Picture,
    png_dir: &Path,
    record: &TextureRecord,
) -> Result<Option<std::path::PathBuf>> {
    let Ok(rgba) = picture_to_rgba(&pic) else {
        return Ok(None);
    };

    let nested = record
        .nested_lz77_offset
        .map(|offset| format!("_L{offset:06X}"))
        .unwrap_or_default();
    let name = format!(
        "ID_{:05}{}_T{:03}_P{:02}_{}x{}.png",
        record.id, nested, record.tim2_index, record.picture_index, record.width, record.height
    );
    let path = png_dir.join(name);
    write_png_rgba(&path, pic.width as u32, pic.height as u32, &rgba)?;
    Ok(Some(path))
}

fn iter_nested_lz77(raw: &[u8]) -> Vec<(usize, usize, Vec<u8>)> {
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(idx) = find_bytes(&raw[pos..], b"LZ77") {
        let off = pos + idx;
        pos = off + 4;
        if off + 12 > raw.len() {
            continue;
        }
        let comp_size =
            u32::from_le_bytes([raw[off + 8], raw[off + 9], raw[off + 10], raw[off + 11]]) as usize;
        let stream_size = 12 + comp_size;
        if comp_size == 0 || off + stream_size > raw.len() {
            continue;
        }
        if let Ok(blob) = decompress_lz77(&raw[off..off + stream_size], None, false) {
            out.push((off, stream_size, blob));
        }
    }
    out
}

fn find_tim2_files(data: &[u8]) -> Vec<Tim2File> {
    let mut found = Vec::new();
    let mut pos = 0;
    while let Some(idx) = find_bytes(&data[pos..], b"TIM2") {
        let off = pos + idx;
        if let Some(tm2) = parse_tim2(data, off) {
            found.push(tm2);
        }
        pos = off + 4;
    }
    found
}

fn parse_tim2(data: &[u8], offset: usize) -> Option<Tim2File> {
    let file_header_end = offset.checked_add(16)?;
    if data.get(offset..file_header_end)?[0..4] != *b"TIM2" {
        return None;
    }
    let version = data[offset + 4];
    let num_pictures = read_u16(data, offset + 6)? as usize;
    if !(3..=4).contains(&version) || num_pictures == 0 || num_pictures > 256 {
        return None;
    }
    let mut pos = file_header_end;
    let mut pictures = Vec::new();
    for index in 0..num_pictures {
        if pos.checked_add(48)? > data.len() {
            return None;
        }
        let total_size = read_u32(data, pos)?;
        let clut_size = read_u32(data, pos + 4)?;
        let image_size = read_u32(data, pos + 8)?;
        let header_size = read_u16(data, pos + 0x0c)? as usize;
        let clut_color_count = read_u16(data, pos + 0x0e)?;
        let image_type = *data.get(pos + 0x13)?;
        let width = read_u16(data, pos + 0x14)?;
        let height = read_u16(data, pos + 0x16)?;
        if header_size < 48
            || total_size < header_size as u32
            || Some(total_size)
                != (header_size as u32)
                    .checked_add(image_size)
                    .and_then(|size| size.checked_add(clut_size))
        {
            return None;
        }
        let image_start = pos.checked_add(header_size)?;
        let image_end = image_start.checked_add(image_size as usize)?;
        let clut_end = image_end.checked_add(clut_size as usize)?;
        if width == 0 || height == 0 || width > 8192 || height > 8192 || clut_end > data.len() {
            return None;
        }
        pictures.push(Tim2Picture {
            index,
            picture_offset: pos,
            image_size,
            clut_size,
            header_size,
            clut_color_count,
            image_type,
            width,
            height,
            image_data: data[image_start..image_end].to_vec(),
            clut_data: data[image_end..clut_end].to_vec(),
        });
        pos = pos.checked_add(total_size as usize)?;
    }
    Some(Tim2File { offset, pictures })
}

fn picture_to_rgba(pic: &Tim2Picture) -> Result<Vec<u8>> {
    match pic.image_type {
        5 => indexed8_to_rgba(pic),
        3 | 4 => indexed4_to_rgba(pic),
        0 => rgba32_to_rgba(pic),
        1 => rgb24_to_rgba(pic),
        2 => rgba5551_to_rgba_bytes(pic),
        other => bail!("unsupported TIM2 image_type {other}"),
    }
}

fn indexed8_to_rgba(pic: &Tim2Picture) -> Result<Vec<u8>> {
    let palette = decode_clut_rgba32(pic, true);
    let mut out = Vec::with_capacity(pic.width as usize * pic.height as usize * 4);
    for index in pic
        .image_data
        .iter()
        .take(pic.width as usize * pic.height as usize)
    {
        out.extend(
            palette
                .get(*index as usize)
                .copied()
                .unwrap_or([255, 0, 255, 255]),
        );
    }
    pad_rgba(&mut out, pic);
    Ok(out)
}

fn indexed4_to_rgba(pic: &Tim2Picture) -> Result<Vec<u8>> {
    let palette = decode_clut_rgba32(pic, false);
    let mut out = Vec::with_capacity(pic.width as usize * pic.height as usize * 4);
    for byte in &pic.image_data {
        out.extend(
            palette
                .get((byte & 0x0f) as usize)
                .copied()
                .unwrap_or([255, 0, 255, 255]),
        );
        out.extend(
            palette
                .get((byte >> 4) as usize)
                .copied()
                .unwrap_or([255, 0, 255, 255]),
        );
    }
    out.truncate(pic.width as usize * pic.height as usize * 4);
    pad_rgba(&mut out, pic);
    Ok(out)
}

fn rgba32_to_rgba(pic: &Tim2Picture) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(pic.width as usize * pic.height as usize * 4);
    for chunk in pic
        .image_data
        .chunks(4)
        .take(pic.width as usize * pic.height as usize)
    {
        if chunk.len() == 4 {
            out.extend([chunk[0], chunk[1], chunk[2], ps2_alpha_to_png(chunk[3])]);
        } else {
            out.extend([255, 0, 255, 255]);
        }
    }
    pad_rgba(&mut out, pic);
    Ok(out)
}

fn rgb24_to_rgba(pic: &Tim2Picture) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(pic.width as usize * pic.height as usize * 4);
    for chunk in pic
        .image_data
        .chunks(3)
        .take(pic.width as usize * pic.height as usize)
    {
        if chunk.len() == 3 {
            out.extend([chunk[0], chunk[1], chunk[2], 255]);
        } else {
            out.extend([255, 0, 255, 255]);
        }
    }
    pad_rgba(&mut out, pic);
    Ok(out)
}

fn rgba5551_to_rgba_bytes(pic: &Tim2Picture) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(pic.width as usize * pic.height as usize * 4);
    for chunk in pic
        .image_data
        .chunks(2)
        .take(pic.width as usize * pic.height as usize)
    {
        if chunk.len() == 2 {
            let px = u16::from_le_bytes([chunk[0], chunk[1]]);
            let r5 = px & 0x1f;
            let g5 = (px >> 5) & 0x1f;
            let b5 = (px >> 10) & 0x1f;
            let a1 = (px >> 15) & 1;
            out.extend([
                ((r5 << 3) | (r5 >> 2)) as u8,
                ((g5 << 3) | (g5 >> 2)) as u8,
                ((b5 << 3) | (b5 >> 2)) as u8,
                if a1 != 0 { 255 } else { 0 },
            ]);
        } else {
            out.extend([255, 0, 255, 255]);
        }
    }
    pad_rgba(&mut out, pic);
    Ok(out)
}

fn decode_clut_rgba32(pic: &Tim2Picture, unswizzle: bool) -> Vec<[u8; 4]> {
    let default_colors = if matches!(pic.image_type, 3 | 4) {
        16
    } else {
        256
    };
    let colors = usize::from(if pic.clut_color_count == 0 {
        default_colors
    } else {
        pic.clut_color_count
    })
    .min(pic.clut_data.len() / 4);
    let raw = (0..colors)
        .map(|i| {
            let base = i * 4;
            [
                pic.clut_data[base],
                pic.clut_data[base + 1],
                pic.clut_data[base + 2],
                ps2_alpha_to_png(pic.clut_data[base + 3]),
            ]
        })
        .collect::<Vec<_>>();
    if unswizzle && colors >= 256 && pic.image_type == 5 {
        (0..colors)
            .map(|i| {
                raw.get(unswizzle_psmt8_clut_index(i))
                    .copied()
                    .unwrap_or([0, 0, 0, 0])
            })
            .collect()
    } else {
        raw
    }
}

fn pad_rgba(out: &mut Vec<u8>, pic: &Tim2Picture) {
    let expected = pic.width as usize * pic.height as usize * 4;
    while out.len() < expected {
        out.extend([255, 0, 255, 255]);
    }
    out.truncate(expected);
}

fn ps2_alpha_to_png(alpha: u8) -> u8 {
    if alpha == 0 {
        0
    } else {
        alpha.saturating_mul(2)
    }
}

fn unswizzle_psmt8_clut_index(i: usize) -> usize {
    (i & !0x18) | ((i & 0x08) << 1) | ((i & 0x10) >> 1)
}

fn write_png_rgba(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    if rgba.len() != width as usize * height as usize * 4 {
        bail!("invalid RGBA length for PNG");
    }
    let file =
        fs::File::create(path).with_context(|| format!("failed to create {}", path.display()))?;
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(rgba)?;
    Ok(())
}

fn unique_index_count(pic: &Tim2Picture) -> Option<usize> {
    match pic.image_type {
        5 => Some(
            pic.image_data
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
        ),
        3 | 4 => {
            let mut vals = std::collections::BTreeSet::new();
            for byte in &pic.image_data {
                vals.insert(byte & 0x0f);
                vals.insert((byte >> 4) & 0x0f);
            }
            Some(vals.len())
        }
        _ => None,
    }
}

fn score_ui(pic: &Tim2Picture, unique: Option<usize>) -> i32 {
    let mut score = 0;
    let aspect = if pic.height == 0 {
        0.0
    } else {
        pic.width as f32 / pic.height as f32
    };
    if matches!(pic.image_type, 3 | 4 | 5) {
        score += 6;
    }
    if aspect >= 3.0 && pic.height <= 64 {
        score += 40;
    } else if aspect >= 2.0 && pic.height <= 96 {
        score += 25;
    } else if aspect >= 1.5 && pic.height <= 128 {
        score += 10;
    }
    if matches!(pic.height, 16 | 24 | 32 | 40 | 48 | 56 | 64) {
        score += 12;
    }
    if let Some(unique) = unique {
        if unique <= 4 {
            score += 14;
        } else if unique <= 16 {
            score += 10;
        } else if unique <= 48 {
            score += 3;
        } else {
            score -= 6;
        }
    }
    if pic.width >= 512 && pic.height >= 384 {
        score -= 25;
    }
    score
}

fn score_font(pic: &Tim2Picture, unique: Option<usize>) -> i32 {
    let mut score = 0;
    if matches!(pic.image_type, 3 | 4 | 5) {
        score += 10;
    }
    if pic.width == pic.height && matches!(pic.width, 128 | 256 | 512) {
        score += 30;
    }
    if pic.width % 16 == 0 && pic.height % 16 == 0 {
        score += 8;
    }
    if unique.is_some_and(|count| count <= 20) {
        score += 10;
    }
    score
}

fn texture_hash(pic: &Tim2Picture) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    fn add(hash: &mut u64, bytes: &[u8]) {
        for byte in bytes {
            *hash ^= u64::from(*byte);
            *hash = hash.wrapping_mul(0x100000001b3);
        }
    }

    add(&mut hash, &[pic.image_type]);
    add(&mut hash, &pic.width.to_le_bytes());
    add(&mut hash, &pic.height.to_le_bytes());
    add(&mut hash, &pic.clut_color_count.to_le_bytes());
    add(&mut hash, &pic.image_data);
    add(&mut hash, &pic.clut_data);
    format!("{hash:016x}")
}

fn image_type_name(image_type: u8) -> &'static str {
    match image_type {
        0 => "PSMCT32/RGBA32?",
        1 => "PSMCT24/RGB24?",
        2 => "PSMCT16/RGB5A1?",
        3 => "INDEX4?",
        4 => "INDEX4",
        5 => "INDEX8",
        _ => "UNKNOWN",
    }
}

fn read_u16(data: &[u8], offset: usize) -> Option<u16> {
    data.get(offset..offset.checked_add(2)?)
        .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], offset: usize) -> Option<u32> {
    data.get(offset..offset.checked_add(4)?)
        .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font_metrics_blob() -> Vec<u8> {
        let image_width = 48_u16;
        let image_height = 25_u16;
        let image_size = usize::from(image_width) * usize::from(image_height) / 2;
        let tim2_offset = 0x130;
        let mut blob = vec![0; tim2_offset + 16 + 48 + image_size + 64];
        blob[FONT_TABLE_POINTER..FONT_TABLE_POINTER + 4].copy_from_slice(&0x100_u32.to_le_bytes());
        blob[0x102..0x104].copy_from_slice(&2_u16.to_le_bytes());
        for (offset, character, x) in [(0x110, 'Г', 0_u16), (0x120, 'Д', 21_u16)] {
            blob[offset..offset + 2].copy_from_slice(&(character as u16).to_le_bytes());
            blob[offset + 2..offset + 4].copy_from_slice(&x.to_le_bytes());
            blob[offset + 6] = 21;
            blob[offset + 7] = 25;
        }
        let tim2 = &mut blob[tim2_offset..];
        tim2[..4].copy_from_slice(b"TIM2");
        tim2[4] = 4;
        tim2[6..8].copy_from_slice(&1_u16.to_le_bytes());
        tim2[16..20].copy_from_slice(&((48 + image_size + 64) as u32).to_le_bytes());
        tim2[20..24].copy_from_slice(&64_u32.to_le_bytes());
        tim2[24..28].copy_from_slice(&(image_size as u32).to_le_bytes());
        tim2[28..30].copy_from_slice(&48_u16.to_le_bytes());
        tim2[30..32].copy_from_slice(&16_u16.to_le_bytes());
        tim2[35] = 4;
        tim2[36..38].copy_from_slice(&image_width.to_le_bytes());
        tim2[38..40].copy_from_slice(&image_height.to_le_bytes());
        let clut = 16 + 48 + image_size;
        tim2[clut + 4..clut + 8].copy_from_slice(&[255, 255, 255, 128]);
        blob
    }

    #[test]
    fn auto_metrics_use_active_map_alpha_and_deduplicate_aliases() {
        let temp = tempfile::tempdir().unwrap();
        let png = temp.path().join("font.png");
        let mut rgba = vec![0; 48 * 25 * 4];
        for y in 5..21 {
            for x in 5..17 {
                rgba[(y * 48 + x) * 4..(y * 48 + x) * 4 + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        write_png_rgba(&png, 48, 25, &rgba).unwrap();
        let patch = TexturePatchEntry {
            file_id: HEADER_TEXTURE_ID,
            tim2_index: 0,
            picture_index: 0,
            png: png.display().to_string(),
            mode: Some("preserve_palette".to_owned()),
            lz77_offset: None,
        };
        let glyph_map = [('á', 'Г'), ('Á', 'Г'), ('é', 'Д')].into_iter().collect();
        let original = font_metrics_blob();
        let raw = compress_lz77(&original, false);

        let (stream, applied, changes) =
            process_texture_file(&raw, &[patch], Some(&glyph_map)).unwrap();

        assert_eq!(applied, 1);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].donor, 'Г');
        assert_eq!(changes[0].source_characters, "Áá");
        assert_eq!((changes[0].old_x, changes[0].old_width), (0, 21));
        assert_eq!((changes[0].new_x, changes[0].new_width), (4, 14));
        let patched = decompress_lz77(&stream, None, true).unwrap();
        assert_eq!(read_u16(&patched, 0x112), Some(4));
        assert_eq!(patched[0x116], 14);
        assert_eq!(&patched[0x114..0x116], &original[0x114..0x116]);
        assert_eq!(patched[0x117], original[0x117]);
        assert_eq!(&patched[0x120..0x130], &original[0x120..0x130]);
    }

    #[test]
    fn auto_metrics_reject_changed_glyph_without_visible_pixels() {
        let temp = tempfile::tempdir().unwrap();
        let png = temp.path().join("font.png");
        let mut original = font_metrics_blob();
        let picture = &find_tim2_files(&original)[0].pictures[0];
        let image_offset = picture.picture_offset + picture.header_size;
        original[image_offset] = 1;
        let rgba = vec![0; 48 * 25 * 4];
        write_png_rgba(&png, 48, 25, &rgba).unwrap();
        let patch = TexturePatchEntry {
            file_id: HEADER_TEXTURE_ID,
            tim2_index: 0,
            picture_index: 0,
            png: png.display().to_string(),
            mode: None,
            lz77_offset: None,
        };
        let glyph_map = [('á', 'Г')].into_iter().collect();

        let err =
            process_texture_file(&compress_lz77(&original, false), &[patch], Some(&glyph_map))
                .unwrap_err();

        assert!(err.to_string().contains("has no visible pixels"));
    }

    fn indexed_font_fixture(header_offset: usize) -> Vec<u8> {
        let mut tim2 = vec![0; 16 + 48 + 2 + 64];
        tim2[..4].copy_from_slice(b"TIM2");
        tim2[4] = 4;
        tim2[6..8].copy_from_slice(&1_u16.to_le_bytes());
        tim2[16..20].copy_from_slice(&114_u32.to_le_bytes());
        tim2[20..24].copy_from_slice(&64_u32.to_le_bytes());
        tim2[24..28].copy_from_slice(&2_u32.to_le_bytes());
        tim2[28..30].copy_from_slice(&48_u16.to_le_bytes());
        tim2[30..32].copy_from_slice(&16_u16.to_le_bytes());
        tim2[35] = 4; // INDEX4
        tim2[36..38].copy_from_slice(&2_u16.to_le_bytes());
        tim2[38..40].copy_from_slice(&2_u16.to_le_bytes());
        tim2[64..66].copy_from_slice(&[0x10, 0x10]);
        tim2[66..74].copy_from_slice(&[0, 0, 0, 128, 255, 255, 255, 128]);
        let stream = compress_lz77(&tim2, false);
        let file_offset = header_offset + 4096;
        let mut data = vec![0; file_offset + 1024];
        data[HEADER_RESOURCE_POINTER..HEADER_RESOURCE_POINTER + 4]
            .copy_from_slice(&(header_offset as u32).to_le_bytes());
        data[FAT_OFFSET..FAT_OFFSET + 4].copy_from_slice(&2_u32.to_le_bytes());
        data[FAT_OFFSET + 8..FAT_OFFSET + 12].copy_from_slice(&(file_offset as u32).to_le_bytes());
        data[FAT_OFFSET + ENTRY_SIZE + 4..FAT_OFFSET + ENTRY_SIZE + 8]
            .copy_from_slice(&(stream.len() as u32).to_le_bytes());
        data[header_offset..header_offset + stream.len()].copy_from_slice(&stream);
        data[file_offset..file_offset + stream.len()].copy_from_slice(&stream);
        data
    }

    #[test]
    fn discovers_header_resource_from_pointer_and_validates_its_bounds() {
        let fat_end = FAT_OFFSET + NUM_ENTRIES * ENTRY_SIZE;
        for offset in [fat_end + 256, fat_end + 1024] {
            let data = indexed_font_fixture(offset);
            let rows = texture_resource_rows(&data).unwrap();
            let header = find_row(&rows, HEADER_TEXTURE_ID).unwrap();
            assert_eq!(header.offset as usize, offset);
            assert_eq!(header.row, NUM_ENTRIES);
            assert_eq!(slot_capacity(&rows, header), Some(4096));
            assert_eq!(parse_entries_from_data(&data).unwrap().len(), NUM_ENTRIES);
        }
        let mut data = indexed_font_fixture(fat_end + 256);
        data[12..16].fill(0);
        assert!(find_row(&texture_resource_rows(&data).unwrap(), HEADER_TEXTURE_ID).is_none());
        data[12..16].copy_from_slice(&32_u32.to_le_bytes());
        assert!(texture_resource_rows(&data).is_err());
        data[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(texture_resource_rows(&data).is_err());
        let mut data = indexed_font_fixture(fat_end + 256);
        let offset = fat_end + 256;
        data[offset + 8..offset + 12].copy_from_slice(&4096_u32.to_le_bytes());
        assert!(texture_resource_rows(&data).is_err());
    }

    #[test]
    fn header_font_exports_hashes_and_roundtrips_through_manifest_without_changing_fat() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("Data.bin");
        let offset = FAT_OFFSET + NUM_ENTRIES * ENTRY_SIZE + 256;
        let original = indexed_font_fixture(offset);
        fs::write(&path, &original).unwrap();
        let records = write_texture_inventory(&path, temp.path().join("inventory")).unwrap();
        assert_eq!(records.len(), 2);
        let header = records.iter().find(|r| r.is_header_resource()).unwrap();
        let ordinary = records.iter().find(|r| !r.is_header_resource()).unwrap();
        assert_eq!(header.texture_hash, ordinary.texture_hash);
        assert!(!header.texture_hash.is_empty());
        assert!(header.png.contains("ID_4294967295_T000_P00_2x2.png"));
        let png = temp.path().join("inventory").join(&header.png);
        let (width, height, mut rgba) = read_png_rgba(&png).unwrap();
        rgba[..4].copy_from_slice(&[255, 255, 255, 255]);
        write_png_rgba(&png, width, height, &rgba).unwrap();
        let manifest = temp.path().join("manifest.json");
        fs::write(
            &manifest,
            serde_json::to_vec(&serde_json::json!([{
                "file_id": HEADER_TEXTURE_ID, "tim2_index": 0, "picture_index": 0,
                "png": png, "mode": "preserve_palette", "lz77_offset": null
            }]))
            .unwrap(),
        )
        .unwrap();
        let streams = temp.path().join("streams");
        let report = patch_textures_from_manifest(&path, &manifest, &streams).unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.streams_written, 1);
        let report = inject_patched_texture_streams(&path, &streams).unwrap();
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        assert_eq!(report.streams_injected, 1);
        let patched = fs::read(&path).unwrap();
        assert_eq!(&patched[..offset], &original[..offset]); // Header and entire FAT unchanged.
        assert_eq!(&patched[offset + 4096..], &original[offset + 4096..]);
        let blob = decompress_lz77(&patched[offset..offset + 4096], None, true).unwrap();
        assert_eq!(
            picture_to_rgba(&find_tim2_files(&blob)[0].pictures[0]).unwrap(),
            rgba
        );
        let new_records = write_texture_inventory(&path, temp.path().join("verified")).unwrap();
        let changed = new_records.iter().find(|r| r.is_header_resource()).unwrap();
        assert_ne!(changed.texture_hash, header.texture_hash);
        assert_eq!(
            new_records.iter().find(|r| r.id == 2).unwrap().texture_hash,
            ordinary.texture_hash
        );
    }

    #[test]
    fn header_font_injection_rejects_overflow_without_touching_archive() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("Data.bin");
        let data = indexed_font_fixture(FAT_OFFSET + NUM_ENTRIES * ENTRY_SIZE + 256);
        fs::write(&path, &data).unwrap();
        let streams = temp.path().join("streams");
        fs::create_dir(&streams).unwrap();
        fs::write(
            streams.join(format!("ID_{HEADER_TEXTURE_ID}.lz77")),
            vec![0; 4097],
        )
        .unwrap();
        let report = inject_patched_texture_streams(&path, &streams).unwrap();
        assert_eq!(report.streams_injected, 0);
        assert!(report.errors[0].contains("does not fit"));
        assert_eq!(fs::read(&path).unwrap(), data);
    }

    #[test]
    fn parses_minimal_tim2() {
        let mut data = b"TIM2".to_vec();
        data.extend([4, 0]);
        data.extend(1_u16.to_le_bytes());
        data.extend([0; 8]);
        data.extend(52_u32.to_le_bytes());
        data.extend(0_u32.to_le_bytes());
        data.extend(4_u32.to_le_bytes());
        data.extend(48_u16.to_le_bytes());
        data.extend(0_u16.to_le_bytes());
        data.extend([0, 0, 0, 0]);
        data.extend(1_u16.to_le_bytes());
        data.extend(1_u16.to_le_bytes());
        data.extend([0; 24]);
        data.extend([1, 2, 3, 4]);

        let parsed = parse_tim2(&data, 0).unwrap();
        assert_eq!(parsed.pictures.len(), 1);
        assert_eq!(parsed.pictures[0].image_type, 0);
        assert_eq!(parsed.pictures[0].width, 1);
    }

    #[test]
    fn ignores_corrupt_tim2_size_overflow() {
        let mut data = b"TIM2".to_vec();
        data.extend([4, 0]);
        data.extend(1_u16.to_le_bytes());
        data.extend([0; 8]);
        data.extend(48_u32.to_le_bytes());
        data.extend(0_u32.to_le_bytes());
        data.extend(u32::MAX.to_le_bytes());
        data.extend(48_u16.to_le_bytes());
        data.extend(0_u16.to_le_bytes());
        data.extend([0, 0, 0, 0]);
        data.extend(1_u16.to_le_bytes());
        data.extend(1_u16.to_le_bytes());
        data.extend([0; 24]);

        assert!(find_tim2_files(&data).is_empty());
    }

    #[test]
    fn rejects_invalid_lz77_offset_without_overflow() {
        let err = lz77_stream_size(b"LZ77", usize::MAX - 1).unwrap_err();
        assert!(err.to_string().contains("no valid LZ77 stream"));
    }

    #[test]
    fn normalizes_dirty_transparent_pixels() {
        let mut rgba = vec![
            255, 128, 64, 0, // dirty transparent
            10, 20, 30, 255, // opaque
            0, 0, 0, 0, // clean transparent
            100, 200, 50, 0, // dirty transparent
        ];
        let modified = normalize_transparent_pixels(&mut rgba);
        assert_eq!(modified, 2);
        assert_eq!(rgba[0..4], [0, 0, 0, 0]);
        assert_eq!(rgba[4..8], [10, 20, 30, 255]);
        assert_eq!(rgba[8..12], [0, 0, 0, 0]);
        assert_eq!(rgba[12..16], [0, 0, 0, 0]);
    }

    #[test]
    fn closest_index_picks_transparent_palette_entry() {
        let palette = [
            [255, 255, 255, 255],
            [128, 0, 0, 255],
            [0, 0, 0, 0], // transparent at index 2
        ];
        let color = [255, 128, 64, 0]; // transparent with dirty RGB
        assert_eq!(closest_index(&color, &palette), 2);
    }
}
