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

fn is_math_symbol(c: char) -> bool {
    let code = c as u32;
    // Mathematical operators, supplemental operators, Greek letters, and a few Latin-1 signs.
    (0x2200..=0x22FF).contains(&code)
        || (0x2A00..=0x2AFF).contains(&code)
        || (0x27C0..=0x27EF).contains(&code)
        || (0x0391..=0x03C9).contains(&code)
        || matches!(c, '±' | '×' | '÷' | '∑' | '∫' | '√' | '∞')
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
    if chars.iter().filter(|c| is_math_symbol(**c)).count() >= MATH_SYMBOL_THRESHOLD {
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
        assert!(needs_transcription(&format!("{} α β γ", prose(500)), false));
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
}
