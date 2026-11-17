//! The Markdown chunker.
//!
//! It splits page Markdown at headings, then packs blocks to about 400 tokens (1 token is about
//! 4 characters) with at most 600 tokens, and one or two sentences of overlap between chunks that
//! the size limit separated. It never splits inside a `$$...$$` block, a fenced code block, a table,
//! or a list item. Each chunk keeps its index, its first and last page, and its heading path.

/// Target chunk size: 400 tokens.
pub const TARGET_CHARS: usize = 1600;
/// Hard limit for text that the chunker can split: 600 tokens.
pub const MAX_CHARS: usize = 2400;
/// The overlap is at most this long.
pub const MAX_OVERLAP_CHARS: usize = 300;
const OVERLAP_SENTENCES: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// 0-based position in the document.
    pub index: u32,
    /// 1-based pages.
    pub first_page: u32,
    pub last_page: u32,
    /// Headings above the chunk, joined with ` > `. Empty before the first heading.
    pub heading_path: String,
    pub text: String,
}

/// The text that is embedded and indexed: `<title> > <heading path>`, a newline, then the chunk text.
/// Students see only `chunk.text`.
pub fn embed_text(title: &str, chunk: &Chunk) -> String {
    if chunk.heading_path.is_empty() {
        format!("{title}\n{}", chunk.text)
    } else {
        format!("{title} > {}\n{}", chunk.heading_path, chunk.text)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Heading(usize),
    Paragraph,
    ListItem,
    /// Math, code, or table: never split.
    Atomic,
}

#[derive(Debug, Clone)]
struct Block {
    kind: Kind,
    text: String,
    page: u32,
}

fn heading_level(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) {
        let rest = &trimmed[hashes..];
        if rest.starts_with(' ') || rest.starts_with('\t') {
            let text = rest.trim();
            if !text.is_empty() {
                return Some((hashes, text));
            }
        }
    }
    None
}

fn is_fence(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("```") || t.starts_with("~~~")
}

fn is_table_line(line: &str) -> bool {
    line.trim_start().starts_with('|')
}

fn is_list_start(line: &str) -> bool {
    let t = line.trim_start();
    let mut chars = t.chars();
    match chars.next() {
        Some('-' | '*' | '+') => chars.next().is_some_and(|c| c == ' ' || c == '\t'),
        Some(c) if c.is_ascii_digit() => {
            let rest = t.trim_start_matches(|c: char| c.is_ascii_digit());
            let mut r = rest.chars();
            matches!(r.next(), Some('.' | ')')) && r.next().is_some_and(|c| c == ' ' || c == '\t')
        }
        _ => false,
    }
}

fn is_block_start(line: &str) -> bool {
    heading_level(line).is_some()
        || is_fence(line)
        || line.trim_start().starts_with("$$")
        || is_table_line(line)
        || is_list_start(line)
}

/// Splits one page of Markdown into blocks.
fn parse_blocks(markdown: &str, page: u32) -> Vec<Block> {
    let lines: Vec<&str> = markdown.lines().collect();
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        if let Some((level, text)) = heading_level(line) {
            blocks.push(Block {
                kind: Kind::Heading(level),
                text: text.to_owned(),
                page,
            });
            i += 1;
        } else if is_fence(line) {
            let fence: String = line.trim_start().chars().take(3).collect();
            let mut end = i + 1;
            while end < lines.len() && !lines[end].trim_start().starts_with(&fence) {
                end += 1;
            }
            let last = end.min(lines.len() - 1);
            blocks.push(Block {
                kind: Kind::Atomic,
                text: lines[i..=last].join("\n"),
                page,
            });
            i = last + 1;
        } else if line.trim_start().starts_with("$$") {
            let t = line.trim();
            let one_line = t.len() > 2 && t.ends_with("$$") && t[2..].contains("$$");
            let mut end = i;
            if !one_line {
                // The block runs to the line with the closing `$$`, or to the end of the page.
                end = i + 1;
                while end < lines.len() && !lines[end].contains("$$") {
                    end += 1;
                }
                end = end.min(lines.len() - 1);
            }
            blocks.push(Block {
                kind: Kind::Atomic,
                text: lines[i..=end].join("\n"),
                page,
            });
            i = end + 1;
        } else if is_table_line(line) {
            let mut end = i;
            while end + 1 < lines.len() && is_table_line(lines[end + 1]) {
                end += 1;
            }
            blocks.push(Block {
                kind: Kind::Atomic,
                text: lines[i..=end].join("\n"),
                page,
            });
            i = end + 1;
        } else if is_list_start(line) {
            // An item runs over its indented continuation lines (nested items included).
            let mut end = i;
            while end + 1 < lines.len() {
                let next = lines[end + 1];
                let continues = !next.trim().is_empty()
                    && (next.starts_with(' ') || next.starts_with('\t'))
                    || (!next.trim().is_empty() && !is_block_start(next));
                if continues {
                    end += 1;
                } else {
                    break;
                }
            }
            blocks.push(Block {
                kind: Kind::ListItem,
                text: lines[i..=end].join("\n"),
                page,
            });
            i = end + 1;
        } else {
            let mut end = i;
            while end + 1 < lines.len() {
                let next = lines[end + 1];
                if next.trim().is_empty() || is_block_start(next) {
                    break;
                }
                end += 1;
            }
            blocks.push(Block {
                kind: Kind::Paragraph,
                text: lines[i..=end].join("\n"),
                page,
            });
            i = end + 1;
        }
    }
    blocks
}

/// Splits text into sentences. A sentence ends at `.`, `!`, or `?` followed by white space.
pub fn split_sentences(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut sentences = Vec::new();
    let mut start = 0;
    for i in 0..chars.len() {
        let ends = matches!(chars[i], '.' | '!' | '?')
            && chars.get(i + 1).is_some_and(|c| c.is_whitespace());
        if ends {
            let sentence: String = chars[start..=i].iter().collect();
            if !sentence.trim().is_empty() {
                sentences.push(sentence.trim().to_owned());
            }
            start = i + 1;
        }
    }
    if start < chars.len() {
        let rest: String = chars[start..].iter().collect();
        if !rest.trim().is_empty() {
            sentences.push(rest.trim().to_owned());
        }
    }
    sentences
}

/// Cuts text into pieces of at most `max` characters (never in the middle of a character).
fn hard_split(text: &str, max: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    chars.chunks(max).map(|c| c.iter().collect()).collect()
}

/// Splits a paragraph that is larger than the hard limit into pieces of about the target size.
fn split_paragraph(text: &str, target: usize) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut current = String::new();
    for sentence in split_sentences(text) {
        let sentence_chars = sentence.chars().count();
        if sentence_chars > MAX_CHARS {
            if !current.is_empty() {
                pieces.push(std::mem::take(&mut current));
            }
            pieces.extend(hard_split(&sentence, target));
            continue;
        }
        if !current.is_empty() && current.chars().count() + 1 + sentence_chars > target {
            pieces.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(&sentence);
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    pieces
}

/// The last one or two sentences of the text, at most 300 characters. Empty when there is no good overlap.
fn overlap_of(text: &str) -> String {
    let sentences = split_sentences(text);
    let mut taken: Vec<&String> = Vec::new();
    let mut chars = 0;
    for sentence in sentences.iter().rev().take(OVERLAP_SENTENCES) {
        let n = sentence.chars().count();
        if chars + n > MAX_OVERLAP_CHARS {
            break;
        }
        chars += n + 1;
        taken.push(sentence);
    }
    taken.reverse();
    taken
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

type PathFn = dyn Fn(&[(usize, String)]) -> String;

#[derive(Default)]
struct Building {
    parts: Vec<(Kind, String)>,
    chars: usize,
    first_page: u32,
    last_page: u32,
    path: String,
    /// The last paragraph text, for the overlap of the next chunk.
    last_paragraph: String,
    /// Overlap text at the start. It is part of the chunk text but not of `parts` accounting.
    has_content: bool,
}

impl Building {
    fn is_empty(&self) -> bool {
        !self.has_content
    }

    fn push(&mut self, kind: Kind, text: &str, page: u32) {
        if !self.has_content {
            self.first_page = page;
        }
        self.last_page = page;
        self.chars += text.chars().count() + 2;
        if kind == Kind::Paragraph {
            self.last_paragraph = text.to_owned();
        }
        self.parts.push((kind, text.to_owned()));
        self.has_content = true;
    }

    fn text(&self) -> String {
        let mut out = String::new();
        let mut previous: Option<Kind> = None;
        for (kind, text) in &self.parts {
            if let Some(prev) = previous {
                // List items stay on consecutive lines.
                let separator = if prev == Kind::ListItem && *kind == Kind::ListItem {
                    "\n"
                } else {
                    "\n\n"
                };
                out.push_str(separator);
            }
            out.push_str(text);
            previous = Some(*kind);
        }
        out
    }
}

/// Chunks the pages of a document. `pages` has `(1-based page number, Markdown)` in page order.
pub fn chunk_pages(pages: &[(u32, String)]) -> Vec<Chunk> {
    chunk_pages_with(pages, TARGET_CHARS)
}

/// `chunk_pages` with another target size in characters. Production uses `TARGET_CHARS`. The
/// evaluation tries other sizes. The hard limit `MAX_CHARS` stays the same.
pub fn chunk_pages_with(pages: &[(u32, String)], target: usize) -> Vec<Chunk> {
    chunk_pages_variant(pages, target, crate::headers::PRODUCTION_VARIANT)
}

/// `chunk_pages_with` with a choice for the first occurrence of a running header. Only the
/// evaluation grid calls it with another variant than the one of production.
pub fn chunk_pages_variant(
    pages: &[(u32, String)],
    target: usize,
    variant: crate::headers::HeaderMode,
) -> Vec<Chunk> {
    // The running headers and footers of the book go first, so no heading path, embedded text, or
    // BM25 field holds them. The ingestion worker and the evaluation both come through here.
    chunk_stripped_pages(
        &crate::headers::strip_running_lines_with(pages, variant).pages,
        target,
    )
}

/// Chunks pages as they are, with no removal of running lines. The evaluation uses it to measure
/// what the removal changes.
pub fn chunk_stripped_pages(pages: &[(u32, String)], target: usize) -> Vec<Chunk> {
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut current = Building::default();

    let path_of = |stack: &[(usize, String)]| {
        stack
            .iter()
            .map(|(_, t)| t.as_str())
            .collect::<Vec<_>>()
            .join(" > ")
    };

    fn flush(current: &mut Building, chunks: &mut Vec<Chunk>) -> String {
        if current.is_empty() {
            return String::new();
        }
        let done = std::mem::take(current);
        let overlap = overlap_of(&done.last_paragraph);
        chunks.push(Chunk {
            index: u32::try_from(chunks.len()).unwrap_or(u32::MAX),
            first_page: done.first_page,
            last_page: done.last_page,
            heading_path: done.path.clone(),
            text: done.text(),
        });
        overlap
    }

    for (page, markdown) in pages {
        for block in parse_blocks(markdown, *page) {
            match block.kind {
                Kind::Heading(level) => {
                    flush(&mut current, &mut chunks);
                    while stack.last().is_some_and(|(l, _)| *l >= level) {
                        stack.pop();
                    }
                    stack.push((level, block.text.clone()));
                }
                Kind::Paragraph if block.text.chars().count() > MAX_CHARS => {
                    let pieces = split_paragraph(&block.text, target);
                    for piece in pieces {
                        let part = Block {
                            kind: Kind::Paragraph,
                            text: piece,
                            page: block.page,
                        };
                        add_block(&mut current, &mut chunks, &stack, &path_of, target, &part);
                    }
                }
                _ => add_block(&mut current, &mut chunks, &stack, &path_of, target, &block),
            }
        }
    }
    flush(&mut current, &mut chunks);
    chunks
}

fn add_block(
    current: &mut Building,
    chunks: &mut Vec<Chunk>,
    stack: &[(usize, String)],
    path_of: &PathFn,
    target: usize,
    block: &Block,
) {
    let size = block.text.chars().count() + 2;
    if !current.is_empty() && current.chars + size > target {
        // The size limit ends this chunk. The next one starts with an overlap.
        let done = std::mem::take(current);
        let overlap = overlap_of(&done.last_paragraph);
        chunks.push(Chunk {
            index: u32::try_from(chunks.len()).unwrap_or(u32::MAX),
            first_page: done.first_page,
            last_page: done.last_page,
            heading_path: done.path.clone(),
            text: done.text(),
        });
        current.path = path_of(stack);
        if !overlap.is_empty() {
            // The overlap belongs to the text only. The page range starts at the first real block.
            current.parts.push((Kind::Paragraph, overlap.clone()));
            current.chars += overlap.chars().count() + 2;
        }
    } else if current.is_empty() {
        current.path = path_of(stack);
    }
    current.push(block.kind, &block.text, block.page);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages(items: &[&str]) -> Vec<(u32, String)> {
        items
            .iter()
            .enumerate()
            .map(|(i, t)| (u32::try_from(i + 1).unwrap(), (*t).to_owned()))
            .collect()
    }

    fn sentence_text(n: usize) -> String {
        (0..n)
            .map(|i| format!("This is sentence number {i} about the topic."))
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn headings_split_chunks_and_build_the_path() {
        let md = "# Algebra\n\nIntro text.\n\n## Linear equations\n\nSolve for x.\n\n## Quadratics\n\nUse the formula.\n\n# Geometry\n\nShapes.";
        let chunks = chunk_pages(&pages(&[md]));
        let got: Vec<(&str, &str)> = chunks
            .iter()
            .map(|c| (c.heading_path.as_str(), c.text.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("Algebra", "Intro text."),
                ("Algebra > Linear equations", "Solve for x."),
                ("Algebra > Quadratics", "Use the formula."),
                ("Geometry", "Shapes."),
            ]
        );
        let indexes: Vec<u32> = chunks.iter().map(|c| c.index).collect();
        assert_eq!(indexes, [0, 1, 2, 3]);
    }

    #[test]
    fn production_chunks_the_pages_as_they_are_and_the_experiment_removes_running_headers() {
        let pages = pages(&[
            "# BOOK SECTION ONE\n\n## Rules\n\nFirst rule text.",
            "# BOOK CHAPTER ONE\n\nMore about the rules of grammar.",
            "# BOOK SECTION ONE\n\nEven more about the laws of motion.",
            "# BOOK CHAPTER ONE\n\nThe last words about the rules of chess.",
            "# BOOK SECTION ONE\n\nAn extra page.",
            "# BOOK CHAPTER ONE\n\nAnd one more page.",
        ]);
        // Production does not strip: the text is chunked as it is.
        assert_eq!(
            chunk_pages(&pages),
            chunk_stripped_pages(&pages, TARGET_CHARS)
        );
        // The experimental rule (only the evaluation grid uses it).
        let clean =
            chunk_pages_variant(&pages, TARGET_CHARS, crate::headers::HeaderMode::KeepFirst);
        let paths: Vec<&str> = clean.iter().map(|c| c.heading_path.as_str()).collect();
        assert!(
            paths
                .iter()
                .all(|p| !p.contains("CHAPTER") || *p == "BOOK CHAPTER ONE"),
            "{paths:?}"
        );
        let text: String = clean.iter().map(|c| c.text.as_str()).collect();
        assert!(text.contains("The last words about the rules of chess."));
        // Without the removal, the later copies of the header are headings of their own.
        let raw = chunk_stripped_pages(&pages, TARGET_CHARS);
        assert!(
            raw.len() > clean.len(),
            "{} against {}",
            raw.len(),
            clean.len()
        );
    }

    #[test]
    fn text_before_the_first_heading_has_an_empty_path() {
        let chunks = chunk_pages(&pages(&["Preface text.\n\n# Chapter\n\nBody."]));
        assert_eq!(chunks[0].heading_path, "");
        assert_eq!(chunks[1].heading_path, "Chapter");
    }

    #[test]
    fn embed_text_has_the_title_and_the_path() {
        let c = Chunk {
            index: 0,
            first_page: 1,
            last_page: 1,
            heading_path: "A > B".into(),
            text: "Body".into(),
        };
        assert_eq!(embed_text("Notes", &c), "Notes > A > B\nBody");
        let c = Chunk {
            heading_path: String::new(),
            ..c
        };
        assert_eq!(embed_text("Notes", &c), "Notes\nBody");
    }

    #[test]
    fn a_math_block_is_never_split() {
        let math = format!("$$\n{}\n$$", "x + y = z \\\\\n".repeat(200));
        let md = format!("{}\n\n{math}\n\n{}", sentence_text(30), sentence_text(30));
        let chunks = chunk_pages(&pages(&[&md]));
        assert!(chunks.len() >= 3);
        let holding: Vec<&Chunk> = chunks
            .iter()
            .filter(|c| c.text.contains("x + y = z"))
            .collect();
        assert_eq!(holding.len(), 1, "the block must live in one chunk");
        assert!(holding[0].text.contains(&math));
        for c in &chunks {
            assert_eq!(
                c.text.matches("$$").count() % 2,
                0,
                "unbalanced $$ in {}",
                c.text
            );
        }
    }

    #[test]
    fn single_line_and_open_math_blocks_stay_whole() {
        let md = "Before.\n\n$$a^2 + b^2 = c^2$$\n\nAfter.\n\n$$\n\\int_0^1 x\\,dx\n$$\n\nEnd.";
        let chunks = chunk_pages(&pages(&[md]));
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.contains("$$a^2 + b^2 = c^2$$"));
        assert!(chunks[0].text.contains("$$\n\\int_0^1 x\\,dx\n$$"));
    }

    #[test]
    fn a_table_is_not_split() {
        let rows: String = (0..60)
            .map(|i| format!("| row {i} | {} |\n", "cell ".repeat(8)))
            .collect();
        let table = format!("| a | b |\n|---|---|\n{rows}");
        let md = format!("{}\n\n{table}\n\n{}", sentence_text(25), sentence_text(25));
        let chunks = chunk_pages(&pages(&[&md]));
        let holding: Vec<&Chunk> = chunks
            .iter()
            .filter(|c| c.text.contains("| row 0 |"))
            .collect();
        assert_eq!(holding.len(), 1);
        assert!(holding[0].text.contains("| row 59 |"));
        assert!(holding[0].text.contains("|---|---|"));
    }

    #[test]
    fn fenced_code_is_not_split() {
        let code = format!("```rust\n{}\n```", "let a = 1;\n".repeat(150));
        let md = format!("{}\n\n{code}\n\nAfter the code.", sentence_text(25));
        let chunks = chunk_pages(&pages(&[&md]));
        let holding: Vec<&Chunk> = chunks
            .iter()
            .filter(|c| c.text.contains("let a = 1;"))
            .collect();
        assert_eq!(holding.len(), 1);
        assert_eq!(holding[0].text.matches("```").count(), 2);
    }

    #[test]
    fn list_items_stay_whole_and_consecutive() {
        let items: String = (0..40)
            .map(|i| format!("- item {i} {}\n  more of item {i}\n", "word ".repeat(15)))
            .collect();
        let chunks = chunk_pages(&pages(&[&items]));
        assert!(chunks.len() >= 2, "a long list is cut between items");
        for c in &chunks {
            for line in c.text.lines().filter(|l| l.starts_with("- item")) {
                let n: usize = line.split_whitespace().nth(2).unwrap().parse().unwrap();
                assert!(
                    c.text.contains(&format!("  more of item {n}")),
                    "item {n} was cut"
                );
            }
        }
        // Items are on consecutive lines, not separated by blank lines.
        assert!(!chunks[0].text.contains("\n\n"));
    }

    #[test]
    fn a_smaller_target_gives_more_and_smaller_chunks() {
        let paragraphs: Vec<String> = (0..12)
            .map(|p| format!("Paragraph {p} starts here. {}", sentence_text(12)))
            .collect();
        let input = pages(&[&paragraphs.join("\n\n")]);
        let normal = chunk_pages(&input);
        assert_eq!(normal, chunk_pages_with(&input, TARGET_CHARS));
        let small = chunk_pages_with(&input, TARGET_CHARS / 2);
        let large = chunk_pages_with(&input, MAX_CHARS);
        assert!(small.len() > normal.len() && normal.len() > large.len());
        assert!(small.iter().all(|c| c.text.chars().count() <= MAX_CHARS));
        assert!(large.iter().all(|c| c.text.chars().count() <= MAX_CHARS));
    }

    #[test]
    fn long_text_is_packed_to_the_target_with_overlap() {
        let paragraphs: Vec<String> = (0..12)
            .map(|p| format!("Paragraph {p} starts here. {}", sentence_text(12)))
            .collect();
        let md = paragraphs.join("\n\n");
        let chunks = chunk_pages(&pages(&[&md]));
        assert!(chunks.len() > 2);
        for c in &chunks {
            assert!(
                c.text.chars().count() <= MAX_CHARS,
                "chunk {} has {} chars",
                c.index,
                c.text.chars().count()
            );
        }
        for c in &chunks[..chunks.len() - 1] {
            assert!(
                c.text.chars().count() >= TARGET_CHARS / 2,
                "chunk {} is too small",
                c.index
            );
        }
        // The next chunk starts with the last sentence(s) of the previous chunk.
        for pair in chunks.windows(2) {
            let tail: Vec<String> = split_sentences(&pair[0].text);
            let last = tail.last().unwrap();
            assert!(
                pair[1].text.starts_with(last.as_str()) || pair[1].text.contains(last.as_str()),
                "no overlap between {} and {}",
                pair[0].index,
                pair[1].index
            );
        }
    }

    #[test]
    fn overlap_is_one_or_two_short_sentences() {
        let text = "One. Two is here. Three is the last one.";
        assert_eq!(overlap_of(text), "Two is here. Three is the last one.");
        assert_eq!(
            overlap_of("Only one sentence here."),
            "Only one sentence here."
        );
        let long = format!("Short. {}.", "x".repeat(400));
        assert_eq!(
            overlap_of(&long),
            "",
            "an overlong sentence gives no overlap"
        );
        assert_eq!(overlap_of(""), "");
    }

    #[test]
    fn no_overlap_after_a_heading() {
        let md = format!("# A\n\n{}\n\n# B\n\nStart of B.", sentence_text(5));
        let chunks = chunk_pages(&pages(&[&md]));
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[1].text, "Start of B.");
    }

    #[test]
    fn page_ranges_follow_the_blocks() {
        let p1 = format!("# Topic\n\n{}", sentence_text(10));
        let p2 = sentence_text(10);
        let p3 = format!("{}\n\n# Next\n\nShort end.", sentence_text(40));
        let chunks = chunk_pages(&pages(&[&p1, &p2, &p3]));
        assert_eq!(chunks[0].first_page, 1);
        assert!(chunks.iter().all(|c| c.first_page <= c.last_page));
        assert!(chunks.windows(2).all(|w| w[0].last_page <= w[1].last_page));
        let last = chunks.last().unwrap();
        assert_eq!(
            (last.heading_path.as_str(), last.first_page, last.last_page),
            ("Next", 3, 3)
        );
        assert!(
            chunks.iter().any(|c| c.first_page == 1 && c.last_page >= 2),
            "a chunk spans pages 1 and 2"
        );
    }

    #[test]
    fn a_huge_paragraph_is_split_at_sentences_within_the_limit() {
        let md = sentence_text(300);
        let chunks = chunk_pages(&pages(&[&md]));
        assert!(chunks.len() > 3);
        for c in &chunks {
            assert!(c.text.chars().count() <= MAX_CHARS);
        }
    }

    #[test]
    fn a_giant_unbroken_run_is_cut_by_characters() {
        let md = "é".repeat(7000);
        let chunks = chunk_pages(&pages(&[&md]));
        assert!(chunks.len() >= 4);
        for c in &chunks {
            assert!(c.text.chars().count() <= MAX_CHARS);
        }
        let total: usize = chunks.iter().map(|c| c.text.chars().count()).sum();
        assert!(total >= 7000);
    }

    #[test]
    fn empty_and_blank_pages_make_no_chunks() {
        assert!(chunk_pages(&[]).is_empty());
        assert!(chunk_pages(&pages(&["", "   \n\n  ", "# Only a heading"])).is_empty());
    }

    #[test]
    fn heading_levels_pop_correctly() {
        let md = "# A\n\n## B\n\n### C\n\nc text\n\n## D\n\nd text\n\n# E\n\ne text";
        let chunks = chunk_pages(&pages(&[md]));
        let paths: Vec<&str> = chunks.iter().map(|c| c.heading_path.as_str()).collect();
        assert_eq!(paths, ["A > B > C", "A > D", "E"]);
    }

    #[test]
    fn hashes_without_a_space_are_not_headings() {
        let chunks = chunk_pages(&pages(&["#hashtag and #1 are text.\n\n#### Deep\n\nBody."]));
        assert_eq!(chunks[0].text, "#hashtag and #1 are text.");
        assert_eq!(chunks[1].heading_path, "Deep");
    }
}
