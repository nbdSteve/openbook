# Retrieval evaluation

This directory holds the data of the retrieval evaluation. The design is in `docs/evaluation.md`. The code is the `openbook-eval` crate (`crates/eval`).

The evaluation runs the same retrieval code as the API (`openbook_search::retrieval`: query cleaning, reciprocal rank fusion, the per-document cap, the relevance gate), the real chunker, and the real BM25 index. The dense index is in memory, over embeddings from the cache.

## What is here

| Path | What |
|---|---|
| `corpus/college-algebra-excerpt.pdf` | 43 pages of the OpenStax book "College Algebra" (CC BY 4.0). See `corpus/ATTRIBUTION.md`. |
| `corpus/sections.json` | The five sections of the excerpt. Each section is one document for the evaluation, so the per-document cap works as it does in a course with several PDFs. |
| `golden.jsonl` | The golden questions. Page numbers are pages of the excerpt. |
| `cache/` | The live cache (made by `refresh`, reviewed and committed like code). Not there until the first live refresh. |
| `cache-dev/` | The development cache. It proves that the harness works. It is never a baseline. |
| `baseline.json` | The hybrid metrics of the committed code, from the live cache. Not there until the first live refresh. |

## Commands

Run all commands from the repository root.

### Offline (no network, no AWS)

```
cargo run -p openbook-eval -- offline --dev-cache
```

This runs from `evals/cache-dev` and prints the metrics table. It writes `target/eval/report.json` and `target/eval/report.md`.

```
cargo run -p openbook-eval -- offline
cargo run -p openbook-eval -- offline --check
```

The first command runs from the live cache `evals/cache`. The second one also compares the hybrid metrics with `evals/baseline.json` and exits with code 1 when a metric fell by more than 0.01 (absolute), or when gate accuracy fell at all. A missing cache entry is an error. The command never skips a question.

When a change makes the metrics better, update the baseline in the same commit:

```
cargo run -p openbook-eval -- offline --write-baseline
```

`--check` and `--write-baseline` refuse a dev cache.

For tuning, `offline` takes `--rrf-k N`, `--final-sources N`, `--max-per-document N`, and `--gate-distance X`. They change the numbers of fusion and the gate for one run. The production numbers are in `crates/search/src/retrieval.rs`.

### Refresh the live cache (real AWS)

```
OPENBOOK_PDFIUM_PATH=$(scripts/fetch-pdfium.sh | tail -1)/lib AWS_PROFILE=<profile> cargo run -p openbook-eval --release -- refresh
```

This prints what it will call and about what it costs, and then stops. To start, add `--yes`. It does these steps with the real code of the ingestion worker:

1. Reads the corpus PDF with pdfium. A page with a poor text layer (math symbols, images) goes to the real Bedrock page transcription.
2. Rewrites each follow-up question with the fast model, with the real `rewrite_query` of the API.
3. Embeds every chunk and every question with Titan Text Embeddings v2.
4. Writes `evals/cache/` (page Markdown, embeddings, rewrites, and the time of the query embedding calls).

It reads the normal AWS environment (`AWS_PROFILE`, `AWS_REGION`; the Region is `ap-southeast-2` when none is set). The model for transcription and rewrite is `au.anthropic.claude-haiku-4-5-20251001-v1:0` unless `OPENBOOK_EVAL_MODEL_ID` says another one. It does not touch S3, DynamoDB, SQS, or S3 Vectors. Review the diff of `evals/cache/`, then run `offline --write-baseline` and commit both.

### Refresh the development cache (no AWS)

```
OPENBOOK_PDFIUM_PATH=$(scripts/fetch-pdfium.sh | tail -1)/lib cargo run -p openbook-eval -- refresh --dev-cache
```

The pages are the PDF text layer, the embeddings are fake (hashed words), and a follow-up question is joined to its earlier question. The numbers are not meaningful as retrieval quality. They show that the harness works.

## How a question is scored

- The search query of a question is the question, cleaned and cut like the API does. A `followup` question uses the cached rewrite of its `context`.
- A chunk is relevant when its page range includes a labeled page. A chunk that includes only a page next to a labeled page has gain 1 for nDCG (the labeled page has gain 2).
- The ranked lists are ten results deep, so that nDCG@10 has ten places. The API returns the first eight.
- Dense only and BM25 only use the same gate rule and the same fusion code on one list. A covered question that the gate cuts off has no results, so it counts as a miss. The false-gate rate shows how often that happens.
- A `not_covered` question is right when the gate gives no sources.

## The golden set

Each line of `golden.jsonl` is `{ "id", "question", "relevantPages", "kind" }`, and a `followup` also has `context` (the earlier question). The kinds are `lookup`, `paraphrase`, `notation`, `followup`, and `not_covered`. The set needs at least 80 questions and at least 10 of each kind. The tests check this.

Every question was written from the text of the pages and checked against the page. `not_covered` questions are plausible student questions that the excerpt does not answer. To add a question, find the page in the PDF, write the question and its page numbers, and run `offline --dev-cache` to see that the file still loads.
