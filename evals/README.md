# Retrieval evaluation

This directory holds the data of the retrieval evaluation. The design is in `docs/evaluation.md`. The code is the `openbook-eval` crate (`crates/eval`).

The evaluation runs the same retrieval code as the API (`openbook_search::retrieval`: query cleaning, reciprocal rank fusion, the soft per-document cap, the relevance gate), the real chunker, and the real BM25 index. The dense index is in memory, over embeddings from the cache.

## What is here

| Path | What |
|---|---|
| `corpora/<id>/` | One corpus. Every command runs all corpora. `--corpus <id>` runs one. |
| `corpora/college-algebra/` | 43 pages of the OpenStax book "College Algebra" (CC BY 4.0), a math corpus. |
| `corpora/american-government/` | 39 pages of the OpenStax book "American Government 2e" (CC BY 4.0), a prose corpus: the origins of the Constitution and American federalism. |
| `corpora/<id>/ATTRIBUTION.md` | The source, the licence, and the changes (page selection only). |
| `corpora/<id>/sections.json` | The sections of the excerpt. Each section is one document for the evaluation, so the per-document cap works as it does in a course with several PDFs. |
| `corpora/<id>/golden.jsonl` | The golden questions. Page numbers are pages of the excerpt. |
| `corpora/<id>/answers/` | The answer cache of the answer evaluation: answers and judgments (JSON lines), committed. |
| `answers-summary.json` | The pooled answer metrics of the committed prompt (schema 1). Written with `--write-summary`. |
| `corpora/<id>/cache/` | The live cache (made by `refresh`, reviewed and committed like code). |
| `corpora/<id>/cache-dev/` | The development cache. It proves that the harness works. It is never a baseline. |
| `corpora/<id>/corpus.json` | The title, the licence, and the source URL of the book, for `benchmarks.json`. |
| `benchmarks.json` | The public summary (schema 1) of the same numbers, for the README and the landing page. Written with the baseline. `--check` fails when it is stale. |
| `baseline.json` | The hybrid metrics of the committed code, from the live caches: one entry for each corpus and one for all corpora pooled. |

## Commands

Run all commands from the repository root. The `make` targets are short forms: `make eval-dev` is `offline --dev-cache`, `make eval` is `offline --check`, and `AWS_PROFILE=<profile> AWS_REGION=ap-southeast-2 make eval-live` (with `YES=1` to start) is the live refresh.

### Offline (no network, no AWS)

```
cargo run -p openbook-eval -- offline --dev-cache
```

This runs from the `cache-dev` of every corpus and prints the metrics table of all corpora pooled, then of each corpus. `--corpus <id>` runs one corpus (and `--cache DIR` then names its cache directory). It writes `target/eval/report.json` and `target/eval/report.md`.

```
cargo run -p openbook-eval -- offline
cargo run -p openbook-eval -- offline --check
```

The first command runs from the live cache of every corpus. The second one also compares the hybrid metrics with `evals/baseline.json` and exits with code 1 when a metric fell by more than 0.01 (absolute), or when gate accuracy fell at all, or when the false-gate rate rose at all. Every corpus and the pooled report are checked, each with the normal run and the cap-off run. The baseline and the run must have the same corpora (`--check` and `--write-baseline` refuse `--corpus`). `--check` cannot be combined with `--write-baseline`. A missing cache entry is an error. The command never skips a question.

When a change makes the metrics better, update the baseline in the same commit:

```
cargo run -p openbook-eval -- offline --write-baseline
```

`--check` and `--write-baseline` refuse a dev cache.

For tuning, `offline` takes these flags. They change the numbers for one run, and they cannot be used with `--check` or `--write-baseline`. The production numbers are in `crates/search/src/retrieval.rs`.

| Flag | What it changes |
|---|---|
| `--rrf-k N` | The `k` of reciprocal rank fusion |
| `--lexical-weight X` | The weight of the BM25 list in the fusion (the dense list has weight 1) |
| `--final-sources N` | How many sources the run returns |
| `--max-per-document N` | The soft per-document cap |
| `--dense-candidates N`, `--lexical-candidates N` | How many candidates each retriever returns |
| `--chunk-tokens N` | The chunk target (300, 400, and 600 are in the cache) |
| `--header-variant off\|keep-first\|drop-all` | The experimental removal of running headers (production: off, no removal). See `docs/retrieval.md`, "Running headers: the next approach" |
| `--cap-off` | Turn the per-document cap off: every chunk gets the same document ID (the chunks and their section titles do not change) |
| `--gate any\|dense\|combined` | The gate rule: any BM25 hit, the dense distance only, or the dense distance or a strong lexical match |
| `--gate-distance X`, `--gate-coverage X`, `--gate-matched N` | The numbers of the gate rule |

The report always shows the cap-off mode too, and the gate inputs and the gate sweep.

### The benchmark tables

```
cargo run -p openbook-eval -- readme
cargo run -p openbook-eval -- readme --check
```

`make benchmarks` is the short form of the first command. It writes the tables of `benchmarks.json` between the `<!-- benchmarks:start -->` and `<!-- benchmarks:end -->` markers of `README.md`. `--check` writes nothing and exits 1 when the block is stale; `make eval` runs it. The pooled rows weigh every question the same, so the larger corpus counts more.

### The grid

```
cargo run -p openbook-eval -- grid
```

`make eval-grid` is the short form. It runs every combination of RRF `k` (30, 60, 100), final sources (6, 8, 10), candidates (20, 40), most sources per document (2, 3, 4, 6, 8), BM25 weight (0.5, 0.75, 1.0, 1.5), chunk target (300, 400, 600 tokens), and header mode (off, keep-first, drop-all; production is off), each in both document modes, for each corpus and pooled, and writes `target/eval/grid.md` (one table per knob and scope, then the best settings) and `grid.json` (every row). `grid --tiny` runs three rows (the tests use it). It keeps the gate rule of the cache's embedding model, and it takes the gate flags. The chunk targets other than 400 need their chunk embeddings in the cache.

### Refresh the live cache (real AWS)

```
OPENBOOK_PDFIUM_PATH=$(scripts/fetch-pdfium.sh | tail -1)/lib AWS_PROFILE=<profile> AWS_REGION=ap-southeast-2 cargo run -p openbook-eval --release -- refresh
```

Set `AWS_REGION` yourself. The refresh follows `AWS_REGION` and then `AWS_DEFAULT_REGION`, and only without both does it use `ap-southeast-2`. A shell that exports another Region sends the calls to that Region.

This prints what it will call and about what it costs, and then stops. To start, add `--yes`. It does these steps with the real code of the ingestion worker:

1. Reads the corpus PDF with pdfium. A page with a poor text layer (math symbols, images) goes to the real Bedrock page transcription.
2. Rewrites each follow-up question with the fast model, with the real `rewrite_query` of the API.
3. Embeds every chunk and every question with Titan Text Embeddings v2.
4. Writes the `cache/` of the corpus (page Markdown, embeddings, rewrites, and the time of the query embedding calls).

It reads the normal AWS environment (`AWS_PROFILE`, `AWS_REGION`; the Region is `ap-southeast-2` only when none is set, so set `AWS_REGION=ap-southeast-2`). The model for transcription and rewrite is `au.anthropic.claude-haiku-4-5-20251001-v1:0` unless `OPENBOOK_EVAL_MODEL_ID` says another one. It does not touch S3, DynamoDB, SQS, or S3 Vectors. Review the diff of the caches, then run `offline --write-baseline` and commit both.

### Add what the cache lacks (real AWS, small)

```
AWS_PROFILE=<profile> AWS_REGION=ap-southeast-2 cargo run -p openbook-eval -- refresh --missing-only
```

`AWS_PROFILE=<profile> AWS_REGION=ap-southeast-2 make eval-live-missing` (with `YES=1` to start) is the short form. It fills what the caches of all corpora lack, in one plan. A corpus with a cache reuses its pages, does no page transcription, and gets Bedrock calls only for the Titan embeddings and the follow-up rewrites that its cache does not have: new or changed questions, the chunks of the other chunk targets, and chunks that changed with the chunker. A corpus without a cache gets the full live refresh (with the models of the other caches), so the command needs PDFium for it (`make eval-live-missing` fetches it). It prints the number of calls of each kind (and the ID of every follow-up question that it will rewrite) and an upper bound of the cost first, and starts only with `--yes`. A rewrite that returns nothing stops the run with an error; the raw question is never stored as the rewrite. It keeps the models that the cache was made with, so the cache keys stay valid, and it drops entries that no run needs.

### Refresh the development cache (no AWS)

```
OPENBOOK_PDFIUM_PATH=$(scripts/fetch-pdfium.sh | tail -1)/lib cargo run -p openbook-eval -- refresh --dev-cache
```

The pages are the PDF text layer, the embeddings are fake (hashed words), and a follow-up question is joined to its earlier question. The numbers are not meaningful as retrieval quality. They show that the harness works.

### Answer evaluation (real AWS)

```
AWS_PROFILE=<profile> AWS_REGION=ap-southeast-2 cargo run -p openbook-eval --release -- answers
AWS_PROFILE=<profile> AWS_REGION=ap-southeast-2 cargo run -p openbook-eval --release -- answers --yes
```

The first command prints the plan: the number of answer calls and judge calls, and an upper bound of the cost from the catalog prices. The second one starts. `make eval-answers` is the short form (`ARGS="..."` for options, `YES=1` to start). The design is in `docs/evaluation.md`, "Answer evaluation".

| Flag | What it does |
|---|---|
| `--kind LIST` | Only these kinds (a comma list of `covered`, `lookup`, `paraphrase`, `notation`, `followup`, `not_covered`, `off_topic`) |
| `--sample-covered N`, `--seed S` | A seeded sample of N covered questions across the selected corpora. The other kinds stay whole. A larger N keeps the smaller sample |
| `--style ID` | The tutoring style (`socratic`, `hints`, `step_by_step`, `concepts`, `direct`). The default is the product default |
| `--model ID`, `--judge-model ID` | Catalog IDs. The defaults are the default chat model and the fast model |
| `--catalog FILE` | The `cdk.json` with the model catalog (default `infra/cdk.json`) |
| `--corpus ID` | One corpus |
| `--write-summary` | Write `evals/answers-summary.json` (needs all questions and all corpora) |
| `--prune` | Remove the cached answers of the same style label and the same questions that this prompt does not use, and the judgments that no answer needs |
| `--no-turn-note` | Build the prompt without the server note before the student message (an ablation, cached under its own label) |
| `--check` | Offline: fail when the current prompt lacks a cached answer or judgment for a golden question, or when `answers-summary.json` is stale (`make eval` runs it) |

Numbers of the committed prompt (Socratic, the 180 in-sample questions, pooled), before the prompt change and after it:

| Metric | Before | After |
|---|---|---|
| Not covered: says "not covered" | 12.1% | 97.0% |
| Covered: grounded citation rate | 27.6% | 93.5% |
| Covered: citation precision | 90.5% | 92.3% |
| Covered: false "not covered" rate | 0.0% | 0.0% |
| Off topic: correct handling | 91.7% | 100.0% |

The Direct style on the in-sample `not_covered` questions went from 27.3% to 97.0%. The 20 held-out questions (out-of-sample) give 95% in both styles. `answers-summary.json` has the numbers of the committed prompt, and `results/2026-11-18.md` has the history, the ablation of the server note, and the held-out tables.

The answers and judgments are committed in `corpora/<id>/answers/`. A run with a cache that holds everything calls nothing and needs no `--yes`. The report is `target/eval/answers.md` and `answers.json`.

## How a question is scored

- The search query of a question is the question, cleaned and cut like the API does. A `followup` question uses the cached rewrite of its `context`.
- A chunk is relevant when its page range includes a labeled page. A chunk that includes only a page next to a labeled page has gain 1 for nDCG (the labeled page has gain 2).
- The API returns at most `final_sources` (8) chunks. Every metric reads that returned list, as it is.
- Dense only and BM25 only use the same gate rule and the same fusion code on one list. A covered question that the gate cuts off has no results, so it counts as a miss. The false-gate rate shows how often that happens.
- A `not_covered` or `off_topic` question is right when the gate gives no sources. The report gives the gate for covered questions, `not_covered`, `off_topic`, and both together.
- Recall@5, Recall@8, and MRR read the list that the API returns (`final_sources` long). nDCG@8 reads the first 8 places of the same list.

## The golden set

Each line of `golden.jsonl` is `{ "id", "question", "relevantPages", "kind" }`, and a `followup` also has `context` (the earlier question). The kinds are `lookup`, `paraphrase`, `notation`, `followup`, `not_covered`, and `off_topic`. An `off_topic` question can have `"smallTalk": true` (a greeting or thanks): the answer evaluation judges it by another rule. A question can have `"heldOut": true`: it was written after the prompt tuning, the answer evaluation reports it as out-of-sample, and the offline retrieval evaluation does not count it. A set needs at least 60 questions and at least 10 of each kind, except `notation`, which only a math corpus has. The tests check this.

Every question was written from the text of the pages and checked against the page. `not_covered` questions are plausible student questions about nearby math that the excerpt does not teach (hard negatives for the gate). A question is `not_covered` only when no page teaches the answer: a mention in a chapter outline or a list of learning objectives does not count. `off_topic` questions have no relation to the course: general knowledge, other school subjects, and small talk such as "hi" and "thanks, that makes sense". To add a question, find the page in the PDF, write the question and its page numbers, and run `offline --dev-cache` to see that the file still loads.
