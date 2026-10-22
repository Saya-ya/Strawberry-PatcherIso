#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Parser;
use straw_core::{
    build_iso_with_patched_data, copy_file_creating_parent, english_glyph_map,
    extract_lz77_scripts_to_dir, inject_elf_into_iso, inject_patched_texture_streams,
    patch_textures_from_manifest_with_glyph_map, patch_translated_elf, patch_translated_scripts,
    spanish_glyph_map, GlyphMap,
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

    let staging = tempfile::tempdir().context("no se pudo crear staging temporal")?;
    let csv = staging.path().join("dialogo.csv");
    let scripts = staging.path().join("scripts");
    let patched_data = staging.path().join("Data_patched.bin");
    let translated_elf = staging.path().join("SLPS_256.11_translated");
    let count = straw_db::export_translations_csv(&db, &csv, true).await?;
    if count == 0 {
        bail!("La base de datos no contiene traducciones.");
    }
    println!("Idioma: {target_lang}; traducciones exportadas: {count}");

    copy_file_creating_parent(&data_bin, &patched_data)?;
    let extracted = extract_lz77_scripts_to_dir(&data_bin, &scripts)?;
    println!("Scripts originales extraídos: {extracted}");
    let report = patch_translated_scripts(&patched_data, &scripts, &csv, Some(&glyph_map))?;
    if !report.errors.is_empty() {
        bail!("Fallaron scripts: {}", report.errors.join("; "));
    }
    println!(
        "Scripts: {}/{}; textos: {}/{}",
        report.scripts_patched, report.scripts_total, report.rows_applied, report.rows_total
    );

    let elf_report = patch_translated_elf(&elf, &translated_elf, &csv, Some(&glyph_map))?;
    println!(
        "ELF: {} aplicadas, {} demasiado largas, {} omitidas",
        elf_report.rows_patched, elf_report.skipped_too_large, elf_report.skipped_other
    );

    let manifest = args.texturas.join("manifest.json");
    if manifest.is_file() {
        let texture_out = staging.path().join("textures");
        let texture_report = patch_textures_from_manifest_with_glyph_map(
            &data_bin,
            &manifest,
            &texture_out,
            Some(&glyph_map),
        )?;
        if !texture_report.errors.is_empty() {
            bail!("Fallaron texturas: {}", texture_report.errors.join("; "));
        }
        let injected = inject_patched_texture_streams(&patched_data, &texture_out)?;
        if !injected.errors.is_empty() {
            bail!(
                "Falló la inyección de texturas: {}",
                injected.errors.join("; ")
            );
        }
        println!(
            "Texturas: {} streams parcheados e inyectados",
            injected.streams_injected
        );
    } else {
        println!("No existe {}; se omiten texturas", manifest.display());
    }

    if args.output.exists() && same_file(&iso, &args.output) {
        bail!("La salida no puede sobrescribir la ISO original");
    }
    build_iso_with_patched_data(&iso, &patched_data, &args.output)?;
    inject_elf_into_iso(&args.output, &elf, &translated_elf)?;
    println!("ISO parcheada: {}", args.output.display());
    Ok(())
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
