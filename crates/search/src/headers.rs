//! Running headers and footers.
//!
//! A book repeats a line at the top (or the bottom) of many pages: "SECTION 1.2 EXPONENTS AND
//! SCIENTIFIC NOTATION", "CHAPTER 1 PREREQUISITES", a bare page number. The page Markdown keeps it,
//! the chunker reads it as a heading, and it then reaches the heading path, the embedded text, and
//! the BM25 heading field of many chunks that have nothing to do with it.
//!
//! `strip_running_lines` runs between the page text and the chunker, for the whole document. The
//! ingestion worker and the evaluation both reach it through `chunker::chunk_pages_with`, so they
//! run the same code. The stored page files are not changed.
//!
//! The rule:
//!
//! 1. Only the first and the last `EDGE_LINES` non-empty lines of a page are looked at.
//! 2. A line is a candidate when it is short (at most `MAX_LINE_CHARS`), has at least two words,
//!    and is not a list item, a table row, math, a code fence, or a sentence (it does not end in
//!    `.`, `?`, `!`, or `:`).
//! 3. Candidates are compared after a normalization: Markdown heading marks and emphasis are
//!    removed, case is ignored, white space is collapsed, and numbers at the start or at the end of
//!    the line (page numbers) are dropped. A number inside the line stays, so "Try It #4" and
//!    "Try It #5", or "SECTION 1.2" and "SECTION 1.3", are not the same line.
//! 4. A normalized line is a running line when it is at the edge of at least `MIN_PAGES` pages and
//!    those pages are at least `MIN_DENSITY` of all the pages from its first to its last
//!    occurrence. The density is taken over the span and not over the whole document, because a
//!    chapter header repeats only inside its chapter: in a 300-page book it is on 10 percent of
//!    the pages, but on 50 percent of the pages of its chapter (every second page).
//! 5. The first occurrence in the document stays (it is often the real section heading) and the
//!    later ones are removed. A line that is only a page number (all its words are numbers) has no
//!    first occurrence to keep: every one is removed.
//!
//! A page that holds only a running line becomes empty, and the chunker skips an empty page. A
//! document with fewer than `MIN_PAGES` pages cannot have a running line.

use std::collections::{BTreeMap, BTreeSet};

/// How many non-empty lines at the top and at the bottom of a page are looked at.
pub const EDGE_LINES: usize = 3;
/// The least number of pages that a running line must be on.
pub const MIN_PAGES: usize = 3;
/// The pages with the line, as a share of the pages from its first to its last occurrence.
pub const MIN_DENSITY: f64 = 0.3;
/// A longer line is text, not a header.
pub const MAX_LINE_CHARS: usize = 100;

/// One line that was found to run through the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningLine {
    /// The normalized form that the pages were compared by. Empty for a page number.
    pub key: String,
    /// The first occurrence as written.
    pub first_text: String,
    /// The page number of the first occurrence.
    pub first_page: u32,
    /// The pages that the line was removed from.
    pub removed_from: Vec<u32>,
}

/// The pages without their running lines, and what was removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stripped {
    pub pages: Vec<(u32, String)>,
    pub lines: Vec<RunningLine>,
}

/// The normalized key of a line, or `None` when the line cannot be a running line. An empty key
/// means that the line is only a page number.
pub fn running_key(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_LINE_CHARS {
        return None;
    }
    if trimmed.contains(['$', '|', '`']) || trimmed.starts_with('[') {
        return None;
    }
    let text = trimmed.trim_start_matches('#').trim();
    let text: String = text
        .chars()
        .filter(|c| !matches!(c, '*' | '_' | '\u{ad}'))
        .collect();
    if is_list_item(&text) {
        return None;
    }
    let text = text.trim();
    if text.ends_with(['.', '?', '!', ':']) {
        return None;
    }
    let lowered = text.to_lowercase();
    let mut words: Vec<&str> = lowered.split_whitespace().collect();
    // Page numbers: numbers at the start or at the end, and "page", "-" around them.
    let is_number =
        |w: &str| !w.is_empty() && w.len() <= 5 && w.bytes().all(|b| b.is_ascii_digit());
    while words.first().is_some_and(|w| is_number(w)) {
        words.remove(0);
    }
    while words.last().is_some_and(|w| is_number(w)) {
        words.pop();
    }
    if words.is_empty() {
        // Only numbers: a page number.
        return text.split_whitespace().all(is_number).then(String::new);
    }
    if words
        .iter()
        .filter(|w| w.chars().any(char::is_alphabetic))
        .count()
        < 2
    {
        return None;
    }
    Some(words.join(" "))
}

fn is_list_item(text: &str) -> bool {
    let t = text.trim_start();
    if t.starts_with(['-', '+', '\u{2022}']) {
        return true;
    }
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    digits > 0 && t[digits..].starts_with(['.', ')']) && t[digits + 1..].starts_with(' ')
}

/// The indexes of the lines at the edge of a page: the first and last `EDGE_LINES` non-empty ones.
fn edge_indexes(lines: &[&str]) -> Vec<usize> {
    let filled: Vec<usize> = (0..lines.len())
        .filter(|i| !lines[*i].trim().is_empty())
        .collect();
    let mut edge = BTreeSet::new();
    edge.extend(filled.iter().take(EDGE_LINES));
    edge.extend(filled.iter().rev().take(EDGE_LINES));
    edge.into_iter().collect()
}

/// Removes the running headers and footers of a document. `pages` has `(page number, Markdown)` in
/// page order. A page that has nothing to remove comes back unchanged, byte for byte.
pub fn strip_running_lines(pages: &[(u32, String)]) -> Stripped {
    // For each page: (line index, key) of the candidate lines at its edge.
    let mut edges: Vec<Vec<(usize, String)>> = Vec::with_capacity(pages.len());
    // For each key: the positions (indexes into `pages`) of the pages that have it.
    let mut seen: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    // For each key: the first occurrence as written.
    let mut first_text: BTreeMap<String, String> = BTreeMap::new();
    for (position, (_, markdown)) in pages.iter().enumerate() {
        let lines: Vec<&str> = markdown.lines().collect();
        let mut found = Vec::new();
        for index in edge_indexes(&lines) {
            if let Some(key) = running_key(lines[index]) {
                first_text
                    .entry(key.clone())
                    .or_insert_with(|| lines[index].trim().to_owned());
                let at = seen.entry(key.clone()).or_default();
                if at.last() != Some(&position) {
                    at.push(position);
                }
                found.push((index, key));
            }
        }
        edges.push(found);
    }

    let running: BTreeSet<String> = seen
        .iter()
        .filter(|(_, at)| {
            let (first, last) = (at[0], at[at.len() - 1]);
            at.len() >= MIN_PAGES && at.len() as f64 / (last - first + 1) as f64 >= MIN_DENSITY
        })
        .map(|(key, _)| key.clone())
        .collect();
    if running.is_empty() {
        return Stripped {
            pages: pages.to_vec(),
            lines: Vec::new(),
        };
    }

    let mut out = Vec::with_capacity(pages.len());
    let mut removed: BTreeMap<String, RunningLine> = BTreeMap::new();
    for (position, (number, markdown)) in pages.iter().enumerate() {
        let lines: Vec<&str> = markdown.lines().collect();
        let mut drop: BTreeSet<usize> = BTreeSet::new();
        let mut kept_here: BTreeSet<&str> = BTreeSet::new();
        for (index, key) in &edges[position] {
            if !running.contains(key) {
                continue;
            }
            // The first occurrence stays (once on its page). A page number has none to keep.
            if seen[key][0] == position && !key.is_empty() && kept_here.insert(key.as_str()) {
                continue;
            }
            drop.insert(*index);
            removed
                .entry(key.clone())
                .or_insert_with(|| RunningLine {
                    key: key.clone(),
                    first_text: first_text[key].clone(),
                    first_page: pages[seen[key][0]].0,
                    removed_from: Vec::new(),
                })
                .removed_from
                .push(*number);
        }
        if drop.is_empty() {
            out.push((*number, markdown.clone()));
            continue;
        }
        let kept: Vec<&str> = lines
            .iter()
            .enumerate()
            .filter(|(i, _)| !drop.contains(i))
            .map(|(_, l)| *l)
            .collect();
        out.push((*number, kept.join("\n").trim_matches('\n').to_owned()));
    }
    let mut lines: Vec<RunningLine> = removed.into_values().collect();
    lines.sort_by_key(|l| l.first_page);
    Stripped { pages: out, lines }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(pages: &[&str]) -> Vec<(u32, String)> {
        pages
            .iter()
            .enumerate()
            .map(|(i, t)| (u32::try_from(i + 1).unwrap(), (*t).to_owned()))
            .collect()
    }

    fn texts(stripped: &Stripped) -> Vec<&str> {
        stripped.pages.iter().map(|(_, t)| t.as_str()).collect()
    }

    #[test]
    fn a_header_on_every_other_page_is_removed_and_the_first_one_stays() {
        let pages = doc(&[
            "# SECTION 1.2 EXPONENTS\n\nBody one.",
            "# CHAPTER 1 PREREQUISITES\n\nBody two.",
            "# SECTION 1.2 EXPONENTS\n\nBody three.",
            "# CHAPTER 1 PREREQUISITES\n\nBody four.",
            "# SECTION 1.2 EXPONENTS\n\nBody five.",
            "# CHAPTER 1 PREREQUISITES\n\nBody six.",
        ]);
        let out = strip_running_lines(&pages);
        assert_eq!(
            texts(&out),
            [
                "# SECTION 1.2 EXPONENTS\n\nBody one.",
                "# CHAPTER 1 PREREQUISITES\n\nBody two.",
                "Body three.",
                "Body four.",
                "Body five.",
                "Body six.",
            ]
        );
        assert_eq!(out.lines.len(), 2);
        let section = out
            .lines
            .iter()
            .find(|l| l.key.starts_with("section"))
            .unwrap();
        assert_eq!(section.first_page, 1);
        assert_eq!(section.removed_from, [3, 5]);
    }

    #[test]
    fn case_marks_and_page_numbers_do_not_make_lines_different() {
        let pages = doc(&[
            "SECTION 5.1 QUADRATIC FUNCTIONS 343\n\nA.",
            "# 344 Section 5.1 Quadratic Functions\n\nB.",
            "**Section 5.1 Quadratic Functions** 345\n\nC.",
            "## SECTION 5.1 QUADRATIC FUNCTIONS\n\nD.",
        ]);
        let out = strip_running_lines(&pages);
        assert_eq!(texts(&out)[1..], ["B.", "C.", "D."]);
        assert_eq!(texts(&out)[0], pages[0].1);
    }

    #[test]
    fn a_footer_is_found_at_the_bottom() {
        let pages = doc(&[
            "Text one.\n\nOpenbook Course Notes",
            "Text two.\n\nOpenbook Course Notes",
            "Text three.\n\nOpenbook Course Notes",
        ]);
        let out = strip_running_lines(&pages);
        assert_eq!(
            texts(&out),
            [
                "Text one.\n\nOpenbook Course Notes",
                "Text two.",
                "Text three."
            ]
        );
    }

    #[test]
    fn bare_page_numbers_are_all_removed() {
        let pages = doc(&[
            "Intro text here.\n\n1",
            "Second page text.\n\n2",
            "Third page text.\n\n3",
            "Fourth page text.\n\n- 4 -",
        ]);
        let out = strip_running_lines(&pages);
        assert_eq!(
            texts(&out),
            [
                "Intro text here.",
                "Second page text.",
                "Third page text.",
                "Fourth page text.\n\n- 4 -",
            ],
            "a dash-wrapped number is a list item and stays"
        );
    }

    #[test]
    fn a_numbered_label_is_not_a_running_header() {
        // "Try It #4" and "Try It #5" differ by a number inside the line.
        let pages = doc(&[
            "## Try It #1\n\nA.",
            "## Try It #2\n\nB.",
            "## Try It #3\n\nC.",
            "## Try It #4\n\nD.",
        ]);
        let out = strip_running_lines(&pages);
        assert_eq!(out.pages, pages);
        assert!(out.lines.is_empty());
    }

    #[test]
    fn single_words_sentences_lists_tables_and_math_are_never_running_lines() {
        for line in [
            "Solution",
            "This line repeats and it is a sentence.",
            "Is this a question?",
            "- two words",
            "1. two words",
            "| a | b |",
            "$$a^2 + b^2$$",
            "[Figure: a graph of a line]",
        ] {
            let page = format!("{line}\n\nBody.");
            let pages = doc(&[&page, &page, &page, &page]);
            let out = strip_running_lines(&pages);
            assert_eq!(out.pages, pages, "{line}");
        }
    }

    #[test]
    fn a_line_in_the_middle_of_a_page_is_left_alone() {
        let middle = "Alpha.\n\nBeta.\n\nGamma.\n\nDelta.\n\nGOING THROUGH THE MIDDLE\n\nEpsilon.\n\nZeta.\n\nEta.\n\nTheta.";
        let pages = doc(&[middle, middle, middle, middle]);
        assert_eq!(strip_running_lines(&pages).pages, pages);
    }

    #[test]
    fn a_line_must_repeat_on_enough_pages_and_often_enough() {
        // Two pages are too few.
        let few = doc(&["RUNNING TITLE\n\nA.", "RUNNING TITLE\n\nB.", "C.", "D."]);
        assert_eq!(strip_running_lines(&few).pages, few);
        // Three pages far apart in a long document are too sparse (3 of 20 is 0.15).
        let mut long: Vec<String> = (0..20).map(|i| format!("Page {i} text.")).collect();
        for at in [0, 9, 19] {
            long[at] = format!("RUNNING TITLE\n\n{}", long[at]);
        }
        let long_pages = doc(&long.iter().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(strip_running_lines(&long_pages).pages, long_pages);
    }

    #[test]
    fn a_header_that_covers_one_chapter_of_a_long_book_is_found() {
        // A chapter header on every second page of pages 41 to 60 of a 100-page book: 10 of 100
        // pages, but 10 of 20 pages of its span.
        let pages: Vec<(u32, String)> = (1..=100u32)
            .map(|n| {
                let header = if (41..=60).contains(&n) && n % 2 == 1 {
                    "CHAPTER 4 GEOMETRY\n\n"
                } else {
                    ""
                };
                (n, format!("{header}Text of page {n}."))
            })
            .collect();
        let out = strip_running_lines(&pages);
        assert_eq!(out.lines.len(), 1);
        assert_eq!(out.lines[0].first_page, 41);
        assert_eq!(out.lines[0].removed_from.len(), 9);
        assert!(
            out.pages[42].1.starts_with("Text of page 43"),
            "{:?}",
            out.pages[42]
        );
        assert!(out.pages[40].1.starts_with("CHAPTER 4"));
    }

    #[test]
    fn small_documents_are_left_alone() {
        assert_eq!(strip_running_lines(&[]).pages, Vec::new());
        let one = doc(&["# A TITLE HERE\n\nText."]);
        assert_eq!(strip_running_lines(&one).pages, one);
        let two = doc(&["A TITLE HERE\n\nOne.", "A TITLE HERE\n\nTwo."]);
        assert_eq!(strip_running_lines(&two).pages, two);
    }

    #[test]
    fn a_page_that_is_only_a_header_becomes_empty() {
        let pages = doc(&[
            "RUNNING TITLE\n\nFirst.",
            "RUNNING TITLE",
            "RUNNING TITLE\n\nThird.",
            "RUNNING TITLE\n\nFourth.",
        ]);
        let out = strip_running_lines(&pages);
        assert_eq!(
            texts(&out),
            ["RUNNING TITLE\n\nFirst.", "", "Third.", "Fourth."]
        );
    }

    #[test]
    fn a_header_at_the_top_and_the_bottom_of_a_page_is_kept_once_on_its_first_page() {
        let pages = doc(&[
            "RUNNING TITLE\n\nOne.\n\nRUNNING TITLE",
            "RUNNING TITLE\n\nTwo.\n\nRUNNING TITLE",
            "RUNNING TITLE\n\nThree.\n\nRUNNING TITLE",
        ]);
        let out = strip_running_lines(&pages);
        assert_eq!(texts(&out), ["RUNNING TITLE\n\nOne.", "Two.", "Three."]);
    }

    #[test]
    fn a_page_without_a_running_line_comes_back_byte_for_byte() {
        let pages = doc(&[
            "SECTION 2 TITLE\n\nOne.\n",
            "Plain page with a trailing newline.\n",
            "SECTION 2 TITLE\n\nThree.",
            "SECTION 2 TITLE\n\nFour.",
        ]);
        let out = strip_running_lines(&pages);
        assert_eq!(out.pages[0], pages[0]);
        assert_eq!(out.pages[1], pages[1]);
        assert_eq!(out.pages[2].1, "Three.");
    }

    /// The start of real pages of the first evaluation corpus (OpenStax College Algebra, CC BY 4.0,
    /// section 1.2, pages 1 to 8 of the excerpt), as the ingestion made them.
    #[test]
    fn real_pages_of_the_algebra_corpus() {
        let pages = doc(&[
            "# SECTION 1.2 EXPONENTS AND SCIENTIFIC NOTATION\n\n## LEARNING OBJECTIVES\n\nIn this section students will:\n- Use the product rule of exponents.\n\nFor any real number $a$ and natural numbers $m$ and $n$, the product rule of exponents states that\n\n$$a^m \\cdot a^n = a^{m+n}$$",
            "# CHAPTER 1 PREREQUISITES\n\n## Example 1 Using the Product Rule\n\nWrite each of the following products with a single base. Do not simplify further.\na. $t^5 \\cdot t^7$",
            "# SECTION 1.2 EXPONENTS AND SCIENTIFIC NOTATION\n\nSolution Use the quotient rule to simplify each expression.\n\na. $\\frac{(-2)^5}{(-2)^3} = (-2)^{5-3} = (-2)^2$",
            "# CHAPTER 1 PREREQUISITES\n\n## Try # 3\n\nWrite each of the following products with a single base. Do not simplify further.",
            "# SECTION 1.2 EXPONENTS AND SCIENTIFIC NOTATION\n\n## Try It #4\n\nb. $\\frac{z^2}{z}$",
            "# CHAPTER 1 PREREQUISITES\n\n## Try It #5\n\nd. $\\frac{1}{(-7z)^2}$",
            "# SECTION 1.2 EXPONENTS AND SCIENTIFIC NOTATION\n\n## Try It #7\n\nd. $(jk^{-3})^7$",
            "# CHAPTER 1 PREREQUISITES\n\n## Try It #2\n\nThe quotient rule and reduce fraction\n\nSimplify.",
        ]);
        let out = strip_running_lines(&pages);
        // Page 1 keeps its real section heading, page 2 keeps the first chapter header.
        assert_eq!(out.pages[0], pages[0]);
        assert_eq!(out.pages[1], pages[1]);
        for n in 2..8 {
            let text = &out.pages[n].1;
            assert!(!text.contains("SECTION 1.2"), "page {}: {text}", n + 1);
            assert!(
                !text.contains("CHAPTER 1 PREREQUISITES"),
                "page {}: {text}",
                n + 1
            );
        }
        // The numbered labels stay.
        assert!(out.pages[4].1.starts_with("## Try It #4"));
        assert!(out.pages[3].1.starts_with("## Try # 3"));
        assert_eq!(out.lines.len(), 2);
    }

    /// Real pages from the same book where the text layer is one long line: nothing is removed,
    /// because the rule works on lines.
    #[test]
    fn a_page_that_is_one_long_line_is_left_alone() {
        let one_line = |n: u32| {
            format!(
                "{n} CHAPTER 3 Functions 3.1 Functions and Function Notation A jetliner changes altitude as its distance from the starting point of a flight increases. The weight of a growing child increases with time."
            )
        };
        let pages: Vec<(u32, String)> = (1..=5).map(|n| (n, one_line(n))).collect();
        assert_eq!(strip_running_lines(&pages).pages, pages);
    }
}
