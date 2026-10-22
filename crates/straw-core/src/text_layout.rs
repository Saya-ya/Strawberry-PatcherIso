pub const SCRIPT_LINE_WIDTH: usize = 357;
pub const VERTICAL_COLUMN_CHARACTERS: usize = 17;

const VERTICAL_SCRIPT_IDS: &[i64] = &[8024, 8025, 8026, 8044, 8045, 8046, 8064, 8065, 8066];

pub fn glyph_width(ch: char) -> usize {
    match ch {
        ' ' => 7,
        'I' | 'l' | 'i' | 'í' | 'Í' => 5,
        'j' | '¡' => 6,
        'f' | 't' | '.' | ',' | ':' | ';' | '\'' => 8,
        'r' => 9,
        'v' | 'x' | 'y' | 'z' | 'ú' | 'ü' | 'Ú' | 'Ü' => 12,
        'c' | 's' | 'ñ' | '¿' => 13,
        'a' | 'b' | 'd' | 'e' | 'g' | 'h' | 'k' | 'n' | 'o' | 'p' | 'q' | 'u' | 'á' | 'é' | 'ó' => {
            14
        }
        'E' | 'F' | 'J' | 'L' | 'S' | 'T' | 'X' | 'Ñ' | 'Á' | 'É' | 'Ó' => 15,
        'B' | 'K' | 'N' | 'U' | 'V' | 'Y' => 16,
        'C' | 'D' | 'G' | 'H' | 'R' => 17,
        'A' | 'O' | 'P' | 'Q' | 'W' | 'm' => 18,
        'M' => 19,
        '0'..='9' | '-' | '–' | '—' | '(' | ')' | '[' | ']' | '«' | '»' => 14,
        '\n' | '\r' => 0,
        _ if ch.is_ascii() => 14,
        _ => 21,
    }
}

pub fn visual_width(text: &str) -> usize {
    text.chars().map(glyph_width).sum()
}

pub fn max_visual_line_width(text: &str) -> usize {
    text.lines().map(visual_width).max().unwrap_or(0)
}

pub fn wrap_script_translation(text: &str) -> String {
    text.split('\n')
        .map(wrap_line)
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn is_vertical_script(script_id: i64) -> bool {
    VERTICAL_SCRIPT_IDS.contains(&script_id)
}

pub fn wrap_script_translation_for(script_id: i64, text: &str) -> String {
    if is_vertical_script(script_id) {
        text.split('\n')
            .map(wrap_vertical_column)
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        wrap_script_translation(text)
    }
}

fn wrap_vertical_column(line: &str) -> String {
    if line.trim().is_empty() || line.chars().count() <= VERTICAL_COLUMN_CHARACTERS {
        return line.to_owned();
    }

    let mut wrapped = Vec::new();
    let mut current = String::new();

    for word in line.split_whitespace() {
        let separator = usize::from(!current.is_empty());
        if !current.is_empty()
            && current.chars().count() + separator + word.chars().count()
                > VERTICAL_COLUMN_CHARACTERS
        {
            wrapped.push(current);
            current = word.to_owned();
        } else {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }
    }

    if !current.is_empty() {
        wrapped.push(current);
    }

    wrapped.join("\n")
}

fn wrap_line(line: &str) -> String {
    if line.trim().is_empty() || visual_width(line) <= SCRIPT_LINE_WIDTH {
        return line.to_owned();
    }

    let mut wrapped = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;

    for word in line.split_whitespace() {
        let word_width = visual_width(word);
        let separator_width = if current.is_empty() {
            0
        } else {
            glyph_width(' ')
        };
        if !current.is_empty() && current_width + separator_width + word_width > SCRIPT_LINE_WIDTH {
            wrapped.push(current);
            current = word.to_owned();
            current_width = word_width;
        } else {
            if !current.is_empty() {
                current.push(' ');
                current_width += separator_width;
            }
            current.push_str(word);
            current_width += word_width;
        }
    }

    if !current.is_empty() {
        wrapped.push(current);
    }

    wrapped.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_short_lines_and_blank_paragraphs() {
        let text = "Primera línea\n\nSegunda línea";
        assert_eq!(wrap_script_translation(text), text);
    }

    #[test]
    fn wraps_at_spaces_without_changing_character_count() {
        let text = "Esta traducción contiene suficientes palabras para superar el ancho visual de una línea del juego";
        let wrapped = wrap_script_translation(text);
        assert!(wrapped.contains('\n'));
        assert!(wrapped
            .lines()
            .all(|line| visual_width(line) <= SCRIPT_LINE_WIDTH));
        assert_eq!(wrapped.chars().count(), text.chars().count());
    }

    #[test]
    fn leaves_long_unbreakable_words_intact() {
        let text = "x".repeat(40);
        assert_eq!(wrap_script_translation(&text), text);
    }

    #[test]
    fn wrapping_is_idempotent() {
        let text = "Esta traducción contiene suficientes palabras para superar el ancho visual de una línea del juego";
        let once = wrap_script_translation(text);
        assert_eq!(wrap_script_translation(&once), once);
    }

    #[test]
    fn vertical_mail_uses_the_source_column_height() {
        let text = "Hermano, Nagisa quiere contarte lo que pasó hoy";
        let wrapped = wrap_script_translation_for(8024, text);
        assert!(wrapped
            .lines()
            .all(|line| line.chars().count() <= VERTICAL_COLUMN_CHARACTERS));
        assert_eq!(wrapped.replace('\n', " "), text);
        assert_eq!(wrap_script_translation_for(8024, &wrapped), wrapped);
    }

    #[test]
    fn ordinary_scripts_keep_horizontal_wrapping() {
        let text = "Esta traducción contiene suficientes palabras para superar el ancho visual de una línea del juego";
        assert_eq!(
            wrap_script_translation_for(8023, text),
            wrap_script_translation(text)
        );
    }
}
