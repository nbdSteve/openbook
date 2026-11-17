# Openbook

Openbook is an open-source platform for AI tutoring that is grounded in your course materials.
A teacher uploads course PDFs. Students ask questions and get tutoring answers that cite the exact pages they came from.
The model takes an open-book exam: it answers from the materials you give it, not from memory.

**Ask** is the Openbook web app for teachers and students, at [openbook.steve.gg](https://openbook.steve.gg).

## Status

Under active development. Phase 1 (Rust API, course model, streaming chat, local end-to-end tests) is in progress. See `docs/architecture.md` for the phases.

## Design in one minute

- Serverless and pay per use: an idle deployment costs cents. No search cluster, no always-on compute.
- Rust API on AWS Lambda behind CloudFront, DynamoDB, S3, Cognito, and Amazon Bedrock (Claude, with data kept in Australia).
- Hybrid retrieval: BM25 (Tantivy) plus dense vectors (Amazon S3 Vectors), fused with reciprocal rank fusion, one index per course.
- The whole system runs locally on [AWS Emulator](https://github.com/aws/aws-emulator), with no AWS account.

Read `docs/architecture.md`, `docs/api.md`, and `docs/local-dev.md` for the details, and `AGENTS.md` for how to work in this repository.

## License

[GNU Affero General Public License v3.0](LICENSE).
