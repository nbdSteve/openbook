//! The page-quality heuristic: does a page need transcription by a vision model?
//!
//! A page needs it when its text layer looks poor:
//! - fewer than 200 characters on a page that has images (a scan or a figure page), or
//! - more than 5% replacement or private-use characters (a broken font map), or
//! - at least 3 mathematical symbols (the text layer garbles formulas), or
//! - many isolated one-character tokens (formulas torn apart into pieces).

/// Pages with images and less text than this need transcription.
pub const MIN_TEXT_CHARS_WITH_IMAGES: usize = 200;
/// The share of replacement and private-use characters above which a page needs transcription.
pub const MAX_BAD_CHAR_SHARE: f64 = 0.05;
/// The number of math symbols from which a page needs transcription.
pub const MATH_SYMBOL_THRESHOLD: usize = 3;
/// A page needs transcription when more than this share of its tokens is a lone character...
pub const MAX_LONE_TOKEN_SHARE: f64 = 0.25;
/// ...and it has at least this many tokens.
pub const MIN_TOKENS_FOR_LONE_CHECK: usize = 20;

fn is_bad_char(c: char) -> bool {
    let code = c as u32;
    c == '\u{FFFD}'
        || (0xE000..=0xF8FF).contains(&code)
        || (0xF0000..=0xFFFFD).contains(&code)
        || (0x100000..=0x10FFFD).contains(&code)
}

/// Mathematical operators and a few Latin-1 signs. Greek letters are not here: see `count_math_symbols`.
fn is_math_operator(c: char) -> bool {
    let code = c as u32;
    (0x2200..=0x22FF).contains(&code)
        || (0x2A00..=0x2AFF).contains(&code)
        || (0x27C0..=0x27EF).contains(&code)
        || matches!(c, '±' | '×' | '÷' | '∑' | '∫' | '√' | '∞')
}

/// A Greek letter (capital or small, with or without an accent mark in one code point).
fn is_greek_letter(c: char) -> bool {
    (0x0391..=0x03C9).contains(&(c as u32)) && c != '\u{03A2}' && c != '\u{03C2}'
}

/// The ASCII signs that join terms in a formula.
fn is_ascii_operator(c: char) -> bool {
    matches!(c, '=' | '+' | '-' | '*' | '/' | '^' | '<' | '>' | '~')
}

/// A letter of a word. Combining accent marks belong to the letter before them (XeTeX writes
/// Greek accents that way).
fn is_wordish(c: char) -> bool {
    c.is_alphabetic() || (0x0300..=0x036F).contains(&(c as u32))
}

/// A character that wraps or separates symbols in a formula: `(α)`, `α, β`.
fn is_separator(c: char) -> bool {
    matches!(c, ',' | ';' | ':' | '(' | ')' | '[' | ']' | '{' | '}' | '|')
}

/// The number of math symbols in the text.
///
/// Operators and the like count one each. A Greek letter counts only as a symbol, never as a part of
/// a Greek word. The text is cut into runs of letters. A run of one Greek letter is a candidate,
/// and it counts when it
/// - touches a math operator, a bracket, a comma, or a digit (`2α`, `(α)`, `α,`),
/// - has an operator as the next or the previous character that is not a space (`α + β`), or
/// - has another one-letter Greek run next to it, with only spaces, commas, or semicolons between
///   (`α, β, γ`).
///
/// A letter inside a word, and the one-letter articles of Greek prose (`Ο Σωκράτης`), do not count.
fn count_math_symbols(text: &str) -> usize {
    let chars: Vec<char> = text.chars().collect();
    // The runs of letters: [start, end) and whether the run is one Greek letter.
    let mut runs: Vec<(usize, usize, bool)> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !is_wordish(chars[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && is_wordish(chars[i]) {
            i += 1;
        }
        let letters: Vec<char> = chars[start..i]
            .iter()
            .copied()
            .filter(|c| c.is_alphabetic())
            .collect();
        runs.push((start, i, letters.len() == 1 && is_greek_letter(letters[0])));
    }
    let operator = |c: Option<char>| c.is_some_and(|c| is_ascii_operator(c) || is_math_operator(c));
    let only_gaps = |from: usize, to: usize| {
        chars[from..to]
            .iter()
            .all(|c| c.is_whitespace() || matches!(c, ',' | ';'))
    };

    let mut count = chars.iter().filter(|c| is_math_operator(**c)).count();
    for (n, &(start, end, lone_greek)) in runs.iter().enumerate() {
        if !lone_greek {
            continue;
        }
        let immediate = [
            start.checked_sub(1).map(|j| chars[j]),
            chars.get(end).copied(),
        ];
        let spaced_before = chars[..start]
            .iter()
            .rev()
            .copied()
            .find(|c| !c.is_whitespace());
        let spaced_after = chars[end..].iter().copied().find(|c| !c.is_whitespace());
        let touching = immediate.iter().flatten().any(|c| {
            is_ascii_operator(*c) || is_math_operator(*c) || is_separator(*c) || c.is_ascii_digit()
        });
        let operator_near = operator(spaced_before) || operator(spaced_after);
        let neighbor = (n > 0 && runs[n - 1].2 && only_gaps(runs[n - 1].1, start))
            || (n + 1 < runs.len() && runs[n + 1].2 && only_gaps(end, runs[n + 1].0));
        if touching || operator_near || neighbor {
            count += 1;
        }
    }
    count
}

/// A one-character token that is normal in prose: the words "a" and "I", and plain digits.
fn is_normal_lone_token(token: &str) -> bool {
    matches!(token, "a" | "A" | "I") || token.chars().all(|c| c.is_ascii_digit())
}

/// True when the page text looks poor. `has_images` says that the page has image objects.
pub fn needs_transcription(text: &str, has_images: bool) -> bool {
    let chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    let total = chars.len();
    if has_images && total < MIN_TEXT_CHARS_WITH_IMAGES {
        return true;
    }
    if total == 0 {
        // A page with no text and no images is blank. There is nothing to transcribe.
        return false;
    }
    let bad = chars.iter().filter(|c| is_bad_char(**c)).count();
    if (bad as f64) / (total as f64) > MAX_BAD_CHAR_SHARE {
        return true;
    }
    if count_math_symbols(text) >= MATH_SYMBOL_THRESHOLD {
        return true;
    }
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.len() >= MIN_TOKENS_FOR_LONE_CHECK {
        let lone = tokens
            .iter()
            .filter(|t| t.chars().count() == 1 && !is_normal_lone_token(t))
            .count();
        if (lone as f64) / (tokens.len() as f64) > MAX_LONE_TOKEN_SHARE {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prose(chars: usize) -> String {
        "The quick brown fox jumps over the lazy dog. "
            .repeat(chars / 45 + 1)
            .chars()
            .take(chars)
            .collect()
    }

    #[test]
    fn normal_prose_uses_the_text_layer() {
        assert!(!needs_transcription(&prose(2000), false));
        assert!(!needs_transcription(&prose(2000), true));
    }

    #[test]
    fn little_text_on_a_page_with_images_needs_transcription() {
        assert!(needs_transcription(&prose(150), true));
        assert!(needs_transcription("", true));
        assert!(
            !needs_transcription(&prose(150), false),
            "short text without images is fine"
        );
        assert!(!needs_transcription(&prose(400), true));
    }

    #[test]
    fn a_blank_page_needs_nothing() {
        assert!(!needs_transcription("", false));
        assert!(!needs_transcription("  \n\n  ", false));
    }

    #[test]
    fn replacement_and_private_use_characters_count() {
        let bad = format!("{}{}", prose(900), "\u{FFFD}".repeat(100));
        assert!(needs_transcription(&bad, false));
        let private = format!("{}{}", prose(900), "\u{E001}".repeat(100));
        assert!(needs_transcription(&private, false));
        // A few of them are tolerated (under 5%).
        let few = format!("{}{}", prose(1900), "\u{FFFD}".repeat(20));
        assert!(!needs_transcription(&few, false));
    }

    #[test]
    fn math_symbols_trigger_it() {
        assert!(needs_transcription(&format!("{} ∑ ∫ ≤", prose(500)), false));
        assert!(needs_transcription(
            &format!("{} α, β, γ", prose(500)),
            false
        ));
        assert!(
            !needs_transcription(&format!("{} ∑ ∫", prose(500)), false),
            "two symbols are not enough"
        );
        assert!(needs_transcription(&format!("{} ± × ÷", prose(500)), false));
    }

    #[test]
    fn isolated_one_character_tokens_trigger_it() {
        let torn = format!(
            "{} {}",
            prose(300),
            "x y z f g h k m n p q r s t u v w".repeat(2)
        );
        assert!(needs_transcription(&torn, false));
        // "a", "I", and plain numbers are normal.
        let normal = "I a 1 2 3 4 5 6 7 8 9 a I a I 1 2 3 4 5 6 7 8 9 and then some words follow";
        assert!(!needs_transcription(normal, false));
        // Too few tokens to judge.
        assert!(!needs_transcription("x y z", false));
    }

    #[test]
    fn counts_characters_not_bytes() {
        // 150 two-byte characters with an image: still under 200 characters.
        assert!(needs_transcription(&"é".repeat(150), true));
        assert!(!needs_transcription(&"é".repeat(250), true));
    }

    #[test]
    fn greek_prose_uses_the_text_layer() {
        let prose = "Ο Πυθαγόρας γεννήθηκε στη Σάμο. Το θεώρημά του συνδέει τις τρεις πλευρές ενός ορθογώνιου τριγώνου. \
                     Η ιδέα είναι απλή: το τετράγωνο της υποτείνουσας ισούται με το άθροισμα των τετραγώνων των δύο άλλων πλευρών. \
                     Οι οικοδόμοι το χρησιμοποιούν για να ελέγχουν αν μια γωνία είναι ορθή. Ο δάσκαλος εξηγεί το θεώρημα στην τάξη.";
        assert!(prose.chars().filter(|c| is_greek_letter(*c)).count() > 100);
        assert!(!needs_transcription(&prose.repeat(3), false));
        // The same prose with combining accent marks (NFD, as XeTeX writes it).
        let decomposed = prose
            .replace('ά', "α\u{0301}")
            .replace('ή', "η\u{0301}")
            .replace('ώ', "ω\u{0301}");
        assert!(!needs_transcription(&decomposed.repeat(3), false));
    }

    #[test]
    fn greek_letters_that_stand_alone_or_touch_operators_are_math() {
        for text in [
            "α + β = γ",
            "Let α, β, γ be real numbers",
            "the angles (α) (β) (γ)",
            "2α = β ± γ",
            "sin θ = a/c, cos θ = b/c, tan θ = a/b, φ = 90 - θ",
        ] {
            assert!(
                count_math_symbols(text) >= 3,
                "{text:?} counted {}",
                count_math_symbols(text)
            );
            assert!(
                needs_transcription(&format!("{} {text}", prose(400)), false),
                "{text:?}"
            );
        }
    }

    #[test]
    fn greek_letters_inside_words_do_not_count() {
        assert_eq!(count_math_symbols("αβγ δελτα Ωμέγα"), 0);
        assert_eq!(
            count_math_symbols("Ο Σωκράτης είναι ο δάσκαλος. Η Αθήνα είναι Η πόλη."),
            0
        );
        assert_eq!(count_math_symbols("ΠΥΘΑΓΟΡΑΣ"), 0);
        // One article between letters never counts; a lone letter in a formula does.
        assert_eq!(count_math_symbols("το ο το"), 0);
        assert_eq!(count_math_symbols("x = α"), 1);
    }

    #[test]
    fn other_math_symbols_still_count_one_each() {
        assert_eq!(count_math_symbols("∑ ∫ ≤ √ ∞ ± × ÷"), 8);
        assert_eq!(count_math_symbols("plain text"), 0);
    }
}
