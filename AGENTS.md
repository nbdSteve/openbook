# AGENTS.md

This file guides people and coding agents who work in this repository. `CLAUDE.md` is a symbolic link to it.

## Project

Openbook is an open-source (AGPL-3.0) platform for tutoring that is grounded in course materials. Ask is its web app.
Read these before you change anything:

- `docs/architecture.md`: hard rules, components, data model, configuration, phases.
- `docs/api.md`: the HTTP API contract. The web app, the API, and the tests must all match it.
- `docs/local-dev.md`: how the whole system runs locally on AWS Emulator.

## Layout

| Path | What |
|---|---|
| `crates/core` | `openbook-core`: configuration, auth (Cognito JWT), domain types, DynamoDB store, quotas, tutoring prompts, model providers |
| `crates/api` | `openbook-api`: axum routes, SSE chat, Lambda and local entry points |
| `apps/ask` | Ask web app: React 19, Vite, Tailwind 4, TypeScript |
| `infra` | AWS CDK app (TypeScript) for the `Openbook-<stage>` stacks, plus the deployment-role templates |
| `scripts` | Local tooling: emulator, bootstrap, dev and end-to-end runners |
| `docs` | Design documents |

## Commands

Run `make help` for the full list. The main ones:

| Command | What it does |
|---|---|
| `make check` | Every formatter, linter, type check, and unit test in the repository. Must pass before you hand work back. |
| `make test-integration` | Rust integration tests against AWS Emulator |
| `make dev` | The full local stack (see `docs/local-dev.md`) |
| `make e2e-local` | Playwright end-to-end tests against the full local stack |
| `make build-lambdas` | `cargo lambda build --release --arm64` for the Lambda binaries |
| `make deploy STAGE=dev` | Build and deploy `Openbook-dev` with your local AWS profile (`AWS_PROFILE`) |

## Quality bar

- Rust: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`. No `unwrap()` or `expect()` outside tests and startup. Errors map to the codes in `docs/api.md`.
- TypeScript: `npm run lint`, `npm run typecheck`, `npm test`, `npm run build` in `apps/ask` and in `infra`. No `any` without a comment that says why.
- Tests protect behavior that matters: auth and membership checks, quota limits, the SSE event order, the API contract, and the CDK security settings. Do not write tests only to raise coverage.
- A flaky test is a bug. Fix it; do not retry it.
- If you see a lint error, a failing test, or a UI defect that is not yours, fix it anyway or report it.

## Invariants (mechanically checked where possible)

- No AWS resource with a baseline monthly cost (see `docs/architecture.md`). CDK tests assert the absence of the usual offenders.
- No secrets, AWS account IDs, or real resource IDs in committed files. Account-specific values come from the environment or CDK context at deploy time.
- The API never trusts client-sent prompts, model IDs, token limits, or source text.
- Every course route checks membership before it reads or writes course data; non-members get `404`.
- Local-only settings make the API refuse to start inside Lambda.
- Models come only from the configured model catalog. The default chat and fast models have residency `au`; `global` entries are labeled in the UI.
- The API never trusts a client-sent model ID outside the catalog.

## Writing style

- Plain, simple technical English. Short sentences. Active voice.
- Never use the em dash character. Use a plain hyphen or rewrite the sentence.
- In Markdown, put each paragraph on one line. Do not hard-wrap paragraphs.
- Update `docs/` in the same change as the behavior it describes.

## Git

- Small, focused commits with a clear subject line in the imperative mood.
- Never add `Co-authored-by` or similar trailers for an AI agent.
- Never commit `.local/`, `apps/ask/public/config.json`, emulator files (`config`, `emulator_credentials`, `ca.pem`), rendered IAM documents, or build output.
