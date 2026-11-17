# Openbook

Openbook is an open-source platform for AI tutoring that is grounded in your course materials.
A teacher uploads course PDFs. Students ask questions and get tutoring answers that cite the exact pages they came from.
The model takes an open-book exam: it answers from the materials you give it, not from memory.

**Ask** is the Openbook web app for teachers and students, at [ask.steve.gg](https://ask.steve.gg).

## Status

Under active development. Phase 1 (Rust API, course model, streaming chat, local end-to-end tests) is in progress. See `docs/architecture.md` for the phases.

## Design in one minute

- Serverless and pay per use: an idle deployment costs cents. No search cluster, no always-on compute.
- Rust API on AWS Lambda behind CloudFront, DynamoDB, S3, Cognito, and Amazon Bedrock (Claude, with data kept in Australia).
- Hybrid retrieval: BM25 (Tantivy) plus dense vectors (Amazon S3 Vectors), fused with reciprocal rank fusion, one index per course.
- The whole system runs locally on [AWS Emulator](https://github.com/aws/aws-emulator), with no AWS account.

Read `docs/architecture.md`, `docs/api.md`, and `docs/local-dev.md` for the details, and `AGENTS.md` for how to work in this repository.

## Benchmarks

![Recall@8](https://img.shields.io/badge/dynamic/json?url=https%3A%2F%2Fraw.githubusercontent.com%2FnbdSteve%2Fopenbook%2Fmain%2Fevals%2Fbenchmarks.json&query=%24.retrieval.combined.hybrid.recallAt8&label=Recall%408&color=2f6f4f) ![MRR](https://img.shields.io/badge/dynamic/json?url=https%3A%2F%2Fraw.githubusercontent.com%2FnbdSteve%2Fopenbook%2Fmain%2Fevals%2Fbenchmarks.json&query=%24.retrieval.combined.hybrid.mrr&label=MRR&color=2f6f4f) ![False-gate rate](https://img.shields.io/badge/dynamic/json?url=https%3A%2F%2Fraw.githubusercontent.com%2FnbdSteve%2Fopenbook%2Fmain%2Fevals%2Fbenchmarks.json&query=%24.gate.combined.falseGateRate&label=False-gate%20rate&color=2f6f4f)

Retrieval is measured offline on excerpts of two openly licensed OpenStax books, one in mathematics and one in prose, with golden questions that were checked by hand against the pages. The method is in [`docs/evaluation.md`](docs/evaluation.md). The questions are the ones that the retrieval was tuned on, so the numbers are in-sample and a real course will do worse. The pooled rows weigh every question the same, so the larger corpus counts more. The badges read `evals/benchmarks.json` on the main branch, and `make benchmarks` updates the table below from it.

<!-- benchmarks:start -->
| Corpus | Questions | Setup | Recall@1 | Recall@5 | Recall@8 | nDCG@8 | MRR |
|---|---|---|---|---|---|---|---|
| All corpora | 180 | Hybrid | 0.8130 | 0.9593 | 0.9756 | 0.6129 | 0.8806 |
| All corpora | 180 | Dense only | 0.8293 | 0.9512 | 0.9675 | 0.6100 | 0.8861 |
| All corpora | 180 | BM25 only | 0.6585 | 0.9512 | 0.9675 | 0.5497 | 0.7778 |
| American Government 2e (OpenStax) | 73 | Hybrid | 0.8298 | 0.9574 | 0.9787 | 0.6552 | 0.8936 |
| American Government 2e (OpenStax) | 73 | Dense only | 0.8298 | 0.9574 | 0.9787 | 0.6405 | 0.8863 |
| American Government 2e (OpenStax) | 73 | BM25 only | 0.7447 | 0.9787 | 0.9787 | 0.6030 | 0.8429 |
| College Algebra (OpenStax) | 107 | Hybrid | 0.8026 | 0.9605 | 0.9737 | 0.5867 | 0.8726 |
| College Algebra (OpenStax) | 107 | Dense only | 0.8289 | 0.9474 | 0.9605 | 0.5911 | 0.8860 |
| College Algebra (OpenStax) | 107 | BM25 only | 0.6053 | 0.9342 | 0.9605 | 0.5168 | 0.7376 |

| Corpus | Gate accuracy | False-gate rate | Off-topic questions caught | Not-covered questions caught |
|---|---|---|---|---|
| All corpora | 0.9000 | 0.0000 | 1.0000 | 0.4545 |
| American Government 2e (OpenStax) | 0.9178 | 0.0000 | 1.0000 | 0.5714 |
| College Algebra (OpenStax) | 0.8879 | 0.0000 | 1.0000 | 0.3684 |
<!-- benchmarks:end -->

## License

[GNU Affero General Public License v3.0](LICENSE).
