#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use anyhow::{bail, Context, Result};
use clap::Parser;
use straw_core::{
    build_iso_with_patched_data, copy_file_creating_parent, english_glyph_map,
    extract_lz77_scripts_to_dir, inject_elf_into_iso, inject_patched_texture_streams,
    patch_textures_from_manifest_with_options, patch_translated_elf,
    patch_translated_scripts_with_options, spanish_glyph_map, GlyphMap,
};

mod gui;

#[derive(Debug, Parser)]
#[command(
    name = "straw-patcher",
    about = "Aplica una traducción a una ISO de Strawberry Panic!"
)]
struct Args {
    /// Ejecuta el modo de consola (usado también por la interfaz gráfica)
    #[arg(long, hide = true)]
    cli: bool,
    /// Directorio con la ISO original, Data.bin y SLPS_256.11
    #[arg(long, default_value = "originales")]
    originales: PathBuf,
    /// Base de datos SQLite de traducción
    #[arg(long, default_value = "translation_manager.db")]
    db: PathBuf,
    /// Carpeta con manifest.json y los PNG de texturas editados
    #[arg(long, default_value = "texturas")]
    texturas: PathBuf,
    /// ISO generada
    #[arg(short, long, default_value = "output/Strawberry_Patched.iso")]
    output: PathBuf,
    /// Carpeta de trabajo para los archivos intermedios (por defecto, junto a la ISO de salida).
    /// Evita usar la carpeta temporal del sistema, que suele estar en memoria y puede ser pequeña.
    #[arg(long)]
    work_dir: Option<PathBuf>,
    /// Número de hilos de trabajo (0 = detección automática)
    #[arg(long, default_value_t = 0)]
    workers: usize,
}

fn main() -> Result<()> {
    let args = Args::parse();
    if !args.cli {
        return eframe::run_native(
            "StrawPatcher — Strawberry Panic!",
            eframe::NativeOptions::default(),
            Box::new(|_cc| Ok(Box::new(gui::PatcherGui::default()))),
        )
        .map_err(|err| anyhow::anyhow!("No se pudo iniciar la interfaz: {err}"));
    }
    tokio::runtime::Runtime::new()?.block_on(run_cli(args))
}

async fn run_cli(args: Args) -> Result<()> {
    let iso = find_file(&args.originales, &[".iso"])?;
    let data_bin = args.originales.join("Data.bin");
    let elf = args.originales.join("SLPS_256.11");
    for path in [&data_bin, &elf, &args.db] {
        if !path.is_file() {
            bail!("No se encontró el archivo requerido: {}", path.display());
        }
    }

    let db = straw_db::connect_path(&args.db).await?;
    let target_lang: String = straw_db::get_setting(&db, "target_lang", "es".to_owned()).await?;
    let glyph_map: GlyphMap = match target_lang.as_str() {
        "en" => english_glyph_map(),
        "custom" => straw_db::get_setting(&db, "custom_glyph_map", GlyphMap::new()).await?,
        _ => spanish_glyph_map(),
    };

    let workers = resolve_workers(args.workers);

    // Stage next to the output by default: Data_patched.bin is ~2 GB, so the
    // system temp folder (often a small in-memory tmpfs) is a poor default.
    let work_dir = match &args.work_dir {
        Some(dir) => dir.clone(),
        None => args
            .output
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(".")),
    };
    std::fs::create_dir_all(&work_dir)
        .with_context(|| format!("No se pudo crear la carpeta de trabajo {}", work_dir.display()))?;
    let staging = tempfile::Builder::new()
        .prefix("straw-patcher-")
        .tempdir_in(&work_dir)
        .with_context(|| {
            format!(
                "No se pudo crear la carpeta temporal en {}",
                work_dir.display()
            )
        })?;
    println!("Carpeta de trabajo: {}", staging.path().display());
    println!("Hilos de trabajo: {workers}");

    let total = Instant::now();
    let csv = staging.path().join("dialogo.csv");
    let scripts = staging.path().join("scripts");
    let patched_data = staging.path().join("Data_patched.bin");
    let translated_elf = staging.path().join("SLPS_256.11_translated");
    let count = straw_db::export_translations_csv(&db, &csv, true).await?;
    if count == 0 {
        bail!("La base de datos no contiene traducciones.");
    }
    println!("Idioma: {target_lang}; traducciones exportadas: {count}");

    let started = Instant::now();
    copy_file_creating_parent(&data_bin, &patched_data)?;
    println!(
        "Copia de Data.bin preparada ({:.1} s)",
        started.elapsed().as_secs_f32()
    );

    let started = Instant::now();
    let extracted = extract_lz77_scripts_to_dir(&data_bin, &scripts)?;
    println!(
        "Scripts originales extraídos: {extracted} ({:.1} s)",
        started.elapsed().as_secs_f32()
    );

    let started = Instant::now();
    let report = patch_translated_scripts_with_options(
        &patched_data,
        &scripts,
        &csv,
        Some(&glyph_map),
        workers,
    )?;
    if !report.errors.is_empty() {
        bail!("Fallaron scripts: {}", report.errors.join("; "));
    }
    println!(
        "Scripts: {}/{}; textos: {}/{} ({:.1} s)",
        report.scripts_patched,
        report.scripts_total,
        report.rows_applied,
        report.rows_total,
        started.elapsed().as_secs_f32()
    );

    let started = Instant::now();
    let elf_report = patch_translated_elf(&elf, &translated_elf, &csv, Some(&glyph_map))?;
    println!(
        "ELF: {} aplicadas, {} demasiado largas, {} omitidas ({:.1} s)",
        elf_report.rows_patched,
        elf_report.skipped_too_large,
        elf_report.skipped_other,
        started.elapsed().as_secs_f32()
    );

    let manifest = args.texturas.join("manifest.json");
    if manifest.is_file() {
        let texture_out = staging.path().join("textures");
        let started = Instant::now();
        let texture_report = patch_textures_from_manifest_with_options(
            &data_bin,
            &manifest,
            &texture_out,
            Some(&glyph_map),
            workers,
        )?;
        if !texture_report.errors.is_empty() {
            bail!("Fallaron texturas: {}", texture_report.errors.join("; "));
        }
        let patched_elapsed = started.elapsed().as_secs_f32();
        let started = Instant::now();
        let injected = inject_patched_texture_streams(&patched_data, &texture_out)?;
        if !injected.errors.is_empty() {
            bail!(
                "Falló la inyección de texturas: {}",
                injected.errors.join("; ")
            );
        }
        println!(
            "Texturas: {} archivos, {} streams inyectados (parcheo {:.1} s, inyección {:.1} s)",
            texture_report.files_processed,
            injected.streams_injected,
            patched_elapsed,
            started.elapsed().as_secs_f32()
        );
    } else {
        println!("No existe {}; se omiten texturas", manifest.display());
    }

    if args.output.exists() && same_file(&iso, &args.output) {
        bail!("La salida no puede sobrescribir la ISO original");
    }
    let started = Instant::now();
    build_iso_with_patched_data(&iso, &patched_data, &args.output)?;
    println!("ISO generada ({:.1} s)", started.elapsed().as_secs_f32());

    let started = Instant::now();
    inject_elf_into_iso(&args.output, &elf, &translated_elf)?;
    println!("ELF inyectado ({:.1} s)", started.elapsed().as_secs_f32());
    println!(
        "ISO parcheada: {} (total {:.1} s)",
        args.output.display(),
        total.elapsed().as_secs_f32()
    );
    Ok(())
}

/// Resolves the worker count, defaulting to a sensible share of the CPU count.
fn resolve_workers(requested: usize) -> usize {
    let available = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(2);
    if requested == 0 {
        available.clamp(1, 8)
    } else {
        requested.clamp(1, 64)
    }
}

fn find_file(dir: &Path, suffixes: &[&str]) -> Result<PathBuf> {
    let mut candidates = std::fs::read_dir(dir)
        .with_context(|| format!("No se pudo leer {}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.is_file()
                && suffixes.iter().any(|suffix| {
                    path.extension().is_some_and(|ext| {
                        format!(".{}", ext.to_string_lossy()).eq_ignore_ascii_case(suffix)
                    })
                })
        })
        .collect::<Vec<_>>();
    candidates.sort();
    candidates
        .into_iter()
        .next()
        .with_context(|| format!("No se encontró ISO en {}", dir.display()))
}

fn same_file(left: &Path, right: &Path) -> bool {
    std::fs::canonicalize(left).ok() == std::fs::canonicalize(right).ok()
}
