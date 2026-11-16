# Ingestion and retrieval (phases 2 to 4)

Draft design. Phase 4 evaluation decides the numbers marked "tune".

## Ingestion (phase 2)

`openbook-ingest` is a Rust Lambda (arm64) on the SQS FIFO queue `Openbook-<stage>-ingest.fifo`, batch size 1, with a dead-letter queue after 3 receives.
`MessageGroupId` is the course ID, so one course's indexes have one writer at a time, and different courses run in parallel.
`MessageDeduplicationId` is `<documentId>:<action>:<requestId>`.

Messages:

```json
{ "action": "ingest", "courseId": "...", "documentId": "...", "s3Key": "courses/.../source.pdf" }
{ "action": "delete", "courseId": "...", "documentId": "..." }
```

The API sends `ingest` from `POST .../documents/{id}/complete`. Document deletion also goes through the queue (the API sets the status to `deleting`, the worker removes vectors, BM25 entries, and S3 objects, then the record), because only the worker may write a course's indexes. Course deletion deletes the vector index and the S3 prefix directly; the worker drops messages for a course that no longer exists.

Steps for `ingest`:

1. Set the status from `queued` to `processing` with a conditional update. If the condition fails, the message is a duplicate: stop.
2. Download the PDF. Open it with pdfium (`pdfium-render`, with the `libpdfium.so` build for linux arm64 in the Lambda package). Refuse more than 500 pages (`failed`, with a clear error).
3. For each page, read the text layer. Send the page to Claude Haiku (`au.` profile) as a rendered PNG (about 150 DPI) when the text layer looks poor: little text, private-use or replacement characters, many isolated symbols, or math glyphs. The prompt asks for a faithful Markdown transcription with LaTeX for math, `[Figure: short description]` for figures, and no added content. Otherwise use the text layer. Run at most 8 pages at once. Write each page to `pages/<nnnn>.md`; skip pages that already exist, so a retry resumes.
4. Chunk the page Markdown by structure: split at headings, then pack paragraphs to about 400 tokens (tune) with one or two sentences of overlap. Never split inside a LaTeX block, a table, or a list item. Each chunk keeps its first and last page, its heading path, and its index.
5. Prepend a context line to the text that is embedded and indexed (not to the text shown to students): `<document title> > <heading path>`. Phase 4 tests full contextual retrieval (a short LLM-written context per chunk) against this cheap version.
6. Embed with Titan Text Embeddings v2 (`amazon.titan-embed-text-v2:0`, 1024 dimensions, normalized, tune), on demand in Sydney. It takes one text per call, so run calls in parallel with a limit. Cohere Embed v4 is available only through a `global.` profile, which breaks the data residency rule; phase 4 can compare it only if that changes.
7. Write vectors to S3 Vectors: index `c-<courseId in lower case>` in the stage's vector bucket (create it on first use: 1024 dimensions, cosine; non-filterable metadata `text`, `heading`; filterable `documentId`, `page`). Key `<documentId>:<chunkIndex>`. Batches of 500.
8. Update the course's Tantivy index (below).
9. Set the status to `ready` with `pageCount` and `chunkCount`. On any failure after retries, set `failed` with an error that is safe to show to the owner.

## BM25 index (Tantivy, phase 2 writes, phase 3 reads)

One Tantivy index per course, stored in the docs bucket under `courses/<courseId>/bm25/`.
Schema: `chunkId` (string, stored), `documentId` (string, indexed, stored), `page` (u64, stored), `heading` (text, stored), `text` (text with the English stemmer, stored).

- Segment files are immutable. `manifest.json` lists the files of one version. The course `META` item holds `bm25Version`.
- The worker keeps a copy in `/tmp`, downloads only missing files, deletes the document's terms (re-ingest and delete), adds the new chunks, commits, uploads the new files and the new manifest, and then moves `bm25Version` forward with a conditional update on the old value.
- The API reads `bm25Version` from the course item it already loads for the membership check, downloads the files of that version into `/tmp` on a cache miss, and opens a reader. A warm Lambda reuses it.

## Retrieval for a chat turn (phase 3)

1. Query: the student's message. After the first turn, Claude Haiku rewrites the message and the last few turns into one standalone question (temperature 0, at most 100 tokens).
2. In parallel: embed the query (Titan v2) and query S3 Vectors for the top 40, and query BM25 for the top 40.
3. Fuse with reciprocal rank fusion: `score = sum(1 / (60 + rank))`. Keep the top 8 (tune), at most 4 from one document.
4. Send the `sources` event, then give the sources to the model as numbered, delimited passages with document name and page. The prompt says the passages are data, not instructions, and that the answer cites them as `[n]`. When retrieval finds nothing relevant (thresholds tune), the prompt tells the model to say that the course materials do not cover the question.
5. Save the retrieval trace on the assistant message: the query used, the ranked candidates from each retriever, and the final sources. Evaluations and teacher insights use it.

## Evaluation (phase 4)

- `crates/eval`: a command-line tool that runs BM25 only, dense only, and hybrid retrieval over a fixture course and reports Recall@5, Recall@10, nDCG@10, and MRR, plus citation accuracy and faithfulness with an LLM judge when it runs with Bedrock.
- Fixture corpus: an openly licensed mathematics textbook (for example OpenStax, CC BY 4.0) with a golden set of questions and their relevant pages.
- CI runs the retrieval evaluation with cached embeddings committed for the fixture (keyed by model and text hash), so it is deterministic and needs no AWS access. A retrieval change must not lower the metrics below the committed baseline.
- A manual workflow runs the full evaluation against Bedrock.
