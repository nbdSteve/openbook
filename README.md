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
<!-- benchmarks:end -->

## License

[GNU Affero General Public License v3.0](LICENSE).
