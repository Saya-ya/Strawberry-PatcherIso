use crate::glyph_map::{encode_game_sjis, encode_game_utf16, GlyphMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextSource {
    Script,
    Elf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FitStatus {
    Unchecked,
    Ok,
    Tight,
    NeedsShift,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FitResult {
    pub status: FitStatus,
    pub used_bytes: usize,
    pub capacity: usize,
    pub remaining: isize,
}

pub fn check_fit(
    translated_text: &str,
    source: TextSource,
    capacity: usize,
    glyph_map: Option<&GlyphMap>,
) -> FitResult {
    if translated_text.trim().is_empty() {
        return FitResult {
            status: FitStatus::Unchecked,
            used_bytes: 0,
            capacity,
            remaining: capacity as isize,
        };
    }

    let encoded = match source {
        TextSource::Script => encode_game_utf16(translated_text, glyph_map),
        TextSource::Elf => encode_game_sjis(translated_text, glyph_map),
    };
    let terminator = if source == TextSource::Script { 2 } else { 0 };
    let used_bytes = encoded.len() + terminator;
    let remaining = capacity as isize - used_bytes as isize;
    let status = if remaining >= 20 {
        FitStatus::Ok
    } else if remaining >= 0 {
        FitStatus::Tight
    } else {
        FitStatus::NeedsShift
    };

    FitResult {
        status,
        used_bytes,
        capacity,
        remaining,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glyph_map::{english_glyph_map, spanish_glyph_map};

    #[test]
    fn empty_text_is_unchecked() {
        let result = check_fit("", TextSource::Script, 100, None);
        assert_eq!(result.status, FitStatus::Unchecked);
        assert_eq!(result.remaining, 100);
    }

    #[test]
    fn script_counts_utf16_and_terminator() {
        let map = spanish_glyph_map();
        let result = check_fit("Hola", TextSource::Script, 100, Some(&map));
        assert_eq!(result.status, FitStatus::Ok);
        assert_eq!(result.used_bytes, 10);
    }

    #[test]
    fn detects_tight_and_needs_shift() {
        let map = english_glyph_map();
        assert_eq!(
            check_fit("x".repeat(40).as_str(), TextSource::Script, 82, Some(&map)).status,
            FitStatus::Tight
        );
        assert_eq!(
            check_fit("x".repeat(41).as_str(), TextSource::Script, 82, Some(&map)).status,
            FitStatus::NeedsShift
        );
    }
}
