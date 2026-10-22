use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Result};
use encoding_rs::SHIFT_JIS;

pub type GlyphMap = BTreeMap<char, char>;

const ES_MAP: &[(char, char)] = &[
    ('á', 'Г'),
    ('é', 'Д'),
    ('í', 'Е'),
    ('ó', 'Ж'),
    ('ú', 'З'),
    ('ñ', 'И'),
    ('Ñ', 'Й'),
    ('¡', 'К'),
    ('¿', 'Л'),
    ('Á', 'Г'),
    ('É', 'Д'),
    ('Í', 'Е'),
    ('Ó', 'Ж'),
    ('Ú', 'З'),
    ('Ü', 'З'),
    ('ü', 'З'),
    ('♥', '＠'),
    ('♡', '＠'),
    ('❤', '＠'),
    ('@', '＠'),
];

const AVAILABLE_GLYPHS: &[char] = &[
    'А', 'Б', 'В', 'Г', 'Д', 'Е', 'Ж', 'З', 'И', 'Й', 'К', 'Л', 'М', 'Н', 'О', 'П', 'Р', 'С', 'Т',
    'У', 'Ф', 'Х', 'Ц', 'Ч', 'Ш', 'Щ', 'Ъ', 'Ы', 'Ь', 'Э', 'Ю', 'Я', 'а', 'б', 'в', 'г', 'д', 'е',
    'ж', 'з', 'и', 'й', 'к', 'л', 'м', 'н', 'о', 'п', 'р', 'с', 'т', 'у', 'ф', 'х', 'ц', 'ч', 'ш',
    'щ', 'ъ', 'ы', 'ь', 'э', 'ю', 'я', '＠',
];

pub fn spanish_glyph_map() -> GlyphMap {
    ES_MAP.iter().copied().collect()
}

pub fn english_glyph_map() -> GlyphMap {
    GlyphMap::new()
}

pub fn available_glyphs() -> BTreeSet<char> {
    AVAILABLE_GLYPHS.iter().copied().collect()
}

pub fn normalize_glyph_map(input: &GlyphMap) -> GlyphMap {
    let available = available_glyphs();
    let mut normalized = GlyphMap::new();

    for (&key, &value) in input {
        if available.contains(&key) && !available.contains(&value) {
            normalized.insert(value, key);
        } else {
            normalized.insert(key, value);
        }
    }

    normalized
}

pub fn validate_glyph_map(input: &GlyphMap) -> Result<()> {
    let available = available_glyphs();
    for (source, donor) in input {
        if !available.contains(donor) {
            bail!("glyph donor {donor:?} for {source:?} is not an available replacement slot");
        }
        let donor_text = donor.to_string();
        let (_, _, had_errors) = SHIFT_JIS.encode(&donor_text);
        if had_errors {
            bail!("glyph donor {donor:?} for {source:?} cannot be encoded as Shift-JIS");
        }
    }
    Ok(())
}

pub fn invert_glyph_map(input: &GlyphMap) -> GlyphMap {
    let mut inverted = GlyphMap::new();
    for (source, glyph) in normalize_glyph_map(input) {
        inverted.entry(glyph).or_insert(source);
    }
    inverted
}

pub fn game_string(text: &str, glyph_map: Option<&GlyphMap>) -> String {
    let default_map;
    let glyph_map = match glyph_map {
        Some(map) => map,
        None => {
            default_map = spanish_glyph_map();
            &default_map
        }
    };

    if glyph_map.is_empty() {
        return text.to_owned();
    }

    text.chars()
        .map(|ch| glyph_map.get(&ch).copied().unwrap_or(ch))
        .collect()
}

pub fn encode_game_utf16(text: &str, glyph_map: Option<&GlyphMap>) -> Vec<u8> {
    game_string(text, glyph_map)
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect()
}

pub fn encode_game_sjis(text: &str, glyph_map: Option<&GlyphMap>) -> Vec<u8> {
    let mapped = game_string(text, glyph_map);
    let (encoded, _, _) = SHIFT_JIS.encode(&mapped);
    encoded.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_spanish_glyphs() {
        let map = spanish_glyph_map();
        assert_eq!(map.len(), 20);
        assert_eq!(map.get(&'ñ'), Some(&'И'));
        assert_eq!(map.get(&'♥'), Some(&'＠'));
        assert_eq!(game_string("año", Some(&map)), "aИo");
    }

    #[test]
    fn english_map_is_empty() {
        let map = english_glyph_map();
        assert!(map.is_empty());
        assert_eq!(game_string("hello", Some(&map)), "hello");
    }

    #[test]
    fn custom_map_normalizes_old_ui_format() {
        let old_ui = [('Г', 'ą'), ('Д', 'ć')].into_iter().collect();
        let normalized = normalize_glyph_map(&old_ui);
        assert_eq!(normalized.get(&'ą'), Some(&'Г'));
        assert_eq!(normalized.get(&'ć'), Some(&'Д'));
        assert_eq!(invert_glyph_map(&normalized), old_ui);
    }

    #[test]
    fn validates_reserved_and_shift_jis_encodable_donors() {
        let valid = [('ã', 'Г'), ('♥', '＠')].into_iter().collect();
        validate_glyph_map(&valid).unwrap();
        let invalid = [('ã', 'A')].into_iter().collect();
        assert!(validate_glyph_map(&invalid).is_err());
    }

    #[test]
    fn encodes_utf16_little_endian() {
        let map = spanish_glyph_map();
        assert_eq!(
            encode_game_utf16("año", Some(&map)),
            "aИo"
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn heart_maps_to_fullwidth_at_for_sjis() {
        assert_eq!(game_string("Hola♥", None), "Hola＠");
        assert_eq!(
            encode_game_sjis("Hola@", None),
            encode_game_sjis("Hola＠", Some(&english_glyph_map()))
        );
    }
}
