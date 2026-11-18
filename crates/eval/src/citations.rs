const MAX_MARKER_DIGITS: usize = 3;

fn strip_delimited(text: &str, open: &str, close: &str, single_line: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(open) {
        let after = &rest[start + open.len()..];
        let end = after.find(close).filter(|end| {
            let inside = &after[..*end];
            !(single_line && (inside.contains('\n') || inside.is_empty()))
        });
        match end {
            Some(end) => {
                out.push_str(&rest[..start]);
                out.push(' ');
                rest = &after[end + close.len()..];
            }
            None => {
                out.push_str(&rest[..start + open.len()]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn visible_text(text: &str) -> String {
    let text = strip_delimited(text, "```", "```", false);
    let text = strip_delimited(&text, "~~~", "~~~", false);
    let text = strip_delimited(&text, "$$", "$$", false);
    let text = strip_delimited(&text, "\\[", "\\]", false);
    let text = strip_delimited(&text, "\\(", "\\)", false);
    let text = strip_delimited(&text, "`", "`", true);
    strip_delimited(&text, "$", "$", true)
}

pub fn markers(text: &str) -> Vec<u32> {
    let text = visible_text(text);
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            let digits = bytes[i + 1..]
                .iter()
                .take_while(|b| b.is_ascii_digit())
                .count();
            let close = i + 1 + digits;
            let well_formed = (1..=MAX_MARKER_DIGITS).contains(&digits)
                && bytes.get(close) == Some(&b']')
                && bytes.get(close + 1) != Some(&b'(');
            if well_formed && let Ok(n) = text[i + 1..close].parse::<u32>() {
                found.push(n);
                i = close + 1;
                continue;
            }
        }
        i += 1;
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_markers_are_read_in_order_with_repeats() {
        assert_eq!(
            markers("A fact [1]. Another [2][3]. Again [1]."),
            [1, 2, 3, 1]
        );
        assert!(markers("no markers here").is_empty());
    }

    #[test]
    fn markers_follow_the_rules_of_the_web_app() {
        assert!(markers("[1234] is too long").is_empty());
        assert!(markers("[] and [a] and [1a]").is_empty());
        assert!(markers("a half link [1](http://x)").is_empty());
        assert_eq!(markers("see [12]"), [12]);
    }

    #[test]
    fn code_and_math_are_not_citations() {
        assert!(markers("The interval $[0, 1]$ and $x_{[2]}$").is_empty());
        assert!(markers("Inline `a[1]` and a block:\n```\nb[2]\n```").is_empty());
        assert!(markers("Display $$ a[3] $$ and \\( c[4] \\) and \\[ d[5] \\]").is_empty());
        assert_eq!(markers("Math $x$ then a cite [2] and `code`."), [2]);
    }

    #[test]
    fn an_open_delimiter_without_a_close_changes_nothing() {
        assert_eq!(markers("it costs $5 and is cited [1]"), [1]);
        assert_eq!(markers("a ``` that never closes [2]"), [2]);
    }
}
