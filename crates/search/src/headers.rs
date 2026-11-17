//! Running headers and footers.
//!
//! A book repeats a phrase at the top or at the bottom of many pages: "SECTION 1.2 EXPONENTS AND
//! SCIENTIFIC NOTATION", "Chapter 2 | The Constitution and Its Origins", "This OpenStax book is
//! available for free at ...", a page number. The page Markdown keeps it, the chunker reads a
//! header line as a heading, and it then reaches the heading path, the embedded text, and the BM25
//! heading field of many chunks that have nothing to do with it. On a page that comes from the text
//! layer of a PDF (a page that needs no transcription), the header is not even a line: the text
//! layer joins the lines of a paragraph, so the header sits inside one long paragraph, at the start
//! or at the end of it. So the rule works on words and not on lines.
//!
//! `strip_running_lines` runs between the page text and the chunker, for the whole document. The
//! ingestion worker and the evaluation both reach it through `chunker::chunk_pages_with`, so they
//! run the same code. The stored page files are not changed.
//!
//! The rule:
//!
//! 1. Only the first and the last `EDGE_WORDS` words of a page are looked at.
//! 2. A candidate is a run of `MIN_PHRASE_WORDS` to `MAX_PHRASE_WORDS` words inside one of these
//!    windows and inside one line. It has at least two words with letters. A line with a `$` or a
//!    backtick (math and code) gives no candidate.
//! 3. Words are compared after a normalization: case is ignored and Markdown marks (`#`, `*`, `_`)
//!    are dropped. A candidate does not start or end with a number, so the page number next to a
//!    header ("40 Chapter 2 | The Constitution", "The Constitution 41") is not part of it. A number
//!    inside a phrase stays, so "SECTION 1.2" and "SECTION 1.3", or "Try It #4" and "Try It #5",
//!    are not the same phrase.
//! 4. A phrase is a running phrase when it is at the edge of at least `MIN_PAGES` pages, those pages
//!    are at least `MIN_DENSITY` of all the pages from its first to its last occurrence, and at
//!    least `MIN_EDGE_SHARE` of all its occurrences in the document are at an edge. The density is
//!    taken over the span and not over the whole document, because a chapter header repeats only
//!    inside its chapter: in a 300-page book it is on 10 percent of the pages, but on 50 percent of
//!    the pages of its chapter (every second page). The edge share keeps out the phrases that are
//!    common in the text ("one of the").
//! 5. When two candidates compete, the one with more pages wins, then the longer one.
//! 6. The first occurrence in the document stays when it is at the top of its page (it is often
//!    the real section heading), and the later ones are removed. An occurrence at the bottom of the
//!    first page is removed too: a footer is not a heading.
//! 7. When a header is removed, a Markdown mark in front of it goes with it, and so does the page
//!    number that is next to it at the edge of the page.
//! 8. A number alone at the start or the end of a page is a page number when it is there on at
//!    least `MIN_PAGES` pages, at the density above, and the numbers grow from page to page. It is
//!    removed on every page, the first one included.
//!
//! A page that holds only a running phrase becomes empty, and the chunker skips an empty page. A
//! document with fewer than `MIN_PAGES` pages has no running phrase.

use std::collections::{BTreeSet, HashMap};

/// How many words at the top and at the bottom of a page are looked at.
pub const EDGE_WORDS: usize = 30;
/// A phrase has at least this many words (so "Try It" and "Chapter Review" are never removed).
pub const MIN_PHRASE_WORDS: usize = 3;
/// A longer phrase is text, not a header.
pub const MAX_PHRASE_WORDS: usize = 20;
/// The least number of pages that a running phrase must be on.
pub const MIN_PAGES: usize = 3;
/// The pages with the phrase, as a share of the pages from its first to its last occurrence.
pub const MIN_DENSITY: f64 = 0.3;
/// The share of all occurrences of a phrase that must be at the edge of a page.
pub const MIN_EDGE_SHARE: f64 = 0.8;

/// One phrase that was found to run through the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningLine {
    /// The normalized form that the pages were compared by. Empty for page numbers.
    pub key: String,
    /// The first occurrence as written.
    pub first_text: String,
    /// The page number of the first occurrence.
    pub first_page: u32,
    /// The page numbers that the phrase was removed from.
    pub removed_from: Vec<u32>,
}

/// The pages without their running phrases, and what was removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stripped {
    pub pages: Vec<(u32, String)>,
    pub lines: Vec<RunningLine>,
}

/// A word of a page.
#[derive(Debug, Clone)]
struct Tok {
    start: usize,
    end: usize,
    line: usize,
    /// Lower case, without Markdown marks. Empty for a word that is only marks.
    norm: String,
    hash: u64,
    is_int: bool,
}

impl Tok {
    fn is_mark(&self) -> bool {
        self.norm.is_empty()
    }
}

fn fnv(text: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn is_number(word: &str) -> bool {
    !word.is_empty() && word.len() <= 5 && word.bytes().all(|b| b.is_ascii_digit())
}

fn tokenize(text: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut line = 0;
    let mut start: Option<usize> = None;
    let push = |out: &mut Vec<Tok>, start: usize, end: usize, line: usize| {
        let raw = &text[start..end];
        // A word that is only marks is a Markdown mark. `#4` is not: it is a label.
        let norm = if raw.chars().all(|c| matches!(c, '#' | '*' | '_' | '>')) {
            String::new()
        } else {
            raw.chars()
                .filter(|c| !matches!(c, '*' | '_' | '\u{ad}'))
                .collect::<String>()
                .to_lowercase()
        };
        out.push(Tok {
            start,
            end,
            line,
            hash: fnv(&norm),
            is_int: is_number(&norm),
            norm,
        });
    };
    for (i, c) in text.char_indices() {
        if c.is_whitespace() {
            if let Some(s) = start.take() {
                push(&mut out, s, i, line);
            }
            if c == '\n' {
                line += 1;
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        push(&mut out, s, text.len(), line);
    }
    out
}

/// The normalized form of a phrase: marks dropped, numbers at the start and at the end dropped.
/// Used to compare a heading with the running phrases of a document.
pub fn phrase_key(text: &str) -> String {
    let mut words: Vec<String> = tokenize(text)
        .into_iter()
        .filter(|t| !t.is_mark())
        .map(|t| t.norm)
        .collect();
    while words.first().is_some_and(|w| is_number(w)) {
        words.remove(0);
    }
    while words.last().is_some_and(|w| is_number(w)) {
        words.pop();
    }
    words.join(" ")
}

/// A page with its words. `seq` holds the indexes of the words that are not marks.
struct Page {
    number: u32,
    text: String,
    toks: Vec<Tok>,
    seq: Vec<usize>,
    /// The lines that hold math (`$`) or code (a backtick): they give no candidates.
    code_lines: BTreeSet<usize>,
}

impl Page {
    fn is_edge(&self, p: usize) -> bool {
        p < EDGE_WORDS || p + EDGE_WORDS >= self.seq.len()
    }

    fn tok(&self, p: usize) -> &Tok {
        &self.toks[self.seq[p]]
    }
}

/// A run of words that could be a running phrase.
struct Candidate {
    len: usize,
    /// Positions (into the page list) of the pages that have it at an edge.
    pages: Vec<usize>,
    /// Where one occurrence is: (page, position in `seq`).
    example: (usize, usize),
}

fn candidates(pages: &[Page]) -> HashMap<u64, Candidate> {
    let mut map: HashMap<u64, Candidate> = HashMap::new();
    for (pi, page) in pages.iter().enumerate() {
        let n = page.seq.len();
        let head_end = EDGE_WORDS.min(n);
        let tail_start = n.saturating_sub(EDGE_WORDS);
        for (from, to) in [(0, head_end), (tail_start, n)] {
            for start in from..to {
                let first = page.tok(start);
                if first.is_int || page.code_lines.contains(&first.line) {
                    continue;
                }
                let mut hash = 0xcbf2_9ce4_8422_2325_u64;
                let mut letters = 0;
                for end in start..to.min(start + MAX_PHRASE_WORDS) {
                    let t = page.tok(end);
                    if t.line != first.line {
                        break;
                    }
                    hash = (hash ^ t.hash).wrapping_mul(0x0100_0000_01b3);
                    if t.norm.chars().any(char::is_alphabetic) {
                        letters += 1;
                    }
                    let len = end - start + 1;
                    if len < MIN_PHRASE_WORDS || letters < 2 || t.is_int {
                        continue;
                    }
                    let key = hash ^ (len as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
                    let entry = map.entry(key).or_insert_with(|| Candidate {
                        len,
                        pages: Vec::new(),
                        example: (pi, start),
                    });
                    if entry.pages.last() != Some(&pi) {
                        entry.pages.push(pi);
                    }
                }
            }
        }
    }
    map
}

fn density_ok(pages: &[usize]) -> bool {
    if pages.len() < MIN_PAGES {
        return false;
    }
    let (first, last) = (pages[0], pages[pages.len() - 1]);
    pages.len() as f64 / (last - first + 1) as f64 >= MIN_DENSITY
}

/// Where the words of `phrase` occur in a page: positions in `seq`.
fn occurrences(page: &Page, phrase: &[&str]) -> Vec<usize> {
    let n = page.seq.len();
    let len = phrase.len();
    let mut found = Vec::new();
    if n < len {
        return found;
    }
    for start in 0..=n - len {
        if (0..len).all(|k| page.tok(start + k).norm == phrase[k]) {
            found.push(start);
        }
    }
    found
}

/// Removes the running headers and footers of a document. `pages` has `(page number, Markdown)` in
/// page order. A page that has nothing to remove comes back unchanged, byte for byte.
pub fn strip_running_lines(pages: &[(u32, String)]) -> Stripped {
    if pages.len() < MIN_PAGES {
        return Stripped {
            pages: pages.to_vec(),
            lines: Vec::new(),
        };
    }
    let pages_t: Vec<Page> = pages
        .iter()
        .map(|(number, text)| {
            let toks = tokenize(text);
            let seq = (0..toks.len()).filter(|i| !toks[*i].is_mark()).collect();
            let code_lines = toks
                .iter()
                .filter(|t| t.norm.contains(['$', '`']))
                .map(|t| t.line)
                .collect();
            Page {
                number: *number,
                text: text.clone(),
                toks,
                seq,
                code_lines,
            }
        })
        .collect();

    let mut ranked: Vec<Candidate> = candidates(&pages_t)
        .into_values()
        .filter(|c| density_ok(&c.pages))
        .collect();
    // More pages first, then the longer phrase, then the earlier one (so the order never changes).
    ranked.sort_by(|a, b| {
        b.pages
            .len()
            .cmp(&a.pages.len())
            .then(b.len.cmp(&a.len))
            .then(a.example.cmp(&b.example))
    });

    // removed[page][index into toks]
    let mut removed: Vec<Vec<bool>> = pages_t.iter().map(|p| vec![false; p.toks.len()]).collect();
    // consumed[page][position in seq]: taken by a phrase, kept or removed
    let mut consumed: Vec<Vec<bool>> = pages_t.iter().map(|p| vec![false; p.seq.len()]).collect();
    let mut lines: Vec<RunningLine> = Vec::new();

    for cand in &ranked {
        let (ep, es) = cand.example;
        let phrase: Vec<&str> = (0..cand.len)
            .map(|k| pages_t[ep].tok(es + k).norm.as_str())
            .collect();
        // The occurrences at an edge that no phrase with more pages has taken, and the share of
        // all occurrences that is at an edge.
        let mut edge: Vec<(usize, usize)> = Vec::new();
        let (mut total, mut at_edge) = (0usize, 0usize);
        for (pi, page) in pages_t.iter().enumerate() {
            for start in occurrences(page, &phrase) {
                total += 1;
                let edgy = page.is_edge(start) && page.is_edge(start + cand.len - 1);
                at_edge += usize::from(edgy);
                let free = (start..start + cand.len).all(|p| !consumed[pi][p]);
                let one_line = page.tok(start).line == page.tok(start + cand.len - 1).line;
                if edgy && free && one_line {
                    edge.push((pi, start));
                }
            }
        }
        let mut page_list: Vec<usize> = edge.iter().map(|(pi, _)| *pi).collect();
        page_list.dedup();
        if !density_ok(&page_list) || (at_edge as f64) < MIN_EDGE_SHARE * total as f64 {
            continue;
        }
        let first_page = page_list[0];
        let mut kept_first = false;
        let first_text = {
            let (pi, start) = edge[0];
            let page = &pages_t[pi];
            page.text[page.tok(start).start..page.tok(start + cand.len - 1).end].to_owned()
        };
        let mut gone: Vec<u32> = Vec::new();
        for (pi, start) in &edge {
            let page = &pages_t[*pi];
            consumed[*pi][*start..*start + cand.len].fill(true);
            // At the top of its page: nearer to the start of the page than to its end.
            let at_top = *start <= page.seq.len() - (*start + cand.len);
            let keep = *pi == first_page && at_top && !kept_first;
            if keep {
                kept_first = true;
                continue;
            }
            for p in *start..*start + cand.len {
                removed[*pi][page.seq[p]] = true;
            }
            // Heading marks in front of the phrase go with it.
            let mut t = page.seq[*start];
            while t > 0 && page.toks[t - 1].is_mark() {
                t -= 1;
                removed[*pi][t] = true;
            }
            if gone.last() != Some(&page.number) {
                gone.push(page.number);
            }
        }
        if !gone.is_empty() {
            lines.push(RunningLine {
                key: phrase.join(" "),
                first_text,
                first_page: pages_t[first_page].number,
                removed_from: gone,
            });
        }
    }

    // The page number next to a removed phrase at the edge of a page, and the marks in front of it.
    for (pi, page) in pages_t.iter().enumerate() {
        let toks = &page.toks;
        let run = |range: &mut dyn Iterator<Item = usize>, removed: &[bool]| -> Vec<usize> {
            range
                .take_while(|i| removed[*i] || toks[*i].is_mark() || toks[*i].is_int)
                .collect()
        };
        // The top: the words from the start that are removed, marks, or numbers.
        let head = run(&mut (0..toks.len()), &removed[pi]);
        if let Some(last) = head.iter().copied().filter(|i| removed[pi][*i]).max() {
            for i in head {
                if removed[pi][i] || toks[i].is_int || i < last {
                    removed[pi][i] = true;
                }
            }
        }
        // The bottom: the same, from the end.
        let tail = run(&mut (0..toks.len()).rev(), &removed[pi]);
        if let Some(first) = tail.iter().copied().filter(|i| removed[pi][*i]).min() {
            for i in tail {
                if removed[pi][i] || toks[i].is_int || i > first {
                    removed[pi][i] = true;
                }
            }
        }
    }

    // A number alone at the start or at the end of the pages: page numbers.
    for from_end in [false, true] {
        let mut found: Vec<(usize, usize, u64)> = Vec::new();
        for (pi, page) in pages_t.iter().enumerate() {
            let alive: Vec<usize> = (0..page.toks.len()).filter(|i| !removed[pi][*i]).collect();
            let pick = if from_end {
                alive.last()
            } else {
                alive.first()
            };
            if let Some(&i) = pick
                && page.toks[i].is_int
                && let Ok(value) = page.toks[i].norm.parse::<u64>()
            {
                found.push((pi, i, value));
            }
        }
        let positions: Vec<usize> = found.iter().map(|f| f.0).collect();
        let growing = found.windows(2).all(|w| w[0].2 < w[1].2);
        if density_ok(&positions) && growing {
            let (fp, fi, _) = found[0];
            let first_text =
                pages_t[fp].text[pages_t[fp].toks[fi].start..pages_t[fp].toks[fi].end].to_owned();
            for (pi, i, _) in &found {
                removed[*pi][*i] = true;
            }
            lines.push(RunningLine {
                key: String::new(),
                first_text,
                first_page: pages_t[fp].number,
                removed_from: found.iter().map(|f| pages_t[f.0].number).collect(),
            });
        }
    }

    let out = pages_t
        .iter()
        .enumerate()
        .map(|(pi, page)| {
            if removed[pi].iter().any(|r| *r) {
                (page.number, rebuild(&page.text, &page.toks, &removed[pi]))
            } else {
                (page.number, page.text.clone())
            }
        })
        .collect();
    lines.sort_by(|a, b| a.first_page.cmp(&b.first_page).then(a.key.cmp(&b.key)));
    Stripped { pages: out, lines }
}

/// The page text without the removed words. A line that has nothing left is dropped, and a
/// removal inside a line leaves one space.
fn rebuild(text: &str, toks: &[Tok], removed: &[bool]) -> String {
    const GONE: char = '\u{1}';
    let mut marked = String::with_capacity(text.len());
    let mut at = 0;
    let mut i = 0;
    while i < toks.len() {
        if removed[i] {
            let mut j = i;
            while j + 1 < toks.len() && removed[j + 1] {
                j += 1;
            }
            marked.push_str(&text[at..toks[i].start]);
            marked.push(GONE);
            at = toks[j].end;
            i = j + 1;
        } else {
            i += 1;
        }
    }
    marked.push_str(&text[at..]);

    let mut kept: Vec<String> = Vec::new();
    for line in marked.split('\n') {
        if !line.contains(GONE) {
            kept.push(line.to_owned());
            continue;
        }
        let mut s = String::new();
        let mut skip = false;
        let mut need_space = false;
        for c in line.chars() {
            if c == GONE {
                while s.ends_with([' ', '\t']) {
                    s.pop();
                }
                skip = true;
                need_space = !s.is_empty();
            } else if skip && matches!(c, ' ' | '\t') {
                continue;
            } else {
                if skip && need_space {
                    s.push(' ');
                }
                skip = false;
                s.push(c);
            }
        }
        let s = s.trim_end().to_owned();
        if !s.trim().is_empty() {
            kept.push(s);
        }
    }
    let mut joined = kept.join("\n");
    while joined.contains("\n\n\n") {
        joined = joined.replace("\n\n\n", "\n\n");
    }
    joined.trim_matches('\n').to_owned()
}

/// The normalized phrases of a list of running lines, for comparison with headings.
pub fn keys_of(lines: &[RunningLine]) -> BTreeSet<&str> {
    lines.iter().map(|l| l.key.as_str()).collect()
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
        assert_eq!(section.first_text, "SECTION 1.2 EXPONENTS");
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
    fn a_footer_is_found_at_the_bottom_and_goes_from_every_page() {
        let pages = doc(&[
            "Text one.\n\nOpenbook Course Notes",
            "Text two.\n\nOpenbook Course Notes",
            "Text three.\n\nOpenbook Course Notes",
        ]);
        let out = strip_running_lines(&pages);
        // A footer is not a heading, so the first page loses it too.
        assert_eq!(texts(&out), ["Text one.", "Text two.", "Text three."]);
    }

    #[test]
    fn bare_page_numbers_that_grow_are_removed() {
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
            "a dash-wrapped number is not a bare number and stays"
        );
        // Numbers that do not grow are text.
        let same = doc(&["One.\n\n7", "Two.\n\n7", "Three.\n\n7", "Four.\n\n7"]);
        assert_eq!(strip_running_lines(&same).pages, same);
    }

    #[test]
    fn a_numbered_label_is_not_a_running_header() {
        // "Try It #4" and "Try It #5" differ by a number inside the phrase, and "Try It" has too
        // few words.
        let pages = doc(&[
            "## Try It #1\n\nA.",
            "## Try It #2\n\nB.",
            "## Try It #3\n\nC.",
            "## Try It #4\n\nD.",
        ]);
        let out = strip_running_lines(&pages);
        assert_eq!(out.pages, pages);
        assert!(out.lines.is_empty());
        let plain = doc(&["## Try It\n\nA.", "## Try It\n\nB.", "## Try It\n\nC."]);
        assert_eq!(strip_running_lines(&plain).pages, plain);
    }

    #[test]
    fn short_phrases_and_phrases_with_math_are_never_running_lines() {
        for line in [
            "Solution",
            "Chapter Review",
            "$$a^2 + b^2 = c^2 + d^2$$",
            "`let x = compute value`",
        ] {
            let page = format!("{line}\n\nBody.");
            let pages = doc(&[&page, &page, &page, &page]);
            let out = strip_running_lines(&pages);
            assert_eq!(out.pages, pages, "{line}");
        }
    }

    fn filler(n: usize, tag: &str) -> String {
        (0..n)
            .map(|i| format!("{tag}{i}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn a_phrase_in_the_middle_of_a_page_is_left_alone() {
        let page = |n: usize| {
            format!(
                "{} GOING THROUGH THE MIDDLE {}",
                filler(60, &format!("a{n}x")),
                filler(60, &format!("b{n}y"))
            )
        };
        let pages: Vec<(u32, String)> = (1..=5).map(|n| (n as u32, page(n))).collect();
        assert_eq!(strip_running_lines(&pages).pages, pages);
    }

    #[test]
    fn a_common_phrase_at_the_edge_is_left_alone() {
        // "one of the most" starts every page, but it is all through the text as well.
        let page = |n: usize| {
            format!(
                "one of the most {} {} {}",
                filler(3, &format!("p{n}x")),
                "one of the most common ways",
                filler(70, &format!("q{n}y"))
            )
        };
        let mut pages: Vec<(u32, String)> = (1..=6).map(|n| (n as u32, page(n))).collect();
        for p in &mut pages {
            let middle = " one of the most common ways ".repeat(8);
            p.1 = format!(
                "{}{}{}",
                &p.1[..p.1.len() / 2],
                middle,
                &p.1[p.1.len() / 2..]
            );
        }
        assert_eq!(strip_running_lines(&pages).pages, pages);
    }

    #[test]
    fn a_line_must_repeat_on_enough_pages_and_often_enough() {
        // Two pages are too few.
        let few = doc(&[
            "THE RUNNING TITLE\n\nA.",
            "THE RUNNING TITLE\n\nB.",
            "C.",
            "D.",
        ]);
        assert_eq!(strip_running_lines(&few).pages, few);
        // Three pages far apart in a long document are too sparse (3 of 20 is 0.15).
        let mut long: Vec<String> = (0..20).map(|i| format!("Page {i} text.")).collect();
        for at in [0, 9, 19] {
            long[at] = format!("THE RUNNING TITLE\n\n{}", long[at]);
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
                (n, format!("{header}Text {n}a about {n}b and {n}c."))
            })
            .collect();
        let out = strip_running_lines(&pages);
        assert_eq!(out.lines.len(), 1);
        assert_eq!(out.lines[0].first_page, 41);
        assert_eq!(out.lines[0].removed_from.len(), 9);
        assert_eq!(out.pages[42].1, "Text 43a about 43b and 43c.");
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
            "THE RUNNING TITLE\n\nFirst.",
            "THE RUNNING TITLE",
            "THE RUNNING TITLE\n\nThird.",
            "THE RUNNING TITLE\n\nFourth.",
        ]);
        let out = strip_running_lines(&pages);
        assert_eq!(
            texts(&out),
            ["THE RUNNING TITLE\n\nFirst.", "", "Third.", "Fourth."]
        );
    }

    #[test]
    fn a_header_at_the_top_and_the_bottom_of_a_page_is_kept_once_on_its_first_page() {
        let pages = doc(&[
            "THE RUNNING TITLE\n\nOne.\n\nTHE RUNNING TITLE",
            "THE RUNNING TITLE\n\nTwo.\n\nTHE RUNNING TITLE",
            "THE RUNNING TITLE\n\nThree.\n\nTHE RUNNING TITLE",
        ]);
        let out = strip_running_lines(&pages);
        assert_eq!(texts(&out), ["THE RUNNING TITLE\n\nOne.", "Two.", "Three."]);
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

    /// Real pages of the second corpus (OpenStax American Government 2e, CC BY 4.0), as the text
    /// layer of the PDF gives them: one paragraph, with the header and the footer inside it. The
    /// header is at the start of one page and at the end of the next one.
    #[test]
    fn real_flat_pages_of_the_government_corpus() {
        let footer =
            "This OpenStax book is available for free at http://cnx.org/content/col26739/1.4";
        let pages = doc(&[
            &format!(
                "Chapter 2 | The Constitution and Its Origins 39 could take them away. The natural rights Locke described, however, had been granted by God and thus could never be abolished. {footer}"
            ),
            &format!(
                "colonies with a degree of benign neglect. Each colony had established its own legislature. Taxes imposed by England were low. 40 Chapter 2 | The Constitution and Its Origins {footer}"
            ),
            &format!(
                "Chapter 2 | The Constitution and Its Origins 41 Figure 2.3 The Sons of Liberty circulated this sensationalized version of the events of March 5, 1770. {footer}"
            ),
            &format!(
                "42 Chapter 2 | The Constitution and Its Origins Figure 2.4 Members of the modern Tea Party movement claim to represent the same spirit. {footer}"
            ),
            &format!(
                "Chapter 2 | The Constitution and Its Origins 43 Figure 2.5 The presentation of the Declaration of Independence is commemorated in a painting. {footer}"
            ),
        ]);
        let out = strip_running_lines(&pages);
        for (number, text) in &out.pages {
            assert!(!text.contains("OpenStax"), "page {number}: {text}");
        }
        // The first header stays at the top of the first page (the number next to it too).
        assert!(
            out.pages[0]
                .1
                .starts_with("Chapter 2 | The Constitution and Its Origins 39 could")
        );
        assert!(out.pages[0].1.ends_with("could never be abolished."));
        // The others lose the header and the page number next to it, and keep the text.
        assert_eq!(
            out.pages[1].1,
            "colonies with a degree of benign neglect. Each colony had established its own legislature. Taxes imposed by England were low."
        );
        assert!(
            out.pages[2].1.starts_with("Figure 2.3 The Sons"),
            "{}",
            out.pages[2].1
        );
        assert!(
            out.pages[3].1.starts_with("Figure 2.4 Members"),
            "{}",
            out.pages[3].1
        );
        assert!(
            out.pages[4].1.starts_with("Figure 2.5 The presentation"),
            "{}",
            out.pages[4].1
        );
        assert_eq!(out.lines.len(), 2);
    }

    #[test]
    fn the_key_of_a_heading_matches_the_key_of_a_phrase() {
        assert_eq!(
            phrase_key("# 126 CHAPTER 2 EQUATIONS AND INEQUALITIES"),
            "chapter 2 equations and inequalities"
        );
        assert_eq!(
            phrase_key("**Section 5.1 Quadratic Functions** 345"),
            "section 5.1 quadratic functions"
        );
        assert_eq!(phrase_key("Try It #4"), "try it #4");
    }
}
