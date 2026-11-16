# Retrieval evaluation (phase 4)

Retrieval quality decides answer quality. This phase measures it with fixed data, protects it in CI, and gives the numbers that tune the chunker, the fusion, and the relevance gate.

## What we measure

- **Recall@5 and Recall@8**: the share of questions where at least one relevant chunk is in the top 5 or the top 8 fused results. The chat prompt uses the top 8.
- **nDCG@10**: ranking quality with graded relevance (2 for the labeled page, 1 for an adjacent page).
- **MRR**: the mean of 1 / rank of the first relevant chunk.
- **Gate accuracy**: questions labeled "not covered" must be gated (no sources), and covered questions must not be. Report the false-gate rate and the missed-gate rate separately.
- **Latency** (live mode only): p50 and p95 for dense, lexical, and the whole retrieval step.

A chunk is relevant when its page range includes a labeled page. Every metric is reported for dense only, BM25 only, and hybrid.

## Data

- `evals/corpus/`: an excerpt of an openly licensed mathematics textbook (OpenStax, CC BY 4.0), about 40 pages that cover quadratics, functions, and exponents, with an `ATTRIBUTION.md` that names the source, the licence, and the changes (page selection only). The excerpt has real textbook layout: headings, worked examples, exercises, figures, and display math.
- `evals/golden.jsonl`: one question per line: `{ "id", "question", "relevantPages": [n], "kind" }`. `kind` is one of `lookup` (close to the book's words), `paraphrase` (different words), `notation` (written in math notation, for example `x^2 - 5x + 6 = 0`), `followup` (needs the previous question, given in `context`), and `not_covered` (answerable, but not from this corpus). At least 80 questions, at least 10 of each kind. Draft questions may come from a model, but every question is checked by hand against the page before it goes in.
- `evals/corpus/sections.json`: splits the excerpt into five sections. The evaluation treats each section as one document, so the per-document cap works as it does in a course with several PDFs. (A course with one big PDF gets at most 4 sources per question, because of that cap.)
- `evals/cache-dev/`: a development cache (PDF text layer and fake embeddings) that shows the harness works. It is never used for the baseline, and `--check` refuses it.
- `evals/cache/`: the inputs that make the offline mode deterministic: the page Markdown that real ingestion produced for the corpus (Claude transcription included), and the embeddings of every chunk and every question (keyed by embedding model and the SHA-256 of the exact embedded text, stored as compressed 16-bit floats).

## Modes

- **Offline** (`make eval`): chunk the cached pages with the real chunker, build the real BM25 index and an in-memory dense index from the cached embeddings, run the real retrieval and fusion code for every golden question, and print the metrics table. No AWS, no network, deterministic, a few seconds. A missing cache entry is an error, not a silent skip.
- **Live** (`make eval-live` with `AWS_PROFILE`): ingest the corpus PDF through the real worker code with real Bedrock transcription and Titan embeddings, query real S3 Vectors in the dev stack, measure latency, and refresh `evals/cache/` (the diff is reviewed and committed like code). The first version of the refresh runs the pipeline on this machine and does not query S3 Vectors: it records the time of the Titan query embedding calls, and the S3 Vectors and BM25 latency measurement is still to do.

## Protection in CI

`evals/baseline.json` holds the offline metrics of the committed code. The `checks` workflow runs `make eval` and fails when any hybrid metric drops by more than 0.01 (absolute) below the baseline, or when gate accuracy drops at all. A change that improves the metrics updates the baseline in the same commit.

## Tuning (after the harness exists)

Run the offline mode over a grid and record the results in `evals/results/<date>.md`: chunk target 300, 400, and 600 tokens; RRF `k` 30, 60, and 100; the top 8 versus the top 6 and the top 10; the gate distance 0.75 to 0.90; dense candidates 20 versus 40. Change a default only when hybrid Recall@8 or nDCG@10 improves without hurting gate accuracy. Contextual retrieval with a short model-written context per chunk is a later experiment against this same harness.

The retrieval code that the API uses (query cleaning, fusion, the per-document cap, the gate) lives in `openbook-search`, so the evaluation runs exactly the production code.
