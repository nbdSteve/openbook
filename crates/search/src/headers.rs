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
//! The rule removes text, so it is built to remove too little and not too much. A phrase is a
//! running phrase only when all of this holds:
//!
//! 1. The document has at least `MIN_DOCUMENT_PAGES` pages. A short document gives too little
//!    evidence to tell a header from content that repeats (a bullet on every step of a slide, an
//!    answer option on each page of a 3-page quiz, a table that goes on over 3 pages).
//! 2. The phrase starts at the first word of the page, or ends at the last word of the page. A page
//!    number (a number alone) and Markdown marks may stand outside it. So a phrase from the middle of a
//!    sentence is never cut. The pass runs twice, so that a header that is next to a footer (the
//!    footer is the last words of the page, and the header ends where the footer starts) is found
//!    in the second pass.
//! 3. It has `MIN_PHRASE_WORDS` to `MAX_PHRASE_WORDS` words, at most one fifth of the page (at least
//!    `MIN_PHRASE_WORDS`, at most `EDGE_WORDS`), in one line, and at least two words with letters.
//! 4. It is not a sentence: no word in it ends a sentence (`.`, `?`, `!`), and the last word does not
//!    either, unless the phrase holds a web address or a copyright sign (the licence line that
//!    OpenStax asks for on every page ends in an address and a full stop). It does not start with a
//!    list marker (`-`, `4.`, `(D)`), and no list marker stands in front of it.
//! 5. It is not in a Markdown table line, and not in a line with math (`$`) or code (a backtick).
//!    Such lines still count when the share of edge occurrences is computed.
//! 6. Words are compared after a normalization: case is ignored and Markdown marks (`#`, `*`, `_`)
//!    are dropped. A phrase does not start or end with a number, so the page number next to a header
//!    ("40 Chapter 2 | The Constitution", "The Constitution 41") is not part of it. A number inside a
//!    phrase stays, so "SECTION 1.2" and "SECTION 1.3", or "Try It #4" and "Try It #5", are not the
//!    same phrase.
//! 7. It is at the edge of at least `MIN_PAGES` pages, those pages are at least `MIN_DENSITY` of all
//!    the pages from its first to its last occurrence, and at least `MIN_EDGE_SHARE` of all its
//!    occurrences in the document are at an edge. The density is taken over the span and not over
//!    the whole document, because a chapter header repeats only inside its chapter: in a 300-page
//!    book it is on 10 percent of the pages, but on 50 percent of the pages of its chapter (every
//!    second page). The edge share keeps out the phrases that are common in the text.
//!
//! What happens to a running phrase:
//!
//! - When two phrases compete, the one with more pages wins, then the longer one. At each place the
//!   longest running phrase that covers the place is removed (the title that stands alone on one
//!   page is the end of the longer header of another page, and removing only the shorter one would
//!   leave a fragment).
//! - The first occurrence in the document stays when it is at the top of its page (it is often the
//!   real section heading), and the later ones are removed. An occurrence at the bottom of the first
//!   page is removed too: a footer is not a heading.
//! - A Markdown mark in front of a removed phrase goes with it. So does the page number that is next
//!   to it, but only one number on each side, and only when it fits the page numbers of the
//!   document: the number minus the page index is the same on most of the pages.
//! - A number alone on its line, at the start or at the end of a page, is a page number when the
//!   same holds. It is removed on every page, the first one included.
//!
//! A page that holds only a running phrase becomes empty, and the chunker skips an empty page.
//! The cost is quadratic in the number of pages (every candidate is looked up on every page):
//! 0.5 seconds for 500 pages, which is small next to transcription and embedding.

use std::collections::{BTreeMap, BTreeSet, HashMap};

/// How many words at most a phrase of a page edge can have (and the most words that are looked at).
pub const EDGE_WORDS: usize = 30;
/// A phrase has at least this many words (so "Try It" and "Chapter Review" are never removed).
pub const MIN_PHRASE_WORDS: usize = 3;
/// A longer phrase is text, not a header.
pub const MAX_PHRASE_WORDS: usize = 20;
/// The least number of pages that a running phrase must be on.
pub const MIN_PAGES: usize = 3;
/// A document with fewer pages is left as it is.
pub const MIN_DOCUMENT_PAGES: usize = 6;
/// The pages with the phrase, as a share of the pages from its first to its last occurrence.
pub const MIN_DENSITY: f64 = 0.3;
/// The share of all occurrences of a phrase that must be at an edge of a page.
pub const MIN_EDGE_SHARE: f64 = 0.8;
/// The share of the numbers next to a header that must fit the page numbers. The other numbers
/// are data in the text, so most of them do not fit, and the share is low.
const MIN_NUMBER_FIT: f64 = 0.4;

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

/// `-`, `*`, a bullet, `4.`, `d)`, `(D)`: the start of a list item.
fn is_list_marker(word: &str) -> bool {
    if matches!(word, "-" | "+" | "*" | "\u{2022}" | "\u{2013}" | "\u{2014}") {
        return true;
    }
    let inner = word.strip_prefix('(').unwrap_or(word);
    let Some(core) = inner.strip_suffix(')').or_else(|| inner.strip_suffix('.')) else {
        return false;
    };
    !core.is_empty() && core.chars().count() <= 3 && core.chars().all(char::is_alphanumeric)
}

fn ends_sentence(word: &str) -> bool {
    word.ends_with(['.', '?', '!'])
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
    /// Lines that give no phrase: math (`$`), code (a backtick), and table rows.
    skip_lines: BTreeSet<usize>,
    /// The position (in `seq`) of the first and of the last word that is not a number.
    first: Option<usize>,
    last: Option<usize>,
}

impl Page {
    fn new(number: u32, text: &str) -> Page {
        let toks = tokenize(text);
        let seq: Vec<usize> = (0..toks.len()).filter(|i| !toks[*i].is_mark()).collect();
        let mut skip_lines = BTreeSet::new();
        let mut line_start: Option<usize> = None;
        for t in &toks {
            if t.norm.contains(['$', '`']) {
                skip_lines.insert(t.line);
            }
            if line_start != Some(t.line) {
                line_start = Some(t.line);
                if t.norm.starts_with('|') {
                    skip_lines.insert(t.line);
                }
            }
        }
        let word = |p: &usize| !toks[seq[*p]].is_int;
        let first = (0..seq.len()).find(word);
        let last = (0..seq.len()).rev().find(word);
        Page {
            number,
            text: text.to_owned(),
            toks,
            seq,
            skip_lines,
            first,
            last,
        }
    }

    fn tok(&self, p: usize) -> &Tok {
        &self.toks[self.seq[p]]
    }

    /// How many words a phrase at an edge of this page can have.
    fn window(&self) -> usize {
        (self.seq.len() / 5).clamp(MIN_PHRASE_WORDS, EDGE_WORDS)
    }

    /// True when the words `start..start + len` begin at the first word of the page or end at the
    /// last one.
    fn anchored(&self, start: usize, len: usize) -> bool {
        self.first == Some(start) || self.last == Some(start + len - 1)
    }

    fn same_line(&self, start: usize, len: usize) -> bool {
        self.tok(start).line == self.tok(start + len - 1).line
    }

    fn skipped(&self, start: usize) -> bool {
        self.skip_lines.contains(&self.tok(start).line)
    }

    fn after_marker(&self, start: usize) -> bool {
        start > 0 && is_list_marker(&self.tok(start - 1).norm)
    }

    /// The rules about the words of a phrase (rules 3 to 5), without the counting.
    fn phrase_ok(&self, start: usize, len: usize) -> bool {
        if len < MIN_PHRASE_WORDS || start + len > self.seq.len() || !self.same_line(start, len) {
            return false;
        }
        if self.skipped(start) || self.after_marker(start) {
            return false;
        }
        let words: Vec<&Tok> = (start..start + len).map(|p| self.tok(p)).collect();
        let alnum = |w: &Tok| w.norm.chars().any(char::is_alphanumeric);
        if words[0].is_int
            || words[len - 1].is_int
            || !alnum(words[0])
            || !alnum(words[len - 1])
            || is_list_marker(&words[0].norm)
        {
            return false;
        }
        if words[..len - 1].iter().any(|w| ends_sentence(&w.norm)) {
            return false;
        }
        if ends_sentence(&words[len - 1].norm)
            && !words.iter().any(|w| {
                w.norm.contains("://") || w.norm.contains("www.") || w.norm.contains('\u{a9}')
            })
        {
            return false;
        }
        words
            .iter()
            .filter(|w| w.norm.chars().any(char::is_alphabetic))
            .count()
            >= 2
    }

    fn phrase_hash(&self, start: usize, len: usize) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for p in start..start + len {
            hash = (hash ^ self.tok(p).hash).wrapping_mul(0x0100_0000_01b3);
        }
        hash ^ (len as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
    }
}

/// A run of words that could be a running phrase.
struct Candidate {
    key: u64,
    len: usize,
    /// Positions (into the page list) of the pages that have it at an edge.
    pages: Vec<usize>,
    /// Where one occurrence is: (page, position in `seq`).
    example: (usize, usize),
}

fn candidates(pages: &[Page]) -> HashMap<u64, Candidate> {
    let mut map: HashMap<u64, Candidate> = HashMap::new();
    for (pi, page) in pages.iter().enumerate() {
        let max = page.window().min(MAX_PHRASE_WORDS);
        let mut spots: Vec<(usize, usize)> = Vec::new();
        if let Some(first) = page.first {
            for len in MIN_PHRASE_WORDS..=max {
                spots.push((first, len));
            }
        }
        if let Some(last) = page.last {
            for len in MIN_PHRASE_WORDS..=max {
                if last + 1 >= len {
                    spots.push((last + 1 - len, len));
                }
            }
        }
        for (start, len) in spots {
            if !page.phrase_ok(start, len) {
                continue;
            }
            let key = page.phrase_hash(start, len);
            let entry = map.entry(key).or_insert_with(|| Candidate {
                key,
                len,
                pages: Vec::new(),
                example: (pi, start),
            });
            if entry.pages.last() != Some(&pi) {
                entry.pages.push(pi);
            }
        }
    }
    map
}

/// A phrase that always has the same word after it (at the top of the page) or in front of it (at
/// the bottom) is the start or the end of something longer, a sentence for example, and not a
/// header. The longer phrase is a candidate on its own.
fn closed(pages: &[Page], cand: &Candidate) -> bool {
    let mut around: HashMap<&str, usize> = HashMap::new();
    let mut seen = 0usize;
    for &pi in &cand.pages {
        let page = &pages[pi];
        let mut spots: Vec<(usize, Option<usize>)> = Vec::new();
        if let Some(first) = page.first {
            spots.push((first, Some(first + cand.len)));
        }
        if let Some(last) = page.last
            && last + 1 >= cand.len
        {
            spots.push((last + 1 - cand.len, (last + 1 - cand.len).checked_sub(1)));
        }
        for (start, neighbour) in spots {
            if start + cand.len > page.seq.len() || page.phrase_hash(start, cand.len) != cand.key {
                continue;
            }
            seen += 1;
            if let Some(t) = neighbour.and_then(|p| page.seq.get(p)) {
                *around.entry(page.toks[*t].norm.as_str()).or_default() += 1;
            }
        }
    }
    let most = around.values().copied().max().unwrap_or(0);
    !(most >= MIN_PAGES && most as f64 >= MIN_EDGE_SHARE * seen as f64)
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

/// What happens to the first occurrence of a running phrase.
///
/// Round 2 compared the two (`evals/results/2026-11-18.md`) and production uses `Keep`: it keeps one
/// copy of a phrase that may be a real heading, so it is the safer choice when the data cannot tell
/// the two apart (the gate accuracy is a little higher with it, by one question of 107). `Drop` stays
/// only so that the evaluation grid can show the comparison. Nothing in the ingestion worker or the
/// API uses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FirstOccurrence {
    /// It stays when it is at the top of its page (it may be the real section heading).
    Keep,
    /// It goes with the others.
    Drop,
}

impl FirstOccurrence {
    pub const ALL: [FirstOccurrence; 2] = [FirstOccurrence::Keep, FirstOccurrence::Drop];

    pub fn name(self) -> &'static str {
        match self {
            FirstOccurrence::Keep => "keep-first",
            FirstOccurrence::Drop => "drop-all",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.name() == text)
    }
}

/// The variant of production. Do not change it without a new comparison in the grid.
pub const PRODUCTION_VARIANT: FirstOccurrence = FirstOccurrence::Keep;

/// Removes the running headers and footers of a document. `pages` has `(page number, Markdown)` in
/// page order. A page that has nothing to remove comes back unchanged, byte for byte.
pub fn strip_running_lines(pages: &[(u32, String)]) -> Stripped {
    strip_running_lines_with(pages, PRODUCTION_VARIANT)
}

/// `strip_running_lines` with a choice for the first occurrence. The evaluation compares both.
pub fn strip_running_lines_with(pages: &[(u32, String)], variant: FirstOccurrence) -> Stripped {
    if pages.len() < MIN_DOCUMENT_PAGES {
        return Stripped {
            pages: pages.to_vec(),
            lines: Vec::new(),
        };
    }
    // Two passes: a header that is next to a footer ends at the last word of the page only when the
    // footer is gone.
    let first = strip_pass(pages, variant);
    let second = strip_pass(&first.pages, variant);
    let mut lines: Vec<RunningLine> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for line in first.lines.into_iter().chain(second.lines) {
        match index.get(&line.key) {
            Some(at) if !line.key.is_empty() => {
                let known = &mut lines[*at];
                for page in line.removed_from {
                    if !known.removed_from.contains(&page) {
                        known.removed_from.push(page);
                    }
                }
                known.first_page = known.first_page.min(line.first_page);
            }
            _ => {
                index.insert(line.key.clone(), lines.len());
                lines.push(line);
            }
        }
    }
    lines.sort_by(|a, b| a.first_page.cmp(&b.first_page).then(a.key.cmp(&b.key)));
    Stripped {
        pages: second.pages,
        lines,
    }
}

/// One occurrence of a running phrase, with the page numbers that stand next to it.
struct Hit {
    page: usize,
    /// Positions in `seq`, inclusive.
    from: usize,
    to: usize,
    kept: bool,
}

fn strip_pass(pages: &[(u32, String)], variant: FirstOccurrence) -> Stripped {
    let pages_t: Vec<Page> = pages.iter().map(|(n, t)| Page::new(*n, t)).collect();
    let all = candidates(&pages_t);
    let mut ranked: Vec<&Candidate> = all
        .values()
        .filter(|c| density_ok(&c.pages) && closed(&pages_t, c))
        .collect();
    // More pages first, then the longer phrase, then the earlier one (so the order never changes).
    ranked.sort_by(|a, b| {
        b.pages
            .len()
            .cmp(&a.pages.len())
            .then(b.len.cmp(&a.len))
            .then(a.example.cmp(&b.example))
    });
    let known: HashMap<u64, usize> = ranked.iter().map(|c| (c.key, c.len)).collect();

    // removed[page][index into toks]
    let mut removed: Vec<Vec<bool>> = pages_t.iter().map(|p| vec![false; p.toks.len()]).collect();
    // consumed[page][position in seq]: taken by a phrase, kept or removed
    let mut consumed: Vec<Vec<bool>> = pages_t.iter().map(|p| vec![false; p.seq.len()]).collect();
    let mut found: BTreeMap<String, RunningLine> = BTreeMap::new();
    let mut hits: Vec<Hit> = Vec::new();

    for cand in ranked {
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
                let anchored = page.anchored(start, cand.len);
                at_edge += usize::from(anchored);
                let free = (start..start + cand.len).all(|p| !consumed[pi][p]);
                if anchored && free && page.phrase_ok(start, cand.len) {
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
        for (pi, start) in edge {
            let page = &pages_t[pi];
            // The longest running phrase that covers this one.
            let (mut from, mut to) = (start, start + cand.len - 1);
            'grow: for total_len in (cand.len + 1..=MAX_PHRASE_WORDS).rev() {
                for left in 0..=(total_len - cand.len) {
                    let right = total_len - cand.len - left;
                    if start < left || start + cand.len + right > page.seq.len() {
                        continue;
                    }
                    let (s, e) = (start - left, start + cand.len + right - 1);
                    if known.get(&page.phrase_hash(s, total_len)) == Some(&total_len)
                        && page.same_line(s, total_len)
                        && (s..=e)
                            .all(|p| !consumed[pi][p] || (start..start + cand.len).contains(&p))
                    {
                        (from, to) = (s, e);
                        break 'grow;
                    }
                }
            }
            consumed[pi][from..=to].fill(true);
            let key = (from..=to)
                .map(|p| page.tok(p).norm.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            let text_of = page.text[page.tok(from).start..page.tok(to).end].to_owned();
            let entry = found.entry(key.clone()).or_insert_with(|| RunningLine {
                key,
                first_text: text_of,
                first_page: pages_t[first_page].number,
                removed_from: Vec::new(),
            });
            // At the top of its page: nearer to the start of the page than to its end.
            let at_top = from <= page.seq.len() - (to + 1);
            let keep =
                variant == FirstOccurrence::Keep && pi == first_page && at_top && !kept_first;
            if keep {
                kept_first = true;
            } else {
                for p in from..=to {
                    removed[pi][page.seq[p]] = true;
                }
                // Heading marks in front of the phrase go with it.
                remove_marks_before(page, &mut removed[pi], page.seq[from]);
                if entry.removed_from.last() != Some(&page.number) {
                    entry.removed_from.push(page.number);
                }
            }
            hits.push(Hit {
                page: pi,
                from,
                to,
                kept: keep,
            });
        }
    }

    remove_page_numbers_next_to_phrases(&pages_t, &hits, &mut removed);
    let mut lines: Vec<RunningLine> = found
        .into_values()
        .filter(|l| !l.removed_from.is_empty())
        .collect();
    remove_bare_page_numbers(&pages_t, &mut removed, &mut lines);

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

fn remove_marks_before(page: &Page, removed: &mut [bool], mut t: usize) {
    while t > 0 && page.toks[t - 1].is_mark() {
        t -= 1;
        removed[t] = true;
    }
}

/// The most common value of `number - page index`, and how many numbers have it.
fn modal_offset(numbers: &[(usize, i64)]) -> Option<(i64, usize)> {
    let mut counts: BTreeMap<i64, usize> = BTreeMap::new();
    for (pi, value) in numbers {
        *counts.entry(value - *pi as i64).or_default() += 1;
    }
    counts.into_iter().max_by_key(|(_, n)| *n)
}

/// A page number that stands next to a removed phrase goes with it: one number on each side, and
/// only when it fits the page numbers of the document (the number minus the page index is the same
/// on most pages).
fn remove_page_numbers_next_to_phrases(pages: &[Page], hits: &[Hit], removed: &mut [Vec<bool>]) {
    // (page, position in seq, value, hit index)
    let mut next_to: Vec<(usize, usize, i64, usize)> = Vec::new();
    for (h, hit) in hits.iter().enumerate() {
        let page = &pages[hit.page];
        let ints_only =
            |range: std::ops::Range<usize>| range.into_iter().all(|p| page.tok(p).is_int);
        // At the top of the page the number can be in front of the phrase or behind it, and the
        // same holds at the bottom. A number in the middle of the text is never looked at.
        let at_edge = ints_only(0..hit.from) || ints_only(hit.to + 1..page.seq.len());
        if !at_edge {
            continue;
        }
        let mut near: Vec<usize> = Vec::new();
        if hit.from > 0 {
            near.push(hit.from - 1);
        }
        if hit.to + 1 < page.seq.len() {
            near.push(hit.to + 1);
        }
        for p in near {
            if page.tok(p).is_int
                && let Ok(v) = page.tok(p).norm.parse::<i64>()
            {
                next_to.push((hit.page, p, v, h));
            }
        }
    }
    let numbers: Vec<(usize, i64)> = next_to.iter().map(|n| (n.0, n.2)).collect();
    let Some((offset, count)) = modal_offset(&numbers) else {
        return;
    };
    if count < MIN_PAGES || (count as f64) < MIN_NUMBER_FIT * numbers.len() as f64 {
        return;
    }
    for (pi, p, value, h) in next_to {
        if value - pi as i64 == offset && !hits[h].kept {
            let t = pages[pi].seq[p];
            removed[pi][t] = true;
            remove_marks_before(&pages[pi], &mut removed[pi], t);
        }
    }
}

/// A number alone on its line at the start or the end of a page is a page number when the number
/// minus the page index is the same on most of those pages.
fn remove_bare_page_numbers(
    pages: &[Page],
    removed: &mut [Vec<bool>],
    lines: &mut Vec<RunningLine>,
) {
    for from_end in [false, true] {
        let mut found: Vec<(usize, usize, i64)> = Vec::new();
        for (pi, page) in pages.iter().enumerate() {
            let alive: Vec<usize> = (0..page.toks.len()).filter(|i| !removed[pi][*i]).collect();
            let pick = if from_end {
                alive.last()
            } else {
                alive.first()
            };
            let Some(&i) = pick else { continue };
            let tok = &page.toks[i];
            let alone = page.toks.iter().filter(|t| t.line == tok.line).count() == 1;
            if tok.is_int
                && alone
                && let Ok(value) = tok.norm.parse::<i64>()
            {
                found.push((pi, i, value));
            }
        }
        let numbers: Vec<(usize, i64)> = found.iter().map(|f| (f.0, f.2)).collect();
        let Some((offset, _)) = modal_offset(&numbers) else {
            continue;
        };
        let fits: Vec<&(usize, usize, i64)> = found
            .iter()
            .filter(|f| f.2 - f.0 as i64 == offset)
            .collect();
        let positions: Vec<usize> = fits.iter().map(|f| f.0).collect();
        if !density_ok(&positions) || (fits.len() as f64) < 0.8 * found.len() as f64 {
            continue;
        }
        let (fp, fi, _) = *fits[0];
        let first_text =
            pages[fp].text[pages[fp].toks[fi].start..pages[fp].toks[fi].end].to_owned();
        for (pi, i, _) in &fits {
            removed[*pi][*i] = true;
        }
        lines.push(RunningLine {
            key: String::new(),
            first_text,
            first_page: pages[fp].number,
            removed_from: fits.iter().map(|f| pages[f.0].number).collect(),
        });
    }
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

    /// A document of `n` pages, page `i` (from 0) made by `f`.
    fn make(n: usize, f: impl Fn(usize) -> String) -> Vec<(u32, String)> {
        (0..n)
            .map(|i| (u32::try_from(i + 1).unwrap(), f(i)))
            .collect()
    }

    /// 60 words that only page `i` has.
    fn body(i: usize) -> String {
        (0..60)
            .map(|k| format!("w{i}x{k}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn unchanged(pages: &[(u32, String)]) {
        let out = strip_running_lines(pages);
        assert_eq!(out.pages, pages, "{:?}", out.lines);
        assert!(out.lines.is_empty());
    }

    #[test]
    fn a_header_on_every_other_page_is_removed_and_the_first_one_stays() {
        let pages = make(8, |i| {
            let title = if i % 2 == 0 {
                "# SECTION 1.2 EXPONENTS"
            } else {
                "# CHAPTER 1 PREREQUISITES"
            };
            format!("{title}\n\n{}", body(i))
        });
        let out = strip_running_lines(&pages);
        assert_eq!(out.pages[0], pages[0]);
        assert_eq!(out.pages[1], pages[1]);
        for i in 2..8 {
            assert_eq!(out.pages[i].1, body(i), "page {}", i + 1);
        }
        assert_eq!(out.lines.len(), 2);
        let section = out
            .lines
            .iter()
            .find(|l| l.key.starts_with("section"))
            .unwrap();
        assert_eq!(section.first_page, 1);
        assert_eq!(section.removed_from, [3, 5, 7]);
        assert_eq!(section.first_text, "SECTION 1.2 EXPONENTS");
    }

    #[test]
    fn the_drop_all_variant_removes_the_first_occurrence_too() {
        let pages = make(8, |i| format!("# THE RUNNING TITLE\n\n{}", body(i)));
        let out = strip_running_lines_with(&pages, FirstOccurrence::Drop);
        for (i, (_, text)) in out.pages.iter().enumerate() {
            assert_eq!(text, &body(i));
        }
        assert_eq!(out.lines[0].removed_from.len(), 8);
        // The default is keep-first.
        assert_eq!(
            strip_running_lines(&pages),
            strip_running_lines_with(&pages, FirstOccurrence::Keep)
        );
        assert_eq!(strip_running_lines(&pages).pages[0], pages[0]);
        assert_eq!(
            FirstOccurrence::parse("drop-all"),
            Some(FirstOccurrence::Drop)
        );
        assert_eq!(FirstOccurrence::parse("x"), None);
    }

    #[test]
    fn case_marks_and_page_numbers_do_not_make_lines_different() {
        let pages = make(8, |i| {
            let n = 343 + i;
            let head = match i % 4 {
                0 => format!("SECTION 5.1 QUADRATIC FUNCTIONS {n}"),
                1 => format!("# {n} Section 5.1 Quadratic Functions"),
                2 => format!("**Section 5.1 Quadratic Functions** {n}"),
                _ => "## SECTION 5.1 QUADRATIC FUNCTIONS".to_owned(),
            };
            format!("{head}\n\n{}", body(i))
        });
        let out = strip_running_lines(&pages);
        assert_eq!(out.pages[0], pages[0]);
        for i in 1..8 {
            assert_eq!(out.pages[i].1, body(i), "page {}", i + 1);
        }
    }

    #[test]
    fn a_footer_is_found_at_the_bottom_and_goes_from_every_page() {
        let pages = make(6, |i| format!("{}\n\nOpenbook Course Notes", body(i)));
        let out = strip_running_lines(&pages);
        // A footer is not a heading, so the first page loses it too.
        for i in 0..6 {
            assert_eq!(out.pages[i].1, body(i));
        }
    }

    #[test]
    fn a_footer_next_to_a_header_goes_with_it_and_so_does_the_page_number() {
        let footer =
            "This OpenStax book is available for free at http://cnx.org/content/col26739/1.4";
        let pages = make(8, |i| {
            if i % 2 == 0 {
                format!(
                    "Chapter 2 | The Constitution and Its Origins {} {} {footer}",
                    39 + i,
                    body(i)
                )
            } else {
                format!(
                    "{} {} Chapter 2 | The Constitution and Its Origins {footer}",
                    body(i),
                    39 + i
                )
            }
        });
        let out = strip_running_lines(&pages);
        for i in 0..8 {
            assert!(!out.pages[i].1.contains("OpenStax"), "page {}", i + 1);
        }
        // The first header stays at the top of the first page, with the number next to it.
        assert_eq!(
            out.pages[0].1,
            format!(
                "Chapter 2 | The Constitution and Its Origins 39 {}",
                body(0)
            )
        );
        for i in 1..8 {
            assert_eq!(out.pages[i].1, body(i), "page {}", i + 1);
        }
    }

    #[test]
    fn bare_page_numbers_that_fit_are_removed_and_other_numbers_stay() {
        let pages = make(8, |i| format!("{}\n\n{}", body(i), i + 12));
        let out = strip_running_lines(&pages);
        for i in 0..8 {
            assert_eq!(out.pages[i].1, body(i));
        }
        // The same number on every page is not a page number.
        unchanged(&make(8, |i| format!("{}\n\n7", body(i))));
        // Numbers that grow but do not follow the pages are not page numbers.
        let jumpy = [3, 4, 9, 10, 15, 16, 23, 30];
        unchanged(&make(8, |i| format!("{}\n\n{}", body(i), jumpy[i])));
    }

    #[test]
    fn numbers_that_start_a_question_or_a_section_stay() {
        // "1 (a) Define ...", "2 (a) ...": the number grows with the page, but it is not alone.
        unchanged(&make(8, |i| {
            format!(
                "{} (a) Define the term {}\n\n{}",
                i + 1,
                body(i),
                body(i + 20)
            )
        }));
        // "12 Definitions ...", "13 ..." in a law.
        unchanged(&make(8, |i| format!("{} Definitions {}", 12 + i, body(i))));
    }

    #[test]
    fn only_one_number_next_to_a_header_goes() {
        let pages = make(8, |i| {
            format!(
                "{} 12 15 18 {} Course Notes for Statistics {}",
                body(i),
                6 * i + 3,
                101 + i
            )
        });
        let out = strip_running_lines(&pages);
        for i in 0..8 {
            assert_eq!(
                out.pages[i].1,
                format!("{} 12 15 18 {}", body(i), 6 * i + 3),
                "page {}",
                i + 1
            );
        }
        // A number that does not fit the page numbers stays.
        let off = make(8, |i| {
            format!(
                "{} Course Notes for Statistics {}",
                body(i),
                [5, 90, 17, 4, 66, 8, 120, 31][i]
            )
        });
        let out = strip_running_lines(&off);
        assert!(out.pages[3].1.ends_with("w3x59 4"), "{}", out.pages[3].1);
    }

    #[test]
    fn a_numbered_label_is_not_a_running_header() {
        // "Try It #4" and "Try It #5" differ by a number inside the phrase, and "Try It" has too
        // few words.
        unchanged(&make(8, |i| format!("## Try It #{i}\n\n{}", body(i))));
        unchanged(&make(8, |i| format!("## Try It\n\n{}", body(i))));
    }

    #[test]
    fn math_code_and_table_rows_are_never_removed() {
        for line in [
            "$$a^2 + b^2 = c^2 + d^2$$",
            "`let x = compute value`",
            "| Country | Capital | Population |",
        ] {
            unchanged(&make(8, |i| format!("{line}\n\n{}", body(i))));
            unchanged(&make(8, |i| format!("{}\n\n{line}", body(i))));
        }
        // A table that goes on over pages keeps its header row, page 1 included.
        unchanged(&make(8, |i| {
            format!(
                "| Country | Capital | Population |\n| --- | --- | --- |\n| c{i} | d{i} | {i} |"
            )
        }));
    }

    #[test]
    fn content_that_repeats_is_not_a_header() {
        // A bullet on every step of a slide (Markdown, and as the text layer gives it).
        let bullet = "Initialise every distance to infinity and the source to zero.";
        unchanged(&make(8, |i| {
            format!("- {bullet}\n- step {}\n\n{}", body(i), body(i + 20))
        }));
        unchanged(&make(8, |i| format!("{bullet} {}", body(i))));
        // A task from a worksheet, in the middle or at the end of the page.
        let task = "4. Use the chain rule to find the derivative of k(x) = (2x+1)^5.";
        unchanged(&make(8, |i| format!("{}\n\n{task}", body(i))));
        unchanged(&make(8, |i| format!("{task} {}", body(i))));
        unchanged(&make(8, |i| format!("{} {task} {}", body(i), body(i + 20))));
        // A proof, and a sentence that starts after a bold word.
        let proof = "**Proof.** Let g be any element of G and consider its order.";
        unchanged(&make(8, |i| format!("{proof} {}", body(i))));
        unchanged(&make(8, |i| format!("{} {proof}", body(i))));
        // The last option of every question of a quiz, one question on a page.
        unchanged(&make(8, |i| {
            format!(
                "Question {i}: {}\n(A) first\n(B) second\n(C) third\n(D) None of the above",
                body(i)
            )
        }));
        unchanged(&make(8, |i| format!("{} (D) None of the above", body(i))));
    }

    #[test]
    fn a_phrase_in_the_middle_of_a_page_is_left_alone() {
        let page = |n: usize| {
            format!(
                "{} GOING THROUGH THE MIDDLE {}",
                (0..60)
                    .map(|k| format!("a{n}x{k}"))
                    .collect::<Vec<_>>()
                    .join(" "),
                (0..60)
                    .map(|k| format!("b{n}y{k}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        };
        unchanged(&make(8, page));
    }

    #[test]
    fn a_short_document_is_left_alone() {
        // The same header on every page of a 5-page document: not enough evidence.
        let pages = make(5, |i| format!("# THE RUNNING TITLE\n\n{}", body(i)));
        unchanged(&pages);
        // And at 6 pages it is a header.
        let six = make(6, |i| format!("# THE RUNNING TITLE\n\n{}", body(i)));
        assert_eq!(strip_running_lines(&six).pages[3].1, body(3));
        assert_eq!(strip_running_lines(&[]).pages, Vec::new());
    }

    #[test]
    fn a_line_must_repeat_on_enough_pages_and_often_enough() {
        // Two pages are too few.
        unchanged(&make(8, |i| {
            if i < 2 {
                format!("THE RUNNING TITLE\n\n{}", body(i))
            } else {
                body(i)
            }
        }));
        // Three pages far apart in a long document are too sparse (3 of 20 is 0.15).
        unchanged(&make(20, |i| {
            if [0, 9, 19].contains(&i) {
                format!("THE RUNNING TITLE\n\n{}", body(i))
            } else {
                body(i)
            }
        }));
    }

    #[test]
    fn a_header_that_covers_one_chapter_of_a_long_book_is_found() {
        // A chapter header on every second page of pages 41 to 60 of a 100-page book: 10 of 100
        // pages, but 10 of 20 pages of its span.
        let pages = make(100, |i| {
            let n = i + 1;
            if (41..=60).contains(&n) && n % 2 == 1 {
                format!("CHAPTER 4 GEOMETRY\n\n{}", body(i))
            } else {
                body(i)
            }
        });
        let out = strip_running_lines(&pages);
        assert_eq!(out.lines.len(), 1);
        assert_eq!(out.lines[0].first_page, 41);
        assert_eq!(out.lines[0].removed_from.len(), 9);
        assert_eq!(out.pages[42].1, body(42));
        assert!(out.pages[40].1.starts_with("CHAPTER 4"));
    }

    #[test]
    fn a_page_that_is_only_a_header_becomes_empty() {
        let pages = make(8, |i| {
            if i == 1 {
                "THE RUNNING TITLE".to_owned()
            } else {
                format!("THE RUNNING TITLE\n\n{}", body(i))
            }
        });
        let out = strip_running_lines(&pages);
        assert_eq!(out.pages[0], pages[0]);
        assert_eq!(out.pages[1].1, "");
        assert_eq!(out.pages[2].1, body(2));
    }

    #[test]
    fn a_header_at_the_top_and_the_bottom_of_a_page_is_kept_once_on_its_first_page() {
        let pages = make(8, |i| {
            format!("THE RUNNING TITLE\n\n{}\n\nTHE RUNNING TITLE", body(i))
        });
        let out = strip_running_lines(&pages);
        assert_eq!(out.pages[0].1, format!("THE RUNNING TITLE\n\n{}", body(0)));
        for i in 1..8 {
            assert_eq!(out.pages[i].1, body(i));
        }
    }

    #[test]
    fn a_page_without_a_running_line_comes_back_byte_for_byte() {
        let pages = make(8, |i| {
            if i == 1 {
                format!("{}\n", body(i))
            } else {
                format!("SECTION 2 TITLE\n\n{}\n", body(i))
            }
        });
        let out = strip_running_lines(&pages);
        assert_eq!(out.pages[0], pages[0]);
        assert_eq!(out.pages[1], pages[1]);
        assert_eq!(out.pages[2].1, body(2));
    }

    #[test]
    fn a_title_that_stands_alone_and_the_end_of_a_longer_header_leave_no_fragment() {
        // Pages of type A have the title alone. Pages of type B have a longer header that ends with
        // the title. Pages of type C have only the chapter. The title has the most pages, so it is
        // picked first, and the longest header that covers each place must be the one that goes.
        let pages = make(12, |i| match i % 3 {
            0 => format!("# SECTION 5.1 QUADRATIC FUNCTIONS\n\n{}", body(i)),
            1 => {
                format!(
                    "{} CHAPTER 5 Polynomial and Rational Functions 5.1 Quadratic Functions",
                    344 + i
                ) + &format!("\n\n{}", body(i))
            }
            _ => format!(
                "# CHAPTER 5 POLYNOMIAL AND RATIONAL FUNCTIONS\n\n{}",
                body(i)
            ),
        });
        let out = strip_running_lines(&pages);
        for i in 3..12 {
            assert_eq!(out.pages[i].1, body(i), "page {}", i + 1);
        }
        for line in &out.lines {
            assert!(line.key != "5.1 quadratic functions", "{:?}", out.lines);
        }
    }

    /// The start of real pages of the first evaluation corpus (OpenStax College Algebra, CC BY 4.0,
    /// section 1.2, pages 1 to 8 of the excerpt), as the ingestion made them, each with a filler
    /// paragraph for the rest of the page.
    #[test]
    fn real_pages_of_the_algebra_corpus() {
        let heads = [
            "# SECTION 1.2 EXPONENTS AND SCIENTIFIC NOTATION\n\n## LEARNING OBJECTIVES\n\nIn this section students will:\n- Use the product rule of exponents.",
            "# CHAPTER 1 PREREQUISITES\n\n## Example 1 Using the Product Rule\n\nWrite each of the following products with a single base. Do not simplify further.",
            "# SECTION 1.2 EXPONENTS AND SCIENTIFIC NOTATION\n\nSolution Use the quotient rule to simplify each expression.",
            "# CHAPTER 1 PREREQUISITES\n\n## Try # 3\n\nWrite each of the following products with a single base. Do not simplify further.",
            "# SECTION 1.2 EXPONENTS AND SCIENTIFIC NOTATION\n\n## Try It #4",
            "# CHAPTER 1 PREREQUISITES\n\n## Try It #5",
            "# SECTION 1.2 EXPONENTS AND SCIENTIFIC NOTATION\n\n## Try It #7",
            "# CHAPTER 1 PREREQUISITES\n\n## Try It #2\n\nThe quotient rule and reduce fraction\n\nSimplify.",
        ];
        let pages = make(8, |i| format!("{}\n\n{}", heads[i], body(i)));
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

    /// Pages of the second corpus (OpenStax American Government 2e, CC BY 4.0) as the text layer of
    /// the PDF gives them: one paragraph, with the header and the footer inside it. The header is at
    /// the start of one page and at the end of the next one, next to the footer.
    #[test]
    fn real_flat_pages_of_the_government_corpus() {
        let footer =
            "This OpenStax book is available for free at http://cnx.org/content/col26739/1.4";
        let text = "could take them away. The natural rights Locke described, however, had been granted by God and thus could never be abolished by human beings or by the institutions that they created";
        let pages = make(8, |i| {
            if i % 2 == 0 {
                format!(
                    "Chapter 2 | The Constitution and Its Origins {} {text} 77 {footer}",
                    39 + i
                )
            } else {
                format!(
                    "{text} 77 {} Chapter 2 | The Constitution and Its Origins {footer}",
                    39 + i
                )
            }
        });
        let out = strip_running_lines(&pages);
        for (number, page) in &out.pages {
            assert!(!page.contains("OpenStax"), "page {number}: {page}");
        }
        assert_eq!(
            out.pages[0].1,
            format!("Chapter 2 | The Constitution and Its Origins 39 {text} 77")
        );
        for i in 1..8 {
            assert_eq!(out.pages[i].1, format!("{text} 77"), "page {}", i + 1);
        }
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
