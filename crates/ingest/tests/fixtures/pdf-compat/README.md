# PDF compatibility fixtures

`scripts/make-pdf-fixtures.sh` makes every file here from sources in the script. No third-party PDF is used. `crates/ingest/tests/pdf_compat.rs` runs each file through the ingest worker (fake embeddings, a test transcriber) and checks the outcome in the table below.

The base text is "Cell Biology Notes. Mitochondria produce most of the chemical energy that a cell needs. The nucleus stores the genetic material of the cell." Files that start from it are marked "base".

| File | Producer | Features | Expected outcome |
|---|---|---|---|
| `latex-plain.pdf` | pdflatex | Type 1 fonts, base text | ready, 1 page, 0 transcribed |
| `latex-math.pdf` | pdflatex | Type 1 math fonts, quadratic formula, sum | ready, 1 page, 1 transcribed (math) |
| `quartz-cupsfilter.pdf` | macOS Quartz PDFContext (`cupsfilter`) | plain text, PDF 1.3 | ready, 1 page |
| `gs-13.pdf` | Ghostscript pdfwrite | base, compatibility 1.3 | ready, 1 page |
| `gs-14.pdf` | Ghostscript pdfwrite | base, compatibility 1.4 | ready, 1 page |
| `gs-17.pdf` | Ghostscript pdfwrite | base, compatibility 1.7 | ready, 1 page |
| `gs-pdfa2b.pdf` | Ghostscript pdfwrite | base, PDF/A-2b with an sRGB output intent | ready, 1 page |
| `qpdf-objstm-xrefstream.pdf` | qpdf | base, object streams and a cross-reference stream | ready, 1 page |
| `qpdf-linearized.pdf` | qpdf | base, linearized | ready, 1 page |
| `qpdf-pdf20.pdf` | qpdf | base, `%PDF-2.0` header | ready, 1 page |
| `incremental-update.pdf` | hand-written by the script (Python) | revision 1 "Draft Lecture Sheet", appended update "Final Lecture Sheet" | ready, newest text only, no "Draft" text |
| `qpdf-encrypted-owner-only.pdf` | qpdf | base, AES-256, owner password only (empty user password) | ready, 1 page |
| `qpdf-encrypted-user-password.pdf` | qpdf | base, AES-256, user password `student-secret` | failed, "This PDF is password protected. Remove the password and upload it again." |
| `scanned-image-only.pdf` | ImageMagick | text rasterized at 150 DPI, JPEG, no text layer | ready, 1 page, 1 transcribed |
| `qpdf-rotated-90.pdf` | qpdf | base, page `/Rotate 90` | ready, 1 page. pdfium's `all()` looks in a rectangle with width and height swapped for 90 and 270 degrees, so the worker reads the text inside the page's bounding box instead (`pdf::read_text`). |
| `qpdf-rotated-180.pdf` | qpdf | base, page `/Rotate 180` | ready, 1 page |
| `qpdf-rotated-270.pdf` | qpdf | base, page `/Rotate 270` | ready, 1 page |
| `multipage-3.pdf` | pdflatex | 3 pages, different text on each (rivers, mountains, deserts) | ready, 3 pages, right text on each page |
| `greek-xelatex.pdf` | XeLaTeX | Greek, embedded TeX Gyre Termes (OpenType) | ready, 1 page. Greek letters count as math symbols, so the page may be transcribed. |
| `chinese-xelatex.pdf` | XeLaTeX | Chinese, embedded FandolSong (OpenType) | ready, 1 page |
| `damaged-repairable-xref.pdf` | pdflatex plus a script edit | base, `startxref` offset set to a wrong value | ready, 1 page (pdfium rebuilds the table) |
| `damaged-unopenable.pdf` | qpdf plus a script edit | the first third of an object-stream file, no catalog or trailer | failed, "This PDF could not be opened. It may be damaged. Try another copy of the file." |

To add a case, add the file to the script, add one entry to `CASES` in `pdf_compat.rs`, and add a row here.
