"""
glyph_map.py — Single source of truth for the Spanish → Cyrillic mapping.

Imported by: script_rebuilder.py, apply_translation.py, webapp/services/fit_checker.py
"""

SPANISH_TO_GLYPH = {
    'á': '\u0413', 
    'é': '\u0414', 
    'í': '\u0415', 
    'ó': '\u0416', 
    'ú': '\u0417', 
    'ñ': '\u0418', 
    'Ñ': '\u0419', 
    '¡': '\u041A', 
    '¿': '\u041B', 
    'Á': '\u0413', 
    'É': '\u0414', 
    'Í': '\u0415', 
    'Ó': '\u0416', 
    'Ú': '\u0417', 
    'Ü': '\u0417', 
    'ü': '\u0417', 
}


def game_string(text: str) -> str:
    """Converts readable text to available game glyphs."""
    return ''.join(SPANISH_TO_GLYPH.get(ch, ch) for ch in text)


def encode_game_utf16(text: str) -> bytes:
    """Encodes text to UTF-16LE with Cyrillic mapping."""
    return game_string(text).encode('utf-16-le')


def encode_game_sjis(text: str) -> bytes:
    """Encodes text to Shift-JIS with Cyrillic mapping."""
    return game_string(text).encode('shift-jis')
