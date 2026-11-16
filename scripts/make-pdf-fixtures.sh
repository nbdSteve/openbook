#!/usr/bin/env bash
# Regenerates every PDF in crates/ingest/tests/fixtures/pdf-compat/ from the sources in this script.
#
# No third-party PDF is downloaded, so the fixtures are ours and carry no license questions. Each
# fixture has a known short text. The README next to the PDFs lists the producer, the features, and
# the expected outcome of each file, and crates/ingest/tests/pdf_compat.rs checks them.
#
# Tools: pdflatex and xelatex (TeX Live with the Fandol and TeX Gyre fonts), gs (Ghostscript),
# magick (ImageMagick), cupsfilter (macOS), qpdf, and python3.
# On macOS: brew install qpdf ghostscript imagemagick.
#
# Usage: scripts/make-pdf-fixtures.sh
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="${repo_root}/crates/ingest/tests/fixtures/pdf-compat"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

for tool in pdflatex xelatex gs magick cupsfilter qpdf python3; do
  command -v "$tool" > /dev/null 2>&1 || {
    echo "error: $tool is not installed" >&2
    exit 1
  }
done

# Fixed dates keep the output close to reproducible.
export SOURCE_DATE_EPOCH=1767225600 # 2026-01-01T00:00:00Z
export FORCE_SOURCE_DATE=1

mkdir -p "$out"
rm -f "$out"/*.pdf

# qpdf exits with 3 for warnings about things that it repaired. That is fine for our inputs.
qpdf_run() {
  local status=0
  qpdf "$@" || status=$?
  if [ "$status" -ne 0 ] && [ "$status" -ne 3 ]; then
    return "$status"
  fi
}

# latex <engine> <file.tex>: builds in the work directory, and shows the log tail on failure.
latex() {
  local log="$work/${2%.tex}.log"
  if ! (cd "$work" && "$1" -interaction=nonstopmode -halt-on-error "$2" > "$log" 2>&1); then
    tail -30 "$log" >&2
    return 1
  fi
}

# The Ghostscript profile for PDF/A lives next to PDFA_def.ps.
gs_icc() {
  local prefix def
  prefix="$(brew --prefix ghostscript 2> /dev/null || echo /usr/local)"
  def="$(find "$prefix/share" "$prefix/opt" -name PDFA_def.ps 2> /dev/null | head -1)"
  if [ -z "$def" ]; then
    echo "error: cannot find Ghostscript's PDFA_def.ps" >&2
    return 1
  fi
  echo "$(dirname "$(dirname "$def")")/iccprofiles/srgb.icc"
}

# 1. The base text document, from pdflatex. The Ghostscript and qpdf variants start from it.
cat > "$work/base.tex" << 'EOF'
\documentclass[11pt]{article}
\usepackage[T1]{fontenc}
\usepackage[a4paper,margin=25mm]{geometry}
\pagestyle{empty}
\begin{document}
\section*{Cell Biology Notes}
Mitochondria produce most of the chemical energy that a cell needs.
The nucleus stores the genetic material of the cell.
\end{document}
EOF
latex pdflatex base.tex

# 2. LaTeX math: Type 1 Computer Modern fonts, a quadratic formula, and a sum.
cat > "$work/latex-math.tex" << 'EOF'
\documentclass[11pt]{article}
\usepackage[a4paper,margin=25mm]{geometry}
\pagestyle{empty}
\begin{document}
\section*{Quadratic Formulas}
The roots of a quadratic equation are given by the quadratic formula.
\[ x = \frac{-b \pm \sqrt{b^2 - 4ac}}{2a} \]
The sum of the first $n$ whole numbers has a closed form.
\[ \sum_{k=1}^{n} k = \frac{n(n+1)}{2} \]
\end{document}
EOF
latex pdflatex latex-math.tex
cp "$work/latex-math.pdf" "$out/latex-math.pdf"
cp "$work/base.pdf" "$out/latex-plain.pdf"

# 3. macOS Quartz: the print system turns plain text into a PDF with the Quartz PDFContext producer.
printf 'Photosynthesis Summary\n\nChlorophyll absorbs red and blue light and reflects green light.\nThe process makes glucose and releases oxygen.\n' > "$work/quartz.txt"
cupsfilter -m application/pdf "$work/quartz.txt" > "$out/quartz-cupsfilter.pdf" 2> /dev/null

# 4. Ghostscript re-writes at three compatibility levels, and a PDF/A-2b.
for level in 1.3 1.4 1.7; do
  gs -q -dNOPAUSE -dBATCH -dSAFER -sDEVICE=pdfwrite -dCompatibilityLevel="$level" \
    -sOutputFile="$out/gs-${level//./}.pdf" "$work/base.pdf"
done

icc="$(gs_icc)"
cat > "$work/PDFA_def.ps" << EOF
%!
[/_objdef {icc_PDFA} /type /stream /OBJ pdfmark
[{icc_PDFA} <</N 3>> /PUT pdfmark
[{icc_PDFA} ($icc) (r) file /PUT pdfmark
[/_objdef {OutputIntent_PDFA} /type /dict /OBJ pdfmark
[{OutputIntent_PDFA} <<
  /Type /OutputIntent
  /S /GTS_PDFA1
  /DestOutputProfile {icc_PDFA}
  /OutputConditionIdentifier (sRGB)
>> /PUT pdfmark
[{Catalog} <</OutputIntents [ {OutputIntent_PDFA} ]>> /PUT pdfmark
EOF
gs -q -dNOPAUSE -dBATCH -dNOSAFER -sDEVICE=pdfwrite -dPDFA=2 -dPDFACompatibilityPolicy=1 \
  -sColorConversionStrategy=RGB -sOutputFile="$out/gs-pdfa2b.pdf" "$work/PDFA_def.ps" "$work/base.pdf"

# 5. qpdf structure variants of the base document.
qpdf_run --object-streams=generate --compress-streams=y "$work/base.pdf" "$out/qpdf-objstm-xrefstream.pdf"
qpdf_run --linearize "$work/base.pdf" "$out/qpdf-linearized.pdf"
qpdf_run --force-version=2.0 "$work/base.pdf" "$out/qpdf-pdf20.pdf"
qpdf_run --rotate=+90:1 "$work/base.pdf" "$out/qpdf-rotated-90.pdf"
qpdf_run --rotate=+180:1 "$work/base.pdf" "$out/qpdf-rotated-180.pdf"
qpdf_run --rotate=+270:1 "$work/base.pdf" "$out/qpdf-rotated-270.pdf"
# Owner password only: the user password is empty, so any reader opens it.
qpdf_run --encrypt "" "owner-secret" 256 -- "$work/base.pdf" "$out/qpdf-encrypted-owner-only.pdf"
# User password: nobody can open it without "student-secret".
qpdf_run --encrypt "student-secret" "owner-secret" 256 -- "$work/base.pdf" "$out/qpdf-encrypted-user-password.pdf"

# 6. An incremental update. Revision 1 is the draft. An update appended after it replaces the
# content stream of page 1. The newest revision must win.
python3 -I - "$out/incremental-update.pdf" << 'EOF'
import sys


def stream(lines):
    body = (
        "BT /F1 14 Tf 72 760 Td (%s) Tj 0 -24 Td (%s) Tj ET" % lines
    ).encode()
    return b"<< /Length %d >>\nstream\n" % len(body) + body + b"\nendstream"


def obj(number, body):
    return b"%d 0 obj\n" % number + body + b"\nendobj\n"


draft = ("Draft Lecture Sheet", "This is the first revision of the sheet and it is out of date.")
final = ("Final Lecture Sheet", "This is the newest revision of the sheet and it is correct.")

data = bytearray(b"%PDF-1.4\n")
offsets = {}
bodies = {
    1: b"<< /Type /Catalog /Pages 2 0 R >>",
    2: b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    3: b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
    4: stream(draft),
    5: b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
}
for number in sorted(bodies):
    offsets[number] = len(data)
    data += obj(number, bodies[number])
xref1 = len(data)
data += b"xref\n0 6\n0000000000 65535 f \n"
for number in range(1, 6):
    data += b"%010d 00000 n \n" % offsets[number]
data += b"trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % xref1

new4 = len(data)
data += obj(4, stream(final))
xref2 = len(data)
data += b"xref\n4 1\n%010d 00000 n \n" % new4
data += b"trailer\n<< /Size 6 /Root 1 0 R /Prev %d >>\nstartxref\n%d\n%%%%EOF\n" % (xref1, xref2)
open(sys.argv[1], "wb").write(data)
EOF

# 7. A scan: text rasterized at 150 DPI and wrapped as a PDF. It has no text layer.
magick -size 1240x1754 xc:white -fill black -font /System/Library/Fonts/Helvetica.ttc -pointsize 56 \
  -annotate +100+200 "Scanned Lecture Notes" -pointsize 34 \
  -annotate +100+300 "The Pythagorean theorem links the three sides" \
  -annotate +100+360 "of a right triangle." \
  -colorspace Gray -depth 8 "$work/scan.png"
magick "$work/scan.png" -density 150 -units PixelsPerInch -quality 60 -compress JPEG "$out/scanned-image-only.pdf"

# 8. A multi-page file: three pages with different text.
cat > "$work/multipage.tex" << 'EOF'
\documentclass[11pt]{article}
\usepackage[T1]{fontenc}
\usepackage[a4paper,margin=25mm]{geometry}
\pagestyle{empty}
\begin{document}
\section*{Chapter One: Rivers}
The Nile flows north through Egypt into the Mediterranean Sea.
\newpage
\section*{Chapter Two: Mountains}
The Andes form the longest mountain range on land.
\newpage
\section*{Chapter Three: Deserts}
The Sahara is the largest hot desert in the world.
\end{document}
EOF
latex pdflatex multipage.tex
cp "$work/multipage.pdf" "$out/multipage-3.pdf"

# 9. Non-Latin scripts with embedded fonts (XeLaTeX): Greek, and Chinese.
cat > "$work/greek.tex" << 'EOF'
\documentclass[11pt]{article}
\usepackage[a4paper,margin=25mm]{geometry}
\usepackage{fontspec}
\setmainfont{texgyretermes-regular.otf}
\pagestyle{empty}
\begin{document}
\section*{Εισαγωγή στη Γεωμετρία}
Το θεώρημα του Πυθαγόρα συνδέει τις πλευρές ενός ορθογωνίου τριγώνου.
\end{document}
EOF
latex xelatex greek.tex
cp "$work/greek.pdf" "$out/greek-xelatex.pdf"

cat > "$work/chinese.tex" << 'EOF'
\documentclass[11pt]{article}
\usepackage[a4paper,margin=25mm]{geometry}
\usepackage{xeCJK}
\setCJKmainfont{FandolSong-Regular.otf}
\pagestyle{empty}
\begin{document}
\section*{细胞生物学笔记}
线粒体为细胞提供大部分化学能量。细胞核储存细胞的遗传物质。
\end{document}
EOF
latex xelatex chinese.tex
cp "$work/chinese.pdf" "$out/chinese-xelatex.pdf"

# 10. Damaged files.
# 10a. Repairable: the startxref offset points to the wrong place. pdfium rebuilds the table.
python3 -I - "$out/qpdf-linearized.pdf" "$out/qpdf-objstm-xrefstream.pdf" "$work/base.pdf" "$out/damaged-repairable-xref.pdf" << 'EOF'
import re, sys

data = open(sys.argv[3], "rb").read()
broken = re.sub(rb"startxref\s+\d+", b"startxref\n7", data)
assert broken != data
open(sys.argv[4], "wb").write(broken)
EOF
# 10b. Unopenable: a valid object-stream file cut off before its objects and trailer.
python3 -I - "$out/qpdf-objstm-xrefstream.pdf" "$out/damaged-unopenable.pdf" << 'EOF'
import sys

data = open(sys.argv[1], "rb").read()
open(sys.argv[2], "wb").write(data[: len(data) // 3])
EOF

echo "Fixtures in $out:"
for f in "$out"/*.pdf; do printf '%s %s\n' "$(wc -c < "$f" | tr -d ' ')" "$(basename "$f")"; done
du -sk "$out" | awk '{print "total", $1, "KB"}'
