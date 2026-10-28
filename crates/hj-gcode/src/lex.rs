//! Splitting a G-code line into words.

/// A letter and its value, e.g. `G1` or `X-1.5`. The letter is upper case.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Word {
    pub letter: char,
    pub value: f64,
}

#[derive(Debug, PartialEq)]
pub enum Line {
    Words(Vec<Word>),
    /// Not G-code: a console command such as `$H` or `play`, which the firmware also accepts in a
    /// program.
    Command,
}

/// Parse one line. Comments in parentheses and after `;` are skipped, as are `%` markers.
pub fn line(text: &str) -> Result<Line, String> {
    let trimmed = text.trim_start();
    let mut chars = trimmed.chars();
    if trimmed.starts_with('$')
        || matches!((chars.next(), chars.next()), (Some(a), Some(b)) if a.is_ascii_alphabetic() && b.is_ascii_alphabetic())
    {
        return Ok(Line::Command);
    }

    let mut words = vec![];
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        match c {
            ' ' | '\t' | '\r' | '\n' | '%' => i += 1,
            ';' => break,
            '(' => match text[i..].find(')') {
                Some(end) => i += end + 1,
                None => return Err("unclosed comment".into()),
            },
            '#' => return Err("parameters (#) aren't supported".into()),
            c if c.is_ascii_alphabetic() => {
                let letter = c.to_ascii_uppercase();
                i += 1;
                while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                    i += 1;
                }
                let start = i;
                if i < bytes.len() && (bytes[i] == b'-' || bytes[i] == b'+') {
                    i += 1;
                }
                let mut digits = 0;
                while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                    digits += usize::from(bytes[i].is_ascii_digit());
                    i += 1;
                }
                let value = text[start..i]
                    .parse::<f64>()
                    .ok()
                    .filter(|_| digits > 0)
                    .ok_or_else(|| format!("{letter} needs a number"))?;
                words.push(Word { letter, value });
            }
            c => return Err(format!("unexpected '{c}'")),
        }
    }
    Ok(Line::Words(words))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<(char, f64)> {
        match line(s).unwrap() {
            Line::Words(w) => w.iter().map(|w| (w.letter, w.value)).collect(),
            Line::Command => panic!("command"),
        }
    }

    #[test]
    fn splits_words_and_skips_comments() {
        assert_eq!(
            words("N10 g1x-1.5 Y.5 (move) Z +2 ; done"),
            vec![('N', 10.0), ('G', 1.0), ('X', -1.5), ('Y', 0.5), ('Z', 2.0)]
        );
        assert_eq!(words("%"), vec![]);
        assert_eq!(words("(only a comment)"), vec![]);
        assert_eq!(words("G38.2 Z-5"), vec![('G', 38.2), ('Z', -5.0)]);
    }

    #[test]
    fn recognizes_console_commands() {
        assert_eq!(line("$H").unwrap(), Line::Command);
        assert_eq!(line("  play /sd/x.nc").unwrap(), Line::Command);
        assert!(matches!(line("M6T1").unwrap(), Line::Words(_)));
    }

    #[test]
    fn reports_errors() {
        assert_eq!(line("G1 X").unwrap_err(), "X needs a number");
        assert_eq!(line("G1 X- Y1").unwrap_err(), "X needs a number");
        assert_eq!(line("G1 (oops").unwrap_err(), "unclosed comment");
        assert!(line("G1 X#1").is_err());
        assert!(line("G1 X1*55").is_err());
    }
}
