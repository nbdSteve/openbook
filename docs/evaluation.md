# Retrieval evaluation (phase 4)

Retrieval quality decides answer quality. This phase measures it with fixed data, protects it in CI, and gives the numbers that tune the chunker, the fusion, and the relevance gate.

## What we measure

- **Recall@5 and Recall@8**: the share of questions where at least one relevant chunk is in the top 5 or the top 8 fused results. The chat prompt uses the top 8.
- **nDCG@8**: ranking quality of the list that the API returns, with graded relevance (2 for the labeled page, 1 for an adjacent page). Recall, MRR, and nDCG all read that list.
- **MRR**: the mean of 1 / rank of the first relevant chunk.
- **Gate accuracy**: questions that the course does not cover must be gated (no sources), and covered questions must not be. There are two kinds of uncovered questions: `not_covered` (nearby math that the excerpt does not teach, the hard negatives) and `off_topic` (no relation to the course: general knowledge, other subjects, small talk). Report the false-gate rate (covered questions that are cut off) and the missed-gate rate (uncovered questions that get sources) separately, and the missed-gate rate for `not_covered`, for `off_topic`, and for both together.
- **Gate inputs**: for every question the report holds what the gate reads: the best dense distance, the top BM25 score, and the lexical term coverage (the share of the distinct analyzed query terms that the best BM25 hit holds, and the number of query terms that exist in the index). A sweep over candidate gate rules and thresholds is computed from them.
- **Latency** (live mode only): p50 and p95 for dense, lexical, and the whole retrieval step.

A chunk is relevant when its page range includes a labeled page. Every metric is reported for dense only, BM25 only, and hybrid.

## Data

- `evals/corpus/`: an excerpt of an openly licensed mathematics textbook (OpenStax, CC BY 4.0), about 40 pages that cover quadratics, functions, and exponents, with an `ATTRIBUTION.md` that names the source, the licence, and the changes (page selection only). The excerpt has real textbook layout: headings, worked examples, exercises, figures, and display math.
- `evals/golden.jsonl`: one question per line: `{ "id", "question", "relevantPages": [n], "kind" }`. `kind` is one of `lookup` (close to the book's words), `paraphrase` (different words), `notation` (written in math notation, for example `x^2 - 5x + 6 = 0`), `followup` (needs the previous question, given in `context`), and `not_covered` (a nearby math topic that the excerpt does not teach), and `off_topic` (no relation to the course). A question is `not_covered` only when no page teaches the answer: a mention in a chapter outline or in a list of learning objectives does not count as teaching. At least 80 questions, at least 10 of each kind. Draft questions may come from a model, but every question is checked by hand against the page before it goes in.
- `evals/corpus/sections.json`: splits the excerpt into five sections. The evaluation treats each section as one document, so the per-document cap works as it does in a course with several PDFs. The `--cap-off` mode gives every chunk the same document ID, so the cap sees one document and never skips a chunk. It changes only the document ID: the chunks keep the section boundaries and the section titles stay in the embedded text, so it is not a real one-PDF course. It isolates the effect of the cap. Every report shows both modes. The evaluation does not measure source diversity (how many documents the sources come from), which is what the cap is for.
- `evals/cache-dev/`: a development cache (PDF text layer and fake embeddings) that shows the harness works. It is never used for the baseline, and `--check` refuses it.
- `evals/cache/`: the inputs that make the offline mode deterministic: the page Markdown that real ingestion produced for the corpus (Claude transcription included), and the embeddings of every chunk and every question (keyed by embedding model and the SHA-256 of the exact embedded text, stored as compressed 16-bit floats).

## Modes

- **Offline** (`make eval`): chunk the cached pages with the real chunker, build the real BM25 index and an in-memory dense index from the cached embeddings, run the real retrieval and fusion code for every golden question, and print the metrics table. No AWS, no network, deterministic, a few seconds. A missing cache entry is an error, not a silent skip.
- **Grid** (`make eval-grid`): runs every combination of the tuning knobs (see "Tuning") offline and writes a table to `target/eval/grid.md` and `grid.json`. No AWS.
- **Live** (`make eval-live` with `AWS_PROFILE` and `AWS_REGION=ap-southeast-2`): ingest the corpus PDF through the real worker code with real Bedrock transcription and Titan embeddings, query real S3 Vectors in the dev stack, measure latency, and refresh `evals/cache/` (the diff is reviewed and committed like code). The first version of the refresh runs the pipeline on this machine and does not query S3 Vectors: it records the time of the Titan query embedding calls, and the S3 Vectors and BM25 latency measurement is still to do. The cache holds the chunk embeddings of every chunk target of the grid (300, 400, and 600 tokens).
- **Incremental live refresh** (`make eval-live-missing`): reuses `evals/cache/pages`, does no transcription, and calls Bedrock only for the embeddings and the follow-up rewrites that the cache does not have (new or changed questions, and chunks of a new chunk target). It prints the number of calls of each kind and an upper bound of the cost first, and starts only with `YES=1`. Both live commands follow `AWS_REGION`: set it to `ap-southeast-2`.

## Protection in CI

`evals/baseline.json` holds the offline metrics of the committed code. The `checks` workflow runs `make eval` and fails when any hybrid metric drops by more than 0.01 (absolute) below the baseline, or when gate accuracy drops at all, or when the false-gate rate rises at all. It checks the run with the cap and the cap-off run, both with the same tolerance. `--check` cannot be combined with `--write-baseline`. A change that improves the metrics updates the baseline in the same commit.

## Tuning

`openbook-eval grid` runs the whole grid offline: RRF `k` 30, 60, and 100; final sources 6, 8, and 10; candidates per retriever 20 and 40; most sources per document 2, 3, 4, 6, and 8 (8 is the same as no cap); the weight of the BM25 list in the fusion 0.5 and 1.0; and the chunk target 300, 400, and 600 tokens. Each setting runs with the cap and with the cap off. `offline` takes the same knobs as flags (`--rrf-k`, `--final-sources`, `--dense-candidates`, `--lexical-candidates`, `--max-per-document`, `--lexical-weight`, `--chunk-tokens`, `--cap-off`) and the gate flags (`--gate any|dense|combined`, `--gate-distance`, `--gate-coverage`, `--gate-matched`) for one run. The production defaults stay as constants in `openbook-search`. The API never reads tuning flags.

Change a default only when hybrid Recall@8 or nDCG@8 improves without hurting gate accuracy. Results go to `evals/results/<date>.md`: one table per experiment, the chosen row marked, and short notes. Contextual retrieval with a short model-written context per chunk is a later experiment against this same harness.

### The gate

The gate decides whether a question has sources at all. A gate rule is data that the evaluation can sweep, not a branch in the API. The candidates are the dense distance only, and the dense distance or a strong lexical match (high term coverage and a minimum number of matched terms) for queries that embed badly, such as notation. The rule must have no false gate on the golden set, with a margin of at least 0.02 in distance beyond the worst covered question. Among the rules that pass, the one with the highest gate accuracy wins, and when two rules differ by one question or less, the simpler one wins. The golden set has 107 questions, so the rule does not fit many parameters. Round 1 chose the dense distance only, limit 0.78 for Titan v2 (`evals/results/2026-11-17.md`). "No false gate" is in-sample on the 76 covered questions, and the limit is 0.032 above the worst one. The API stores the real distances in `trace.gate` for the next round. The BM25 weight of the fusion must stay at 1.0 or more: a lower weight would let BM25 only re-order dense results, never add a source.

The distance limit belongs to one embedding model. Titan v2 has its own rule. The fake embedder (hashed bag of words, local runs and tests) and any model that has not been measured keep the first rule (any BM25 hit passes), because their distances have a different scale. `GateRule::for_embedding_model` picks the rule from the model ID of the embedder, so a Titan limit never gates the fake model.

The retrieval code that the API uses (query cleaning, fusion, the per-document cap, the gate) lives in `openbook-search`, so the evaluation runs exactly the production code.
