pub mod datafat;
pub mod dialogue;
pub mod extract;
pub mod fit_checker;
pub mod fs_ops;
pub mod glyph_map;
pub mod iso;
pub mod lz77;
pub mod patch_elf;
pub mod patch_scripts;
pub mod script_rebuilder;
pub mod text_layout;
pub mod texture_inventory;

pub use datafat::{
    find_row, parse_entries, read_entries, size_field_write_offset, slot_capacity, FatEntry,
};
pub use dialogue::{
    analyze_script_dec, classify_script_rebuild_mode, extract_elf_strings, AnalyzedScript,
    ImportedText,
};
pub use extract::{
    extract_lz77_scripts, extract_lz77_scripts_from_file, extract_lz77_scripts_to_dir,
    write_extracted_scripts, ExtractedScript,
};
pub use fs_ops::copy_file_creating_parent;

pub use fit_checker::{check_fit, FitResult, FitStatus, TextSource};
pub use glyph_map::{
    encode_game_sjis, encode_game_utf16, english_glyph_map, game_string, invert_glyph_map,
    normalize_glyph_map, spanish_glyph_map, validate_glyph_map, GlyphMap,
};
pub use iso::{
    build_iso_with_patched_data, find_signature_offset, inject_elf_into_iso, DATA_BIN_SIGNATURE,
};
pub use lz77::{compress_lz77, decompress_lz77};
pub use patch_elf::{patch_translated_elf, PatchElfReport};
pub use patch_scripts::{patch_translated_scripts, PatchScriptsReport};
pub use script_rebuilder::{
    find_segment_containing, rebuild_local_slack, rebuild_shift_suffix, RebuildReport,
    SegmentReport, TextSegment, TranslationRow,
};
pub use text_layout::{
    glyph_width, is_vertical_script, max_visual_line_width, visual_width, wrap_script_translation,
    wrap_script_translation_for, SCRIPT_LINE_WIDTH, VERTICAL_COLUMN_CHARACTERS,
};
pub use texture_inventory::{
    clean_all_pngs_in_dir, clean_png_transparency, inject_patched_texture_streams,
    normalize_transparent_pixels, patch_textures_from_manifest,
    patch_textures_from_manifest_with_glyph_map, write_texture_inventory,
    write_texture_inventory_with_progress, GlyphMetricChange, TextureInjectReport,
    TexturePatchReport, TextureRecord,
};
