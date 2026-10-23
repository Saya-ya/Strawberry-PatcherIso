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

mod branding;
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
        let mut options = eframe::NativeOptions::default();
        options.viewport.icon = Some(std::sync::Arc::new(branding::window_icon()));
        return eframe::run_native(
            "StrawPatcher — Strawberry Panic!",
            options,
            Box::new(|_cc| Ok(Box::new(gui::PatcherGui::default()))),
        )
        .map_err(|err| anyhow::anyhow!("No se pudo iniciar la interfaz: {err}"));
    }
    tokio::runtime::Runtime::new()?.block_on(run_cli(args))
}

async fn run_cli(args: Args) -> Result<()> {
    let args = resolve_args_against_exe(args);
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
    if !manifest.is_file() {
        // Textures are optional, but say it loudly: a text-only ISO can look
        // wrong (wrong font glyphs) without any obvious cause.
        println!(
            "AVISO: no existe {}; se omiten las texturas. La ISO saldra solo con textos y ELF.",
            manifest.display()
        );
    } else {
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
            bail!(
                "Fallaron {} texturas. Revisa que '{}' contenga manifest.json y la subcarpeta 'uploads' con los PNG: {}",
                texture_report.errors.len(),
                args.texturas.display(),
                texture_report.errors.join("; ")
            );
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

/// Anchors relative input paths to the folder containing the executable.
///
/// Explorer does not set the working directory to the executable's folder when
/// a program is launched by double-click, so a bare `originales/` would resolve
/// against an unpredictable location and the default layout would break. Any
/// path that already exists as given, and any absolute path, is left untouched
/// so explicit overrides keep working.
fn resolve_args_against_exe(mut args: Args) -> Args {
    let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    else {
        return args;
    };
    for path in [
        &mut args.originales,
        &mut args.db,
        &mut args.texturas,
    ] {
        anchor_relative_to(path, &exe_dir, false);
    }
    anchor_relative_to(&mut args.output, &exe_dir, true);
    if let Some(work_dir) = args.work_dir.as_mut() {
        anchor_relative_to(work_dir, &exe_dir, true);
    }
    args
}

/// Points a relative `path` at `base_dir` when the files are there.
///
/// Windows does not set the working directory to the executable's folder when
/// a program is launched by double-click, so a bare `originales/` would resolve
/// against an unpredictable location. Anything that already exists as given is
/// left alone, so explicit overrides keep priority.
fn anchor_relative_to(path: &mut PathBuf, base_dir: &Path, allow_missing: bool) {
    if path.is_absolute() || path.exists() {
        return;
    }
    let candidate = base_dir.join(&*path);
    let exists_under_base = candidate.exists();
    // For a path that does not exist yet (the output ISO inside a not-yet
    // created `output/` folder) it is enough that it stays inside base_dir.
    let stays_in_base = candidate
        .strip_prefix(base_dir)
        .ok()
        .map(|rest| !rest.as_os_str().is_empty())
        .unwrap_or(false);
    let parent_ready = candidate.parent().is_some_and(Path::exists);
    if exists_under_base || (allow_missing && (parent_ready || stays_in_base)) {
        *path = candidate;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_relative_paths_to_the_executable_folder() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path();
        std::fs::create_dir_all(base.join("originales")).unwrap();
        std::fs::create_dir_all(base.join("texturas").join("uploads")).unwrap();
        std::fs::write(base.join("translation_manager.db"), b"db").unwrap();

        let mut db = PathBuf::from("translation_manager.db");
        anchor_relative_to(&mut db, base, false);
        assert_eq!(db, base.join("translation_manager.db"));

        let mut originales = PathBuf::from("originales");
        anchor_relative_to(&mut originales, base, false);
        assert_eq!(originales, base.join("originales"));

        let mut texturas = PathBuf::from("texturas");
        anchor_relative_to(&mut texturas, base, false);
        assert_eq!(texturas, base.join("texturas"));
    }

    #[test]
    fn keeps_absolute_and_existing_relative_paths_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path();
        let absolute = PathBuf::from("/tmp/ya/existe.iso");
        let mut path = absolute.clone();
        anchor_relative_to(&mut path, base, true);
        assert_eq!(path, absolute);

        let mut cwd_relative = PathBuf::from("originales");
        std::fs::create_dir("originales").unwrap();
        anchor_relative_to(&mut cwd_relative, base, false);
        assert_eq!(cwd_relative, PathBuf::from("originales"));
        std::fs::remove_dir("originales").unwrap();
    }

    #[test]
    fn anchors_output_that_does_not_exist_yet() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path();
        let mut output = PathBuf::from("output/Strawberry_Patched.iso");
        anchor_relative_to(&mut output, base, true);
        assert_eq!(
            output,
            base.join("output").join("Strawberry_Patched.iso")
        );
    }
}
