use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use anyhow::{Context, Result};
use serde::{de::DeserializeOwned, Serialize};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    Row, SqliteConnection, SqlitePool,
};
use straw_core::{
    analyze_script_dec, check_fit, extract_elf_strings, spanish_glyph_map,
    wrap_script_translation_for, FitStatus, ImportedText, TextSource,
};

pub const DEFAULT_DB_PATH: &str = "work/translation_manager.db";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptSummary {
    pub id: i64,
    pub source: String,
    pub script_type: String,
    pub variant: String,
    pub rebuild_mode: String,
    pub is_supported: bool,
    pub total_texts: i64,
    pub translated_texts: i64,
    pub total_sections: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEntrySummary {
    pub id: i64,
    pub script_id: i64,
    pub source: String,
    pub byte_offset: i64,
    pub byte_offset_hex: String,
    pub section_id: i64,
    pub section_order: i64,
    pub original_text: String,
    pub translated_text: String,
    pub is_translated: bool,
    pub needs_shift: bool,
    pub fit_status: String,
    pub segment_capacity: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptDetail {
    pub script: ScriptSummary,
    pub texts: Vec<TextEntrySummary>,
    pub total: i64,
    pub page: i64,
    pub limit: i64,
    pub total_pages: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    pub id: i64,
    pub script_id: i64,
    pub page: i64,
    pub byte_offset: i64,
    pub byte_offset_hex: String,
    pub section_id: i64,
    pub section_order: i64,
    pub original_text: String,
    pub translated_text: String,
    pub is_translated: bool,
    pub needs_shift: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslationStats {
    pub scripts: i64,
    pub text_entries: i64,
    pub translated_entries: i64,
    pub needs_shift_entries: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildSummary {
    pub id: i64,
    pub started_at: String,
    pub finished_at: String,
    pub status: String,
    pub build_type: String,
    pub iso_path: String,
    pub step: String,
    pub progress_pct: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedTranslation {
    pub source: String,
    pub file_id: String,
    pub offset: String,
    pub original_text: String,
    pub translated_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    pub scripts_imported: usize,
    pub texts_imported: usize,
    pub translations_preserved: usize,
    pub elf_texts_imported: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslationImportReport {
    pub source_rows: usize,
    pub imported: usize,
    pub skipped_missing: usize,
    pub skipped_unchanged: usize,
}

pub async fn connect(database_url: &str) -> Result<SqlitePool> {
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect(database_url)
        .await
        .with_context(|| format!("failed to connect to SQLite database {database_url}"))
}

pub async fn connect_path(path: impl AsRef<Path>) -> Result<SqlitePool> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true);
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .with_context(|| format!("failed to connect to SQLite database {}", path.display()))
}

pub async fn init_db(pool: &SqlitePool) -> Result<()> {
    create_schema(pool).await?;
    seed_default_settings(pool).await?;
    normalize_stored_translation_linebreaks(pool).await?;
    rebuild_fts_if_empty(pool).await?;
    Ok(())
}

pub async fn import_extracted_texts(
    pool: &SqlitePool,
    scripts_dir: impl AsRef<Path>,
    elf_path: impl AsRef<Path>,
) -> Result<ImportReport> {
    import_extracted_texts_with_progress(pool, scripts_dir, elf_path, |_| {}).await
}

pub async fn import_extracted_texts_with_progress<F>(
    pool: &SqlitePool,
    scripts_dir: impl AsRef<Path>,
    elf_path: impl AsRef<Path>,
    mut progress: F,
) -> Result<ImportReport>
where
    F: FnMut(&str),
{
    let scripts_dir = scripts_dir.as_ref();
    let elf_path = elf_path.as_ref();

    progress("Leyendo traducciones existentes para preservarlas");
    let translations = existing_translations(pool).await?;
    progress(&format!(
        "Traducciones existentes detectadas: {}",
        translations.len()
    ));

    progress(&format!(
        "Escaneando scripts .dec en {}",
        scripts_dir.display()
    ));
    let mut scripts = Vec::new();
    if scripts_dir.exists() {
        let mut scanned = 0;
        for entry in std::fs::read_dir(scripts_dir)
            .with_context(|| format!("failed to read {}", scripts_dir.display()))?
        {
            let path = entry?.path();
            if path.extension().is_some_and(|ext| ext == "dec") {
                scanned += 1;
                if scanned == 1 || scanned % 100 == 0 {
                    progress(&format!("Analizando scripts .dec: {scanned}"));
                }
                if let Some(script) = analyze_script_dec(&path)? {
                    scripts.push(script);
                }
            }
        }
    }
    scripts.sort_by_key(|script| script.script_id);
    let script_text_count: usize = scripts.iter().map(|script| script.texts.len()).sum();
    progress(&format!(
        "Scripts analizables: {}, textos de script detectados: {}",
        scripts.len(),
        script_text_count
    ));

    progress(&format!(
        "Extrayendo textos ELF desde {}",
        elf_path.display()
    ));
    let elf_texts = if elf_path.exists() {
        extract_elf_strings(elf_path)?
    } else {
        Vec::new()
    };
    progress(&format!("Textos ELF detectados: {}", elf_texts.len()));

    let mut translations_preserved = 0;
    let mut texts_imported = 0;

    let mut tx = pool.begin().await?;

    progress("Limpiando tablas actuales text_entries y scripts");
    sqlx::query("DELETE FROM text_entries")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM scripts").execute(&mut *tx).await?;

    progress("Insertando scripts y textos en SQLite");
    for (script_index, script) in scripts.iter().enumerate() {
        sqlx::query(
            r#"
            INSERT INTO scripts (id, source, script_type, variant, rebuild_mode, is_supported, total_texts, translated_texts, total_sections)
            VALUES (?, 'SCRIPT', ?, ?, ?, 1, ?, 0, ?)
            "#,
        )
        .bind(script.script_id)
        .bind(&script.script_type)
        .bind(&script.variant)
        .bind(&script.rebuild_mode)
        .bind(script.texts.len() as i64)
        .bind(script.total_sections)
        .execute(&mut *tx)
        .await?;

        for text in &script.texts {
            if insert_imported_text(&mut *tx, text, &translations).await? {
                translations_preserved += 1;
            }
            texts_imported += 1;
            if texts_imported == 1 || texts_imported % 1000 == 0 {
                progress(&format!(
                    "Textos insertados: {texts_imported}/{script_text_count} (script {} de {})",
                    script_index + 1,
                    scripts.len()
                ));
            }
        }
        refresh_script_translated_count_on_connection(&mut *tx, script.script_id).await?;
        if script_index == 0 || (script_index + 1) % 100 == 0 || script_index + 1 == scripts.len() {
            progress(&format!(
                "Scripts insertados: {}/{}",
                script_index + 1,
                scripts.len()
            ));
        }
    }

    if !elf_texts.is_empty() {
        progress("Insertando textos ELF en SQLite");
        sqlx::query(
            r#"
            INSERT INTO scripts (id, source, script_type, variant, rebuild_mode, is_supported, total_texts, translated_texts, total_sections)
            VALUES (-1, 'ELF', 'ELF', '', 'elf', 1, ?, 0, 1)
            "#,
        )
        .bind(elf_texts.len() as i64)
        .execute(&mut *tx)
        .await?;

        for (index, text) in elf_texts.iter().enumerate() {
            if insert_imported_text(&mut *tx, text, &translations).await? {
                translations_preserved += 1;
            }
            texts_imported += 1;
            if index == 0 || (index + 1) % 100 == 0 || index + 1 == elf_texts.len() {
                progress(&format!(
                    "Textos ELF insertados: {}/{}",
                    index + 1,
                    elf_texts.len()
                ));
            }
        }
        refresh_script_translated_count_on_connection(&mut *tx, -1).await?;
    }

    progress("Reconstruyendo indice de busqueda FTS");
    rebuild_fts_on_connection(&mut *tx).await?;
    tx.commit().await?;
    progress(&format!(
        "Importacion SQLite terminada: {} textos, {} traducciones preservadas",
        texts_imported, translations_preserved
    ));

    Ok(ImportReport {
        scripts_imported: scripts.len() + usize::from(!elf_texts.is_empty()),
        texts_imported,
        translations_preserved,
        elf_texts_imported: elf_texts.len(),
    })
}

pub async fn get_setting<T>(pool: &SqlitePool, key: &str, default: T) -> Result<T>
where
    T: DeserializeOwned,
{
    let row = sqlx::query("SELECT value FROM settings WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await?;

    let Some(row) = row else {
        return Ok(default);
    };
    let value: Option<String> = row.try_get("value")?;
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return Ok(default);
    };

    match serde_json::from_str(&value) {
        Ok(parsed) => Ok(parsed),
        Err(_) => Ok(default),
    }
}

pub async fn set_setting<T>(pool: &SqlitePool, key: &str, value: &T) -> Result<()>
where
    T: Serialize + ?Sized,
{
    let serialized = serde_json::to_string(value)?;
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES (?, ?) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(serialized)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn import_translations_from_db(
    pool: &SqlitePool,
    source_db_path: impl AsRef<Path>,
) -> Result<TranslationImportReport> {
    let source_db_path = source_db_path.as_ref();
    let options = SqliteConnectOptions::new()
        .filename(source_db_path)
        .read_only(true);
    let source_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .with_context(|| {
            format!(
                "failed to open source database {}",
                source_db_path.display()
            )
        })?;

    let source_rows = sqlx::query(
        r#"
        SELECT source, script_id, byte_offset, original_text, translated_text
        FROM text_entries
        WHERE translated_text IS NOT NULL AND trim(translated_text) != ''
        ORDER BY script_id, byte_offset
        "#,
    )
    .fetch_all(&source_pool)
    .await
    .with_context(|| {
        format!(
            "{} is not a compatible translation database",
            source_db_path.display()
        )
    })?;

    let mut report = TranslationImportReport {
        source_rows: source_rows.len(),
        imported: 0,
        skipped_missing: 0,
        skipped_unchanged: 0,
    };
    let glyph_map = spanish_glyph_map();
    let mut touched_scripts = HashSet::new();
    let mut tx = pool.begin().await?;

    for row in source_rows {
        let source = row
            .try_get::<Option<String>, _>("source")?
            .unwrap_or_else(|| "SCRIPT".to_owned());
        let script_id: i64 = row.try_get("script_id")?;
        let byte_offset: i64 = row.try_get("byte_offset")?;
        let original_text: String = row.try_get("original_text")?;
        let text_source = if source == "ELF" {
            TextSource::Elf
        } else {
            TextSource::Script
        };
        let translated_text = canonicalize_translation(
            &row.try_get::<String, _>("translated_text")?,
            text_source,
            script_id,
        );
        if translated_text.trim().is_empty() {
            report.skipped_unchanged += 1;
            continue;
        }

        let Some(target) = sqlx::query(
            r#"
            SELECT id, translated_text, original_text, segment_capacity
            FROM text_entries
            WHERE source = ? AND script_id = ? AND byte_offset = ? AND original_text = ?
            "#,
        )
        .bind(&source)
        .bind(script_id)
        .bind(byte_offset)
        .bind(&original_text)
        .fetch_optional(&mut *tx)
        .await?
        else {
            report.skipped_missing += 1;
            continue;
        };

        let current = target
            .try_get::<Option<String>, _>("translated_text")?
            .map(|text| canonicalize_translation(&text, text_source, script_id))
            .unwrap_or_default();
        if current == translated_text {
            report.skipped_unchanged += 1;
            continue;
        }

        let id: i64 = target.try_get("id")?;
        let original: String = target.try_get("original_text")?;
        let fallback_capacity = match text_source {
            TextSource::Script => original.encode_utf16().count() * 2 + 2,
            TextSource::Elf => original.len(),
        };
        let capacity = target
            .try_get::<Option<i64>, _>("segment_capacity")?
            .unwrap_or(0)
            .max(fallback_capacity as i64)
            .max(1) as usize;
        let fit = check_fit(&translated_text, text_source, capacity, Some(&glyph_map));

        sqlx::query(
            r#"
            UPDATE text_entries
            SET translated_text = ?,
                is_translated = 1,
                needs_shift = ?,
                fit_status = ?,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = ?
            "#,
        )
        .bind(&translated_text)
        .bind(if fit.status == FitStatus::NeedsShift {
            1
        } else {
            0
        })
        .bind(fit_status_label(fit.status))
        .bind(id)
        .execute(&mut *tx)
        .await?;
        report.imported += 1;
        touched_scripts.insert(script_id);
    }
    tx.commit().await?;

    for script_id in touched_scripts {
        refresh_script_translated_count(pool, script_id).await?;
    }
    rebuild_fts(pool).await?;

    Ok(report)
}

pub async fn list_scripts(pool: &SqlitePool) -> Result<Vec<ScriptSummary>> {
    let rows = sqlx::query(
        r#"
        SELECT s.id, s.source, s.script_type, s.variant, s.rebuild_mode, s.is_supported,
               s.total_texts,
               (SELECT COUNT(*) FROM text_entries t
                WHERE t.script_id = s.id AND t.is_translated = 1) AS translated_texts,
               s.total_sections
        FROM scripts s
        ORDER BY id
        "#,
    )
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(ScriptSummary {
                id: row.try_get("id")?,
                source: row
                    .try_get::<Option<String>, _>("source")?
                    .unwrap_or_default(),
                script_type: row
                    .try_get::<Option<String>, _>("script_type")?
                    .unwrap_or_default(),
                variant: row
                    .try_get::<Option<String>, _>("variant")?
                    .unwrap_or_default(),
                rebuild_mode: row
                    .try_get::<Option<String>, _>("rebuild_mode")?
                    .unwrap_or_default(),
                is_supported: row.try_get::<i64, _>("is_supported")? != 0,
                total_texts: row.try_get::<Option<i64>, _>("total_texts")?.unwrap_or(0),
                translated_texts: row
                    .try_get::<Option<i64>, _>("translated_texts")?
                    .unwrap_or(0),
                total_sections: row
                    .try_get::<Option<i64>, _>("total_sections")?
                    .unwrap_or(0),
            })
        })
        .collect()
}

pub async fn get_script(pool: &SqlitePool, script_id: i64) -> Result<Option<ScriptSummary>> {
    let row = sqlx::query(
        r#"
        SELECT s.id, s.source, s.script_type, s.variant, s.rebuild_mode, s.is_supported,
               s.total_texts,
               (SELECT COUNT(*) FROM text_entries t
                WHERE t.script_id = s.id AND t.is_translated = 1) AS translated_texts,
               s.total_sections
        FROM scripts s
        WHERE s.id = ?
        "#,
    )
    .bind(script_id)
    .fetch_optional(pool)
    .await?;

    row.map(script_from_row).transpose()
}

pub async fn get_script_detail(
    pool: &SqlitePool,
    script_id: i64,
    page: i64,
    limit: i64,
) -> Result<Option<ScriptDetail>> {
    get_script_detail_filtered(pool, script_id, page, limit, "all").await
}

pub async fn get_script_detail_filtered(
    pool: &SqlitePool,
    script_id: i64,
    page: i64,
    limit: i64,
    status_filter: &str,
) -> Result<Option<ScriptDetail>> {
    let Some(script) = get_script(pool, script_id).await? else {
        return Ok(None);
    };

    let page = page.max(1);
    let limit = limit.clamp(1, 200);
    let offset = (page - 1) * limit;
    let status_condition = status_filter_condition(status_filter);

    let total_sql = format!(
        "SELECT COUNT(*) AS count FROM text_entries WHERE script_id = ? {status_condition}"
    );
    let total: i64 = sqlx::query(&total_sql)
        .bind(script_id)
        .fetch_one(pool)
        .await?
        .try_get("count")?;

    let rows_sql = format!(
        r#"
        SELECT id, script_id, source, byte_offset, section_id, section_order,
               original_text, translated_text, is_translated, needs_shift,
               fit_status, segment_capacity
        FROM text_entries
        WHERE script_id = ? {status_condition}
        ORDER BY section_id, section_order
        LIMIT ? OFFSET ?
        "#
    );
    let rows = sqlx::query(&rows_sql)
        .bind(script_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;

    let texts = rows
        .into_iter()
        .map(text_entry_from_row)
        .collect::<Result<Vec<_>>>()?;
    let total_pages = if total > 0 {
        (total + limit - 1) / limit
    } else {
        1
    };

    Ok(Some(ScriptDetail {
        script,
        texts,
        total,
        page,
        limit,
        total_pages,
    }))
}

pub async fn get_text_entry(pool: &SqlitePool, entry_id: i64) -> Result<Option<TextEntrySummary>> {
    let row = sqlx::query(
        r#"
        SELECT id, script_id, source, byte_offset, section_id, section_order,
               original_text, translated_text, is_translated, needs_shift,
               fit_status, segment_capacity
        FROM text_entries
        WHERE id = ?
        "#,
    )
    .bind(entry_id)
    .fetch_optional(pool)
    .await?;

    row.map(text_entry_from_row).transpose()
}

pub async fn delete_elf_text_entry(pool: &SqlitePool, entry_id: i64) -> Result<bool> {
    let mut tx = pool.begin().await?;
    let deleted =
        sqlx::query("DELETE FROM text_entries WHERE id = ? AND script_id = -1 AND source = 'ELF'")
            .bind(entry_id)
            .execute(&mut *tx)
            .await?
            .rows_affected()
            != 0;
    if deleted {
        sqlx::query(
            "UPDATE scripts SET total_texts = (SELECT COUNT(*) FROM text_entries WHERE script_id = -1), \
             translated_texts = (SELECT COUNT(*) FROM text_entries WHERE script_id = -1 AND is_translated = 1) WHERE id = -1",
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(deleted)
}

pub async fn update_text_entry_translation(
    pool: &SqlitePool,
    entry_id: i64,
    translated_text: &str,
) -> Result<Option<TextEntrySummary>> {
    let Some(existing) = get_text_entry(pool, entry_id).await? else {
        return Ok(None);
    };
    let source = if existing.source == "ELF" {
        TextSource::Elf
    } else {
        TextSource::Script
    };
    let translated_text = canonicalize_translation(translated_text, source, existing.script_id);
    let is_translated = !translated_text.trim().is_empty();
    let fallback_capacity = match source {
        TextSource::Script => existing.original_text.encode_utf16().count() * 2 + 2,
        TextSource::Elf => existing.original_text.len(),
    };
    let capacity = existing
        .segment_capacity
        .max(fallback_capacity as i64)
        .max(1) as usize;
    let glyph_map = spanish_glyph_map();
    let fit = check_fit(&translated_text, source, capacity, Some(&glyph_map));

    sqlx::query(
        r#"
        UPDATE text_entries
        SET translated_text = ?,
            is_translated = ?,
            fit_status = ?,
            needs_shift = ?,
            locked_by = '',
            locked_at = NULL,
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ?
        "#,
    )
    .bind(translated_text)
    .bind(if is_translated { 1 } else { 0 })
    .bind(fit_status_label(fit.status))
    .bind(if fit.status == FitStatus::NeedsShift {
        1
    } else {
        0
    })
    .bind(entry_id)
    .execute(pool)
    .await?;

    refresh_script_translated_count(pool, existing.script_id).await?;
    get_text_entry(pool, entry_id).await
}

pub async fn search_text_entries(
    pool: &SqlitePool,
    query: &str,
    limit: i64,
) -> Result<Vec<SearchResult>> {
    search_text_entries_filtered(pool, query, limit, None, "all").await
}

pub async fn search_text_entries_filtered(
    pool: &SqlitePool,
    query: &str,
    limit: i64,
    script_id: Option<i64>,
    status_filter: &str,
) -> Result<Vec<SearchResult>> {
    let query = query.trim();
    if query.is_empty() && script_id.is_none() && status_filter == "all" {
        return Ok(Vec::new());
    }
    let limit = limit.clamp(1, 500);
    let status_condition = status_filter_condition(status_filter);
    let script_condition = if script_id.is_some() {
        " AND te.script_id = ?"
    } else {
        ""
    };

    let sql = if query.is_empty() {
        format!(
            r#"
        SELECT te.id, te.script_id, te.byte_offset, te.section_id, te.section_order,
               te.original_text, te.translated_text, te.is_translated, te.needs_shift,
               (
                   SELECT COUNT(*)
                   FROM text_entries prev
                   WHERE prev.script_id = te.script_id
                     AND (
                         prev.section_id < te.section_id
                         OR (prev.section_id = te.section_id AND prev.section_order < te.section_order)
                         OR (prev.section_id = te.section_id AND prev.section_order = te.section_order AND prev.id <= te.id)
                     )
               ) AS position_in_script
        FROM text_entries te
        WHERE 1 = 1 {script_condition} {status_condition}
        ORDER BY te.script_id, te.section_id, te.section_order
        LIMIT ?
        "#
        )
    } else {
        format!(
            r#"
        SELECT te.id, te.script_id, te.byte_offset, te.section_id, te.section_order,
               te.original_text, te.translated_text, te.is_translated, te.needs_shift,
               (
                   SELECT COUNT(*)
                   FROM text_entries prev
                   WHERE prev.script_id = te.script_id
                     AND (
                         prev.section_id < te.section_id
                         OR (prev.section_id = te.section_id AND prev.section_order < te.section_order)
                         OR (prev.section_id = te.section_id AND prev.section_order = te.section_order AND prev.id <= te.id)
                     )
               ) AS position_in_script
        FROM text_entries_fts fts
        JOIN text_entries te ON te.id = fts.rowid
        WHERE text_entries_fts MATCH ? {script_condition} {status_condition}
        ORDER BY te.script_id, te.section_id, te.section_order
        LIMIT ?
        "#
        )
    };
    let mut query_builder = sqlx::query(&sql);
    if !query.is_empty() {
        query_builder = query_builder.bind(query);
    }
    if let Some(script_id) = script_id {
        query_builder = query_builder.bind(script_id);
    }
    let rows = query_builder.bind(limit).fetch_all(pool).await?;

    rows.into_iter()
        .map(|row| {
            let byte_offset: i64 = row.try_get("byte_offset")?;
            let position_in_script: i64 = row.try_get("position_in_script")?;
            Ok(SearchResult {
                id: row.try_get("id")?,
                script_id: row.try_get("script_id")?,
                page: ((position_in_script.max(1) - 1) / 50) + 1,
                byte_offset,
                byte_offset_hex: format!("0x{byte_offset:05X}"),
                section_id: row.try_get::<Option<i64>, _>("section_id")?.unwrap_or(0),
                section_order: row.try_get::<Option<i64>, _>("section_order")?.unwrap_or(0),
                original_text: row
                    .try_get::<Option<String>, _>("original_text")?
                    .unwrap_or_default(),
                translated_text: row
                    .try_get::<Option<String>, _>("translated_text")?
                    .unwrap_or_default(),
                is_translated: row.try_get::<i64, _>("is_translated")? != 0,
                needs_shift: row.try_get::<i64, _>("needs_shift")? != 0,
            })
        })
        .collect()
}

fn status_filter_condition(status_filter: &str) -> &'static str {
    match status_filter {
        "translated" => " AND is_translated = 1 AND needs_shift = 0 AND fit_status != 'tight'",
        "warning" => " AND is_translated = 1 AND needs_shift = 0 AND fit_status = 'tight'",
        "needs_shift" => " AND needs_shift = 1",
        "untranslated" => " AND is_translated = 0",
        _ => "",
    }
}

pub async fn translation_stats(pool: &SqlitePool) -> Result<TranslationStats> {
    let row = sqlx::query(
        r#"
        SELECT
            (SELECT COUNT(*) FROM scripts) AS scripts,
            (SELECT COUNT(*) FROM text_entries) AS text_entries,
            (SELECT COUNT(*) FROM text_entries WHERE is_translated = 1) AS translated_entries,
            (SELECT COUNT(*) FROM text_entries WHERE needs_shift = 1) AS needs_shift_entries
        "#,
    )
    .fetch_one(pool)
    .await?;

    Ok(TranslationStats {
        scripts: row.try_get("scripts")?,
        text_entries: row.try_get("text_entries")?,
        translated_entries: row.try_get("translated_entries")?,
        needs_shift_entries: row.try_get("needs_shift_entries")?,
    })
}

pub async fn recent_builds(pool: &SqlitePool, limit: i64) -> Result<Vec<BuildSummary>> {
    let limit = limit.clamp(1, 50);
    let rows = sqlx::query(
        r#"
        SELECT id, started_at, finished_at, status, build_type, iso_path, step, progress_pct
        FROM build_history
        ORDER BY id DESC
        LIMIT ?
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(BuildSummary {
                id: row.try_get("id")?,
                started_at: row
                    .try_get::<Option<String>, _>("started_at")?
                    .unwrap_or_default(),
                finished_at: row
                    .try_get::<Option<String>, _>("finished_at")?
                    .unwrap_or_default(),
                status: row
                    .try_get::<Option<String>, _>("status")?
                    .unwrap_or_default(),
                build_type: row
                    .try_get::<Option<String>, _>("build_type")?
                    .unwrap_or_default(),
                iso_path: row
                    .try_get::<Option<String>, _>("iso_path")?
                    .unwrap_or_default(),
                step: row
                    .try_get::<Option<String>, _>("step")?
                    .unwrap_or_default(),
                progress_pct: row.try_get::<Option<i64>, _>("progress_pct")?.unwrap_or(0),
            })
        })
        .collect()
}

pub async fn record_build_step(
    pool: &SqlitePool,
    status: &str,
    build_type: &str,
    step: &str,
    progress_pct: i64,
    iso_path: &str,
    error_log: &str,
) -> Result<i64> {
    let result = sqlx::query(
        r#"
        INSERT INTO build_history (finished_at, status, build_type, iso_path, error_log, step, progress_pct)
        VALUES (CURRENT_TIMESTAMP, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(status)
    .bind(build_type)
    .bind(iso_path)
    .bind(error_log)
    .bind(step)
    .bind(progress_pct.clamp(0, 100))
    .execute(pool)
    .await?;

    Ok(result.last_insert_rowid())
}

pub async fn exported_translations(
    pool: &SqlitePool,
    only_translated: bool,
) -> Result<Vec<ExportedTranslation>> {
    let mut sql = String::from(
        r#"
        SELECT source, script_id, byte_offset, original_text, translated_text
        FROM text_entries
        "#,
    );
    if only_translated {
        sql.push_str(" WHERE is_translated = 1");
    }
    sql.push_str(" ORDER BY script_id, section_id, section_order");

    let rows = sqlx::query(&sql).fetch_all(pool).await?;
    rows.into_iter()
        .map(|row| {
            let source = row
                .try_get::<Option<String>, _>("source")?
                .unwrap_or_else(|| "SCRIPT".to_owned());
            let script_id: i64 = row.try_get("script_id")?;
            let byte_offset: i64 = row.try_get("byte_offset")?;
            let file_id = if script_id == -1 {
                "ELF".to_owned()
            } else {
                script_id.to_string()
            };
            let offset = if source == "SCRIPT" {
                format!("0x{byte_offset:05X}")
            } else {
                format!("0x{byte_offset:06X}")
            };
            let text_source = if source == "ELF" {
                TextSource::Elf
            } else {
                TextSource::Script
            };
            Ok(ExportedTranslation {
                source,
                file_id,
                offset,
                original_text: row
                    .try_get::<Option<String>, _>("original_text")?
                    .unwrap_or_default(),
                translated_text: row
                    .try_get::<Option<String>, _>("translated_text")?
                    .map(|text| canonicalize_translation(&text, text_source, script_id))
                    .unwrap_or_default(),
            })
        })
        .collect()
}

pub async fn normalize_stored_translation_linebreaks(pool: &SqlitePool) -> Result<usize> {
    let rows = sqlx::query(
        r#"
        SELECT id, script_id, source, translated_text, segment_capacity
        FROM text_entries
        WHERE translated_text IS NOT NULL
          AND translated_text != ''
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut updated = 0;
    let mut tx = pool.begin().await?;
    for row in rows {
        let id: i64 = row.try_get("id")?;
        let script_id: i64 = row.try_get("script_id")?;
        let source = row
            .try_get::<Option<String>, _>("source")?
            .unwrap_or_else(|| "SCRIPT".to_owned());
        let original: String = row.try_get("translated_text")?;
        let text_source = if source == "ELF" {
            TextSource::Elf
        } else {
            TextSource::Script
        };
        let normalized = canonicalize_translation(&original, text_source, script_id);
        if normalized == original {
            continue;
        }

        let capacity = row
            .try_get::<Option<i64>, _>("segment_capacity")?
            .unwrap_or(0)
            .max(1) as usize;
        let fit = check_fit(&normalized, text_source, capacity, None);

        sqlx::query(
            r#"
            UPDATE text_entries
            SET translated_text = ?,
                is_translated = ?,
                needs_shift = ?,
                fit_status = ?,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = ?
            "#,
        )
        .bind(&normalized)
        .bind(if normalized.trim().is_empty() { 0 } else { 1 })
        .bind(if fit.status == FitStatus::NeedsShift {
            1
        } else {
            0
        })
        .bind(fit_status_label(fit.status))
        .bind(id)
        .execute(&mut *tx)
        .await?;
        updated += 1;
    }
    tx.commit().await?;

    Ok(updated)
}

pub async fn export_translations_csv(
    pool: &SqlitePool,
    csv_path: impl AsRef<Path>,
    only_translated: bool,
) -> Result<usize> {
    let csv_path = csv_path.as_ref();
    if let Some(parent) = csv_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let rows = exported_translations(pool, only_translated).await?;
    let mut writer = csv::WriterBuilder::new()
        .has_headers(false)
        .terminator(csv::Terminator::CRLF)
        .from_path(csv_path)
        .with_context(|| format!("failed to create {}", csv_path.display()))?;
    writer.write_record([
        "source",
        "file_id",
        "offset",
        "original_text",
        "translated_text",
    ])?;
    for row in &rows {
        writer.write_record([
            row.source.as_str(),
            row.file_id.as_str(),
            row.offset.as_str(),
            row.original_text.as_str(),
            row.translated_text.as_str(),
        ])?;
    }
    writer.flush()?;
    Ok(rows.len())
}

async fn refresh_script_translated_count(pool: &SqlitePool, script_id: i64) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE scripts
        SET translated_texts = (
            SELECT COUNT(*) FROM text_entries
            WHERE script_id = ? AND is_translated = 1
        )
        WHERE id = ?
        "#,
    )
    .bind(script_id)
    .bind(script_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn refresh_script_translated_count_on_connection(
    conn: &mut SqliteConnection,
    script_id: i64,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE scripts
        SET translated_texts = (
            SELECT COUNT(*) FROM text_entries
            WHERE script_id = ? AND is_translated = 1
        )
        WHERE id = ?
        "#,
    )
    .bind(script_id)
    .bind(script_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn existing_translations(pool: &SqlitePool) -> Result<HashMap<(i64, i64, String), String>> {
    let rows = sqlx::query(
        r#"
        SELECT source, script_id, byte_offset, original_text, translated_text
        FROM text_entries
        WHERE translated_text IS NOT NULL AND translated_text != ''
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut translations = HashMap::new();
    for row in rows {
        let translated_text: String = row.try_get("translated_text")?;
        let source = row
            .try_get::<Option<String>, _>("source")?
            .unwrap_or_else(|| "SCRIPT".to_owned());
        let text_source = if source == "ELF" {
            TextSource::Elf
        } else {
            TextSource::Script
        };
        translations.insert(
            (
                row.try_get("script_id")?,
                row.try_get("byte_offset")?,
                row.try_get::<String, _>("original_text")?,
            ),
            canonicalize_translation(&translated_text, text_source, row.try_get("script_id")?),
        );
    }
    Ok(translations)
}

async fn insert_imported_text(
    conn: &mut SqliteConnection,
    text: &ImportedText,
    translations: &HashMap<(i64, i64, String), String>,
) -> Result<bool> {
    let translated = translations
        .get(&(text.script_id, text.byte_offset, text.original_text.clone()))
        .cloned()
        .unwrap_or_default();
    let is_translated = !translated.trim().is_empty();
    let capacity = if text.segment_capacity > 0 {
        text.segment_capacity
    } else {
        text.original_bytes
    };
    let source = if text.source == "ELF" {
        TextSource::Elf
    } else {
        TextSource::Script
    };
    let fit = if is_translated {
        check_fit(&translated, source, capacity as usize, None)
    } else {
        check_fit("", source, capacity as usize, None)
    };

    sqlx::query(
        r#"
        INSERT INTO text_entries (
            script_id, source, byte_offset, section_id, section_order,
            original_text, translated_text, original_bytes, segment_start,
            segment_capacity, is_translated, needs_shift, fit_status
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(text.script_id)
    .bind(&text.source)
    .bind(text.byte_offset)
    .bind(text.section_id)
    .bind(text.section_order)
    .bind(&text.original_text)
    .bind(&translated)
    .bind(text.original_bytes)
    .bind(text.segment_start)
    .bind(text.segment_capacity)
    .bind(if is_translated { 1 } else { 0 })
    .bind(if fit.status == FitStatus::NeedsShift {
        1
    } else {
        0
    })
    .bind(fit_status_label(fit.status))
    .execute(&mut *conn)
    .await?;

    Ok(is_translated)
}

fn fit_status_label(status: FitStatus) -> &'static str {
    match status {
        FitStatus::Unchecked => "unchecked",
        FitStatus::Ok => "ok",
        FitStatus::Tight => "tight",
        FitStatus::NeedsShift => "needs_shift",
    }
}

pub fn normalize_translation_linebreaks(text: &str) -> String {
    let normalized = text
        .replace("<lb/>", "\n")
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let lines = normalized
        .split('\n')
        .map(|line| line.trim_end_matches([' ', '\t']))
        .collect::<Vec<_>>();
    let first = lines.iter().position(|line| !line.trim().is_empty());
    let last = lines.iter().rposition(|line| !line.trim().is_empty());
    match (first, last) {
        (Some(first), Some(last)) => lines[first..=last].join("\n"),
        _ => String::new(),
    }
}

fn canonicalize_translation(text: &str, source: TextSource, script_id: i64) -> String {
    let normalized = normalize_translation_linebreaks(text);
    if source == TextSource::Script {
        wrap_script_translation_for(script_id, &normalized)
    } else {
        normalized
    }
}

async fn rebuild_fts(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM text_entries_fts")
        .execute(pool)
        .await?;
    sqlx::query(
        r#"
        INSERT INTO text_entries_fts(rowid, original_text, translated_text)
        SELECT id, original_text, translated_text FROM text_entries
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn rebuild_fts_on_connection(conn: &mut SqliteConnection) -> Result<()> {
    sqlx::query("DELETE FROM text_entries_fts")
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        r#"
        INSERT INTO text_entries_fts(rowid, original_text, translated_text)
        SELECT id, original_text, translated_text FROM text_entries
        "#,
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

fn script_from_row(row: sqlx::sqlite::SqliteRow) -> Result<ScriptSummary> {
    Ok(ScriptSummary {
        id: row.try_get("id")?,
        source: row
            .try_get::<Option<String>, _>("source")?
            .unwrap_or_default(),
        script_type: row
            .try_get::<Option<String>, _>("script_type")?
            .unwrap_or_default(),
        variant: row
            .try_get::<Option<String>, _>("variant")?
            .unwrap_or_default(),
        rebuild_mode: row
            .try_get::<Option<String>, _>("rebuild_mode")?
            .unwrap_or_default(),
        is_supported: row.try_get::<i64, _>("is_supported")? != 0,
        total_texts: row.try_get::<Option<i64>, _>("total_texts")?.unwrap_or(0),
        translated_texts: row
            .try_get::<Option<i64>, _>("translated_texts")?
            .unwrap_or(0),
        total_sections: row
            .try_get::<Option<i64>, _>("total_sections")?
            .unwrap_or(0),
    })
}

fn text_entry_from_row(row: sqlx::sqlite::SqliteRow) -> Result<TextEntrySummary> {
    let byte_offset: i64 = row.try_get("byte_offset")?;
    Ok(TextEntrySummary {
        id: row.try_get("id")?,
        script_id: row.try_get("script_id")?,
        source: row
            .try_get::<Option<String>, _>("source")?
            .unwrap_or_default(),
        byte_offset,
        byte_offset_hex: format!("0x{byte_offset:05X}"),
        section_id: row.try_get::<Option<i64>, _>("section_id")?.unwrap_or(0),
        section_order: row.try_get::<Option<i64>, _>("section_order")?.unwrap_or(0),
        original_text: row
            .try_get::<Option<String>, _>("original_text")?
            .unwrap_or_default(),
        translated_text: row
            .try_get::<Option<String>, _>("translated_text")?
            .unwrap_or_default(),
        is_translated: row.try_get::<i64, _>("is_translated")? != 0,
        needs_shift: row.try_get::<i64, _>("needs_shift")? != 0,
        fit_status: row
            .try_get::<Option<String>, _>("fit_status")?
            .unwrap_or_else(|| "unchecked".to_owned()),
        segment_capacity: row
            .try_get::<Option<i64>, _>("segment_capacity")?
            .unwrap_or(0),
    })
}

async fn create_schema(pool: &SqlitePool) -> Result<()> {
    let statements = [
        r#"
        CREATE TABLE IF NOT EXISTS scripts (
            id INTEGER PRIMARY KEY,
            source VARCHAR(10) DEFAULT 'SCRIPT',
            script_type VARCHAR(30) DEFAULT '',
            variant VARCHAR(1) DEFAULT '',
            rebuild_mode VARCHAR(30) DEFAULT '',
            offset_in_bin INTEGER DEFAULT 0,
            size_in_bin INTEGER DEFAULT 0,
            slot_capacity INTEGER DEFAULT 0,
            is_supported BOOLEAN DEFAULT 0,
            total_texts INTEGER DEFAULT 0,
            translated_texts INTEGER DEFAULT 0,
            total_sections INTEGER DEFAULT 0
        )
        "#,
        r#"
        CREATE TABLE IF NOT EXISTS text_entries (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            script_id INTEGER NOT NULL,
            source VARCHAR(10) DEFAULT 'SCRIPT',
            byte_offset INTEGER NOT NULL,
            section_id INTEGER DEFAULT 0,
            section_order INTEGER DEFAULT 0,
            original_text TEXT NOT NULL,
            translated_text TEXT DEFAULT '',
            original_bytes INTEGER DEFAULT 0,
            segment_start INTEGER DEFAULT 0,
            segment_capacity INTEGER DEFAULT 0,
            is_translated BOOLEAN DEFAULT 0,
            needs_shift BOOLEAN DEFAULT 0,
            fit_status VARCHAR(20) DEFAULT 'unchecked',
            locked_by VARCHAR(50) DEFAULT '',
            locked_at DATETIME DEFAULT NULL,
            updated_at DATETIME DEFAULT NULL,
            FOREIGN KEY(script_id) REFERENCES scripts(id)
        )
        "#,
        "CREATE INDEX IF NOT EXISTS ix_script_offset ON text_entries (script_id, byte_offset)",
        "CREATE INDEX IF NOT EXISTS ix_script_section ON text_entries (script_id, section_id, section_order)",
        "CREATE INDEX IF NOT EXISTS ix_translated ON text_entries (is_translated)",
        r#"
        CREATE TABLE IF NOT EXISTS settings (
            key VARCHAR(50) PRIMARY KEY,
            value TEXT DEFAULT ''
        )
        "#,
        r#"
        CREATE TABLE IF NOT EXISTS build_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            started_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            finished_at DATETIME DEFAULT NULL,
            status VARCHAR(20) DEFAULT 'running',
            build_type VARCHAR(20) DEFAULT 'full',
            script_id INTEGER DEFAULT NULL,
            iso_path VARCHAR(500) DEFAULT '',
            error_log TEXT DEFAULT '',
            step VARCHAR(200) DEFAULT '',
            progress_pct INTEGER DEFAULT 0
        )
        "#,
        r#"
        CREATE VIRTUAL TABLE IF NOT EXISTS text_entries_fts USING fts5(
            original_text, translated_text,
            content='text_entries',
            content_rowid='id'
        )
        "#,
        r#"
        CREATE TRIGGER IF NOT EXISTS text_entries_ai AFTER INSERT ON text_entries BEGIN
            INSERT INTO text_entries_fts(rowid, original_text, translated_text)
            VALUES (new.id, new.original_text, new.translated_text);
        END
        "#,
        r#"
        CREATE TRIGGER IF NOT EXISTS text_entries_ad AFTER DELETE ON text_entries BEGIN
            INSERT INTO text_entries_fts(text_entries_fts, rowid, original_text, translated_text)
            VALUES ('delete', old.id, old.original_text, old.translated_text);
        END
        "#,
        r#"
        CREATE TRIGGER IF NOT EXISTS text_entries_au AFTER UPDATE ON text_entries BEGIN
            INSERT INTO text_entries_fts(text_entries_fts, rowid, original_text, translated_text)
            VALUES ('delete', old.id, old.original_text, old.translated_text);
            INSERT INTO text_entries_fts(rowid, original_text, translated_text)
            VALUES (new.id, new.original_text, new.translated_text);
        END
        "#,
    ];

    for statement in statements {
        sqlx::query(statement).execute(pool).await?;
    }
    ensure_column(pool, "scripts", "rebuild_mode", "VARCHAR(30) DEFAULT ''").await?;
    Ok(())
}

async fn ensure_column(
    pool: &SqlitePool,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    let pragma = format!("PRAGMA table_info({table})");
    let rows = sqlx::query(&pragma).fetch_all(pool).await?;
    let exists = rows.iter().any(|row| {
        row.try_get::<String, _>("name")
            .is_ok_and(|name| name == column)
    });
    if !exists {
        let alter = format!("ALTER TABLE {table} ADD COLUMN {column} {definition}");
        sqlx::query(&alter).execute(pool).await?;
    }
    Ok(())
}

async fn seed_default_settings(pool: &SqlitePool) -> Result<()> {
    set_default_setting(pool, "ui_lang", "es").await?;
    set_default_setting(pool, "target_lang", "es").await?;
    set_default_setting(pool, "default_build_type", "full").await?;
    set_default_setting(pool, "default_workers", &2_usize).await?;
    set_default_setting(pool, "script_page_limit", &50_i64).await?;
    set_default_setting(pool, "search_result_limit", &100_i64).await?;
    set_default_setting(pool, "custom_glyph_map", &serde_json::json!({})).await?;
    Ok(())
}

async fn set_default_setting<T>(pool: &SqlitePool, key: &str, value: &T) -> Result<()>
where
    T: Serialize + ?Sized,
{
    let serialized = serde_json::to_string(value)?;
    sqlx::query("INSERT OR IGNORE INTO settings (key, value) VALUES (?, ?)")
        .bind(key)
        .bind(serialized)
        .execute(pool)
        .await?;
    Ok(())
}

async fn rebuild_fts_if_empty(pool: &SqlitePool) -> Result<()> {
    let count: i64 = sqlx::query("SELECT COUNT(*) AS count FROM text_entries_fts")
        .fetch_one(pool)
        .await?
        .try_get("count")?;
    if count == 0 {
        sqlx::query(
            r#"
            INSERT INTO text_entries_fts(rowid, original_text, translated_text)
            SELECT id, original_text, translated_text FROM text_entries
            "#,
        )
        .execute(pool)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn init_creates_default_settings() {
        let pool = connect("sqlite::memory:").await.unwrap();
        init_db(&pool).await.unwrap();

        let ui_lang: String = get_setting(&pool, "ui_lang", "missing".to_owned())
            .await
            .unwrap();
        let target_lang: String = get_setting(&pool, "target_lang", "missing".to_owned())
            .await
            .unwrap();
        let custom: serde_json::Value = get_setting(&pool, "custom_glyph_map", json!(null))
            .await
            .unwrap();

        assert_eq!(ui_lang, "es");
        assert_eq!(target_lang, "es");
        assert_eq!(custom, json!({}));
    }

    #[tokio::test]
    async fn failed_reimport_rolls_back_deleted_translation_data() {
        let pool = connect("sqlite::memory:").await.unwrap();
        init_db(&pool).await.unwrap();
        sqlx::query("INSERT INTO scripts (id, source) VALUES (7, 'SCRIPT')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO text_entries (script_id, source, byte_offset, original_text, translated_text, is_translated) \
             VALUES (7, 'SCRIPT', 12, 'original', 'traducción guardada', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TRIGGER reject_script_delete BEFORE DELETE ON scripts \
             BEGIN SELECT RAISE(ABORT, 'simulated import failure'); END",
        )
        .execute(&pool)
        .await
        .unwrap();

        let temp = tempfile::tempdir().unwrap();
        let scripts = temp.path().join("scripts");
        std::fs::create_dir(&scripts).unwrap();
        let result = import_extracted_texts(&pool, &scripts, temp.path().join("missing-elf")).await;

        assert!(result.is_err());
        let saved: (i64, String) = sqlx::query_as(
            "SELECT script_id, translated_text FROM text_entries WHERE byte_offset = 12",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(saved, (7, "traducción guardada".to_owned()));
        let script_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM scripts WHERE id = 7")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(script_count, 1);
    }

    #[tokio::test]
    async fn settings_roundtrip_json_values() {
        let pool = connect("sqlite::memory:").await.unwrap();
        init_db(&pool).await.unwrap();

        set_setting(&pool, "test_json", &json!({ "a": 1, "b": [2] }))
            .await
            .unwrap();
        let value: serde_json::Value = get_setting(&pool, "test_json", json!({})).await.unwrap();

        assert_eq!(value, json!({ "a": 1, "b": [2] }));
    }

    #[tokio::test]
    async fn invalid_setting_json_returns_default() {
        let pool = connect("sqlite::memory:").await.unwrap();
        init_db(&pool).await.unwrap();

        sqlx::query("INSERT OR REPLACE INTO settings (key, value) VALUES ('broken', '{')")
            .execute(&pool)
            .await
            .unwrap();

        let value: serde_json::Value = get_setting(&pool, "broken", json!({ "fallback": true }))
            .await
            .unwrap();
        assert_eq!(value, json!({ "fallback": true }));
    }

    #[tokio::test]
    async fn fts_triggers_track_text_entries() {
        let pool = connect("sqlite::memory:").await.unwrap();
        init_db(&pool).await.unwrap();

        sqlx::query("INSERT INTO scripts (id) VALUES (1)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO text_entries (script_id, byte_offset, original_text, translated_text) \
             VALUES (1, 16, 'original', 'traducido')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let count: i64 = sqlx::query(
            "SELECT COUNT(*) AS count FROM text_entries_fts WHERE text_entries_fts MATCH 'traducido'",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
        .try_get("count")
        .unwrap();

        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn deleting_elf_entry_keeps_counts_and_fts_consistent() {
        let pool = connect("sqlite::memory:").await.unwrap();
        init_db(&pool).await.unwrap();
        sqlx::query("INSERT INTO scripts (id, source, total_texts, translated_texts) VALUES (-1, 'ELF', 2, 2), (1, 'SCRIPT', 1, 1)")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO text_entries (id, script_id, source, byte_offset, original_text, translated_text, is_translated) VALUES (10, -1, 'ELF', 16, 'falso', 'Cargando', 1), (11, -1, 'ELF', 32, 'real', 'Menú', 1), (12, 1, 'SCRIPT', 16, 'dialogo', 'texto', 1)")
            .execute(&pool).await.unwrap();

        assert!(!delete_elf_text_entry(&pool, 12).await.unwrap());
        assert!(delete_elf_text_entry(&pool, 10).await.unwrap());
        assert!(!delete_elf_text_entry(&pool, 10).await.unwrap());
        let counts: (i64, i64) =
            sqlx::query_as("SELECT total_texts, translated_texts FROM scripts WHERE id = -1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(counts, (1, 1));
        let fts_hits: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM text_entries_fts WHERE text_entries_fts MATCH 'Cargando'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(fts_hits, 0);
        assert!(get_text_entry(&pool, 11).await.unwrap().is_some());
        assert!(get_text_entry(&pool, 12).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn lists_scripts_in_id_order() {
        let pool = connect("sqlite::memory:").await.unwrap();
        init_db(&pool).await.unwrap();

        sqlx::query(
            "INSERT INTO scripts (id, source, script_type, variant, is_supported, total_texts, translated_texts, total_sections) \
             VALUES (2, 'SCRIPT', 'B', 'x', 1, 10, 4, 3), \
                    (1, 'ELF', 'A', '', 0, 2, 1, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        for offset in 0..4 {
            sqlx::query(
                "INSERT INTO text_entries (script_id, byte_offset, original_text, translated_text, is_translated) \
                 VALUES (2, ?, ?, ?, 1)",
            )
            .bind(offset)
            .bind(format!("original-{offset}"))
            .bind(format!("translated-{offset}"))
            .execute(&pool)
            .await
            .unwrap();
        }

        let scripts = list_scripts(&pool).await.unwrap();
        assert_eq!(scripts.len(), 2);
        assert_eq!(scripts[0].id, 1);
        assert_eq!(scripts[1].id, 2);
        assert!(scripts[1].is_supported);
        assert_eq!(scripts[1].translated_texts, 4);
    }

    #[tokio::test]
    async fn updating_text_refreshes_script_count() {
        let pool = connect("sqlite::memory:").await.unwrap();
        init_db(&pool).await.unwrap();

        sqlx::query("INSERT INTO scripts (id, total_texts, translated_texts) VALUES (1, 1, 0)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO text_entries (script_id, source, byte_offset, original_text, translated_text, is_translated) \
             VALUES (1, 'SCRIPT', 16, 'original', '', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let updated = update_text_entry_translation(&pool, 1, "traducido")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated.translated_text, "traducido");
        assert!(updated.is_translated);

        let script = get_script(&pool, 1).await.unwrap().unwrap();
        assert_eq!(script.translated_texts, 1);
    }

    #[tokio::test]
    async fn editing_sequence_keeps_flags_and_counts_consistent() {
        let pool = connect("sqlite::memory:").await.unwrap();
        init_db(&pool).await.unwrap();

        sqlx::query("INSERT INTO scripts (id, total_texts, translated_texts) VALUES (1, 1, 0)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO text_entries (script_id, source, byte_offset, original_text, translated_text, is_translated, segment_capacity) \
             VALUES (1, 'SCRIPT', 16, 'original', '', 0, 20)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let cases = [
            ("   \n  ", "", false, "unchecked", false, 0),
            ("hola", "hola", true, "tight", false, 1),
            (
                "este texto es demasiado largo",
                "este texto es demasiado largo",
                true,
                "needs_shift",
                true,
                1,
            ),
            ("123456789\r\n  \n", "123456789", true, "tight", false, 1),
            ("", "", false, "unchecked", false, 0),
        ];

        for (input, stored, is_translated, fit_status, needs_shift, translated_count) in cases {
            let updated = update_text_entry_translation(&pool, 1, input)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(updated.translated_text, stored);
            assert_eq!(updated.is_translated, is_translated);
            assert_eq!(updated.fit_status, fit_status);
            assert_eq!(updated.needs_shift, needs_shift);

            let script = get_script(&pool, 1).await.unwrap().unwrap();
            assert_eq!(script.translated_texts, translated_count);
        }
    }

    #[tokio::test]
    async fn normalized_save_recomputes_fit_from_stored_text() {
        let pool = connect("sqlite::memory:").await.unwrap();
        init_db(&pool).await.unwrap();

        sqlx::query("INSERT INTO scripts (id, total_texts, translated_texts) VALUES (1, 1, 0)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO text_entries (script_id, source, byte_offset, original_text, translated_text, is_translated, segment_capacity) \
             VALUES (1, 'SCRIPT', 16, 'original', '', 0, 20)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let updated = update_text_entry_translation(&pool, 1, "123456789\n\n")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(updated.translated_text, "123456789");
        assert_eq!(updated.fit_status, "tight");
        assert!(!updated.needs_shift);
    }

    #[tokio::test]
    async fn search_uses_fts_table() {
        let pool = connect("sqlite::memory:").await.unwrap();
        init_db(&pool).await.unwrap();

        sqlx::query("INSERT INTO scripts (id) VALUES (1)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            r#"
            WITH RECURSIVE seq(n) AS (
                VALUES(1)
                UNION ALL
                SELECT n + 1 FROM seq WHERE n < 50
            )
            INSERT INTO text_entries (script_id, section_id, section_order, byte_offset, original_text, translated_text, is_translated)
            SELECT 1, 0, n, n, 'filler ' || n, '', 0 FROM seq
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO text_entries (script_id, section_id, section_order, byte_offset, original_text, translated_text, is_translated) \
             VALUES (1, 0, 51, 51, 'strawberry original', 'panic traducido', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let results = search_text_entries(&pool, "panic", 20).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].script_id, 1);
        assert_eq!(results[0].page, 2);
        assert_eq!(results[0].translated_text, "panic traducido");
    }

    #[tokio::test]
    async fn empty_search_returns_no_results() {
        let pool = connect("sqlite::memory:").await.unwrap();
        init_db(&pool).await.unwrap();

        sqlx::query("INSERT INTO scripts (id) VALUES (1)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO text_entries (script_id, byte_offset, original_text, translated_text) \
             VALUES (1, 16, 'strawberry original', 'panic traducido')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let results = search_text_entries(&pool, "   ", 20).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn script_detail_filters_by_translation_status() {
        let pool = connect("sqlite::memory:").await.unwrap();
        init_db(&pool).await.unwrap();

        sqlx::query("INSERT INTO scripts (id, total_texts, translated_texts) VALUES (1, 4, 3)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO text_entries (script_id, source, byte_offset, section_id, section_order, original_text, translated_text, is_translated, needs_shift, fit_status) \
             VALUES (1, 'SCRIPT', 16, 0, 0, 'clean original', 'clean translated', 1, 0, 'ok'), \
                    (1, 'SCRIPT', 18, 0, 1, 'warning original', 'warning translated', 1, 0, 'tight'), \
                    (1, 'SCRIPT', 20, 0, 2, 'shift original', 'shift translated', 1, 1, 'needs_shift'), \
                    (1, 'SCRIPT', 22, 0, 3, 'pending original', '', 0, 0, 'unchecked')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let translated = get_script_detail_filtered(&pool, 1, 1, 50, "translated")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(translated.total, 1);
        assert_eq!(translated.texts[0].original_text, "clean original");

        let warning = get_script_detail_filtered(&pool, 1, 1, 50, "warning")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(warning.total, 1);
        assert_eq!(warning.texts[0].fit_status, "tight");

        let needs_shift = get_script_detail_filtered(&pool, 1, 1, 50, "needs_shift")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(needs_shift.total, 1);
        assert!(needs_shift.texts[0].needs_shift);

        let untranslated = get_script_detail_filtered(&pool, 1, 1, 50, "untranslated")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(untranslated.total, 1);
        assert!(!untranslated.texts[0].is_translated);
    }

    #[tokio::test]
    async fn search_filters_by_script_and_status() {
        let pool = connect("sqlite::memory:").await.unwrap();
        init_db(&pool).await.unwrap();

        sqlx::query("INSERT INTO scripts (id) VALUES (1), (2)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO text_entries (script_id, source, byte_offset, section_id, section_order, original_text, translated_text, is_translated, needs_shift, fit_status) \
             VALUES (1, 'SCRIPT', 16, 0, 0, 'panic clean', 'panic limpio', 1, 0, 'ok'), \
                    (1, 'SCRIPT', 18, 0, 1, 'panic warning', 'panic justo', 1, 0, 'tight'), \
                    (2, 'SCRIPT', 20, 0, 0, 'panic clean other', 'panic otro', 1, 0, 'ok')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let all = search_text_entries_filtered(&pool, "panic", 20, None, "all")
            .await
            .unwrap();
        assert_eq!(all.len(), 3);

        let script_one = search_text_entries_filtered(&pool, "panic", 20, Some(1), "all")
            .await
            .unwrap();
        assert_eq!(script_one.len(), 2);
        assert!(script_one.iter().all(|result| result.script_id == 1));

        let warning = search_text_entries_filtered(&pool, "panic", 20, Some(1), "warning")
            .await
            .unwrap();
        assert_eq!(warning.len(), 1);
        assert_eq!(warning[0].original_text, "panic warning");

        let status_only = search_text_entries_filtered(&pool, "", 20, Some(1), "warning")
            .await
            .unwrap();
        assert_eq!(status_only.len(), 1);
        assert_eq!(status_only[0].original_text, "panic warning");
    }

    #[test]
    fn normalizes_line_endings_and_preserves_paragraphs() {
        assert_eq!(
            normalize_translation_linebreaks("hola\r\n  \nesta es la tercera fila"),
            "hola\n\nesta es la tercera fila"
        );
        assert_eq!(
            normalize_translation_linebreaks("uno\r\n\r\ndos\n \n tres  "),
            "uno\n\ndos\n\n tres"
        );
        assert_eq!(normalize_translation_linebreaks("\n\nuno\n\n"), "uno");
        assert_eq!(normalize_translation_linebreaks("uno<lb/>dos"), "uno\ndos");
    }

    #[tokio::test]
    async fn wraps_long_script_translations_when_saving() {
        let pool = connect("sqlite::memory:").await.unwrap();
        init_db(&pool).await.unwrap();
        sqlx::query("INSERT INTO scripts (id, total_texts) VALUES (1, 1)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO text_entries (script_id, source, byte_offset, original_text, translated_text, segment_capacity) \
             VALUES (1, 'SCRIPT', 16, '日本語', '', 1000)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let input = "Esta traducción contiene suficientes palabras para superar el ancho visual de una línea del juego";
        let saved = update_text_entry_translation(&pool, 1, input)
            .await
            .unwrap()
            .unwrap();

        assert!(saved.translated_text.contains('\n'));
        assert_eq!(saved.translated_text.replace('\n', " "), input);
    }

    #[tokio::test]
    async fn repairs_stored_blank_translation_lines() {
        let pool = connect("sqlite::memory:").await.unwrap();
        init_db(&pool).await.unwrap();

        sqlx::query("INSERT INTO scripts (id) VALUES (1)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO text_entries (script_id, source, byte_offset, original_text, translated_text, is_translated, segment_capacity) \
             VALUES (1, 'SCRIPT', 16, 'original', 'hola\r\n  \ntercera fila', 1, 120)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let updated = normalize_stored_translation_linebreaks(&pool)
            .await
            .unwrap();
        assert_eq!(updated, 1);

        let text = get_text_entry(&pool, 1).await.unwrap().unwrap();
        assert_eq!(text.translated_text, "hola\n\ntercera fila");
        assert!(text.is_translated);
    }

    #[tokio::test]
    async fn translation_stats_counts_entries() {
        let pool = connect("sqlite::memory:").await.unwrap();
        init_db(&pool).await.unwrap();

        sqlx::query("INSERT INTO scripts (id) VALUES (1)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO text_entries (script_id, byte_offset, original_text, translated_text, is_translated, needs_shift) \
             VALUES (1, 16, 'a', 'b', 1, 0), (1, 18, 'c', '', 0, 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let stats = translation_stats(&pool).await.unwrap();
        assert_eq!(stats.scripts, 1);
        assert_eq!(stats.text_entries, 2);
        assert_eq!(stats.translated_entries, 1);
        assert_eq!(stats.needs_shift_entries, 1);
    }

    #[tokio::test]
    async fn exports_translations_for_build() {
        let pool = connect("sqlite::memory:").await.unwrap();
        init_db(&pool).await.unwrap();

        sqlx::query("INSERT INTO scripts (id) VALUES (1)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO text_entries (script_id, source, byte_offset, section_id, section_order, original_text, translated_text, is_translated) \
             VALUES (1, 'SCRIPT', 16, 0, 0, 'a', 'b\r\n  \nc', 1), (1, 'SCRIPT', 18, 0, 1, 'd', '', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let rows = exported_translations(&pool, true).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].file_id, "1");
        assert_eq!(rows[0].offset, "0x00010");
        assert_eq!(rows[0].translated_text, "b\n\nc");

        let temp = tempfile::tempdir().unwrap();
        let csv_path = temp.path().join("dialogo.csv");
        let count = export_translations_csv(&pool, &csv_path, true)
            .await
            .unwrap();
        assert_eq!(count, 1);

        let mut reader = csv::Reader::from_path(csv_path).unwrap();
        let records = reader
            .records()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].get(4), Some("b\n\nc"));
    }

    #[tokio::test]
    async fn imports_translations_from_compatible_database() {
        let pool = connect("sqlite::memory:").await.unwrap();
        init_db(&pool).await.unwrap();

        sqlx::query("INSERT INTO scripts (id, total_texts, translated_texts) VALUES (1, 2, 0)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO text_entries (script_id, source, byte_offset, original_text, translated_text, is_translated, segment_capacity) \
             VALUES (1, 'SCRIPT', 16, 'original', '', 0, 40), \
                    (1, 'SCRIPT', 18, 'otro', '', 0, 40)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("translated.db");
        let source = connect_path(&source_path).await.unwrap();
        init_db(&source).await.unwrap();
        sqlx::query("INSERT INTO scripts (id, total_texts, translated_texts) VALUES (1, 2, 1)")
            .execute(&source)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO text_entries (script_id, source, byte_offset, original_text, translated_text, is_translated, segment_capacity) \
             VALUES (1, 'SCRIPT', 16, 'original', 'traducido', 1, 40), \
                    (1, 'SCRIPT', 20, 'no existe', 'ignorado', 1, 40)",
        )
        .execute(&source)
        .await
        .unwrap();
        source.close().await;

        let report = import_translations_from_db(&pool, &source_path)
            .await
            .unwrap();
        assert_eq!(report.source_rows, 2);
        assert_eq!(report.imported, 1);
        assert_eq!(report.skipped_missing, 1);

        let text = get_text_entry(&pool, 1).await.unwrap().unwrap();
        assert_eq!(text.translated_text, "traducido");
        assert!(text.is_translated);
        let script = get_script(&pool, 1).await.unwrap().unwrap();
        assert_eq!(script.translated_texts, 1);
    }

    #[tokio::test]
    async fn records_build_steps() {
        let pool = connect("sqlite::memory:").await.unwrap();
        init_db(&pool).await.unwrap();

        let id = record_build_step(
            &pool,
            "success",
            "full",
            "build iso",
            80,
            "work/Strawberry_translated.iso",
            "",
        )
        .await
        .unwrap();

        let builds = recent_builds(&pool, 10).await.unwrap();
        assert_eq!(builds.len(), 1);
        assert_eq!(builds[0].id, id);
        assert_eq!(builds[0].status, "success");
        assert_eq!(builds[0].step, "build iso");
        assert_eq!(builds[0].progress_pct, 80);
    }
}
