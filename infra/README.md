# Openbook infrastructure

The AWS CDK app (TypeScript) for Openbook. It creates the `Openbook-<stage>` stack in `ap-southeast-2` (`dev` or `prod`), and an optional certificate stack in `us-east-1` for a custom domain. Everything is pay per use (see `docs/architecture.md`).

## Commands

| Command | What it does |
|---|---|
| `npm run lint` | ESLint |
| `npm run typecheck` | `tsc --noEmit` |
| `npm run build` | `tsc` to `dist/` |
| `npm test` | Vitest: the security tests, the IAM template ties, the exact policy snapshots, the workflow checks |
| `npm run synth:placeholders` | `cdk synth` with the fixture assets in `test/fixtures` and no AWS identity (the script drops every `AWS_*` and `CDK_DEFAULT_*` variable and points the SDK at nothing) |
| `npm run render:iam -- --example` | Render the IAM templates (see `iam/README.md`) |

From the repository root, `make synth`, `make deploy STAGE=dev`, and `make check` wrap these.

## Context

The CDK app reads all settings from context (`cdk.json` or `-c key=value`). Nothing account-specific is committed. The stack is account-agnostic unless `CDK_DEFAULT_ACCOUNT` is set, and it then names the account through pseudo parameters.

| Key | Default | Meaning |
|---|---|---|
| `stage` | `dev` | `dev` or `prod`. Prod retains data, has point-in-time recovery, deletion protection, termination protection, the permissions boundary, and 25 reserved concurrent executions. |
| `apiAssetDir` | `../target/lambda/openbook-api` | The directory with the Lambda `bootstrap` binary |
| `ingestAssetDir` | `../target/lambda/openbook-ingest` | The directory with the worker's `bootstrap` and `libpdfium.so`. Synth refuses a directory without both. `make build-lambdas` makes it. |
| `transcribeModel`, `embeddingModelId` | the fast model, `amazon.titan-embed-text-v2:0` | The catalog ID that transcribes pages (it must have `vision: true` and residency `au`, or synth fails), and the on-demand embedding model in Sydney |
| `webAssetDir` | `../apps/ask/dist` | The built web app. Synth refuses an `index.html` with an inline `<script>`, because the CSP blocks it. |
| `models`, `defaultChatModel`, `fastModel` | in `cdk.json` | The model catalog (`docs/architecture.md`, "Model catalog"). Synth fails if it breaks a rule, including the model ID allowlist. The Bedrock IAM policy comes from it. |
| `limitPagesPerDocument`, `limitDailyTranscribedPagesPerUser`, `limitDailyTranscribedPagesGlobal` | `stageLimits.<stage>`, then the API default | The page and transcription limits. Until the public launch, `cdk.json` caps the global transcription at 300 pages a day in prod and 100 in dev. |
| `limitDailyMessagesPerUser`, `limitDailyMessagesGlobal`, `limitDailyTokensGlobal`, `limitFailedJoinsPerDay`, `limitCoursesOwned`, `limitDocumentsPerCourse`, `limitUploadBytes` | `stageLimits.<stage>` in `cdk.json`, then the API default | The `OPENBOOK_LIMIT_*` values. Until the public launch, `cdk.json` caps the global spend: prod 300 messages and 300,000 tokens a day, dev 100 and 100,000 (the API defaults of 500 and 3,000,000 allow more than the US$20 budget). For the launch, delete `stageLimits` or override with `-c limitDailyMessagesGlobal=...`. |
| `domainName`, `hostedZoneName`, `hostedZoneId` | unset | A custom domain. All three are needed. The hosted zone ID avoids a lookup at synth time. |

## The ingest worker

`openbook-ingest` (docs/retrieval.md, "Ingestion (phase 2)") is a second Lambda function of the stack, with its own role and log group (stage retention):

- arm64, `provided.al2023`, 3008 MB, 900 s, 4096 MB of `/tmp`, reserved concurrency 5 (prod) or 2 (dev).
- Fed by the FIFO queue `Openbook-<stage>-ingest.fifo` (visibility 960 s, retention 4 days, dead-letter queue `Openbook-<stage>-ingest-dlq.fifo` after 5 receives (the worker gives up after 3 and marks the document failed; the extra receives cover a crash), SQS-managed encryption, TLS only) through an event source mapping with batch size 1, partial batch responses, and a maximum concurrency equal to the reserved concurrency (so the poller never throttles on the function and burns a receive count).
- The S3 Vectors bucket `openbook-<stage>-vectors` (SSE-S3; retained in prod). The worker creates one index per course.
- Both functions get the same environment, with `OPENBOOK_INGEST_QUEUE_URL`, `OPENBOOK_VECTOR_BUCKET`, `OPENBOOK_EMBEDDING_MODEL_ID`, `OPENBOOK_TRANSCRIBE_MODEL`, and the page and transcription limits. A test checks the environment against the 4 KB limit of Lambda.
- Permissions (exact statements are in `test/snapshots/stack-policies.prod.json`): the API sends to the queue (`sqs:SendMessage`), queries course indexes (`QueryVectors`, `GetVectors`, for phase 3), and embeds with Titan. It never deletes an index or an S3 object (the worker does all deletes). The worker consumes from the queue, uses the table and the docs bucket, creates and deletes course indexes and puts and deletes vectors in the stage bucket (it never queries), and invokes only the transcribe model and Titan. Every index action is authorized on the index ARN `bucket/<name>/index/c-*`, including `CreateIndex`. Every role has the permissions boundary.
- `libpdfium.so` must sit next to the worker's `bootstrap`. `make build-lambdas` copies it (pinned by SHA-256 in `scripts/fetch-pdfium.sh`). The deploy job checks the assembly for exactly two `bootstrap` files, both executable, and one `libpdfium.so` beside the worker's.
- The catalog entries have a `vision` flag (the Claude entries are `true`, gpt-oss is `false`).

## Deploy

1. Bootstrap the account and Region once with `cdk bootstrap` (qualifier `hnb659fds`).
2. Apply the IAM documents in `iam/` once, out of band (`iam/README.md`).
3. Dev: `AWS_PROFILE=<profile> make deploy STAGE=dev`.
4. Prod: push to `main`. `.github/workflows/deploy.yml` deploys it.

The deploy workflow has three jobs. `checks` runs `checks.yml` on the exact commit. `build` has no OIDC token: it builds the Lambda and the web app, runs the tests, runs `cdk synth -c stage=prod` with no AWS identity, packs `infra/cdk.out` into one tar file (the artifact actions save every file as mode 644, which would remove the execute bit of the Lambda `bootstrap`), and records the tar's SHA-256 as a job output. `deploy` is the only job with `id-token: write` and the `production` environment. It runs no build and no test. It downloads the tar, compares the SHA-256, checks the tar entries (only plain files and directories under `cdk.out/`), unpacks it, checks that `bootstrap` is executable, and validates the assembly with `scripts/validate-assembly.js` (from the checked-out commit) before it asks for the token. Then it runs `npm ci --ignore-scripts` for the pinned CDK CLI and `cdk deploy Openbook-prod --app cdk.out --role-arn <execution role>`.

The hash protects the transport only. The assembly is build-controlled input, and the CDK CLI runs the `source.executable` of an asset, if the asset manifest has one, with the full environment. So the deploy job does read output of code that the build job ran. `validate-assembly.js` is the control for that: it accepts only the exact shape of the stack (the four artifacts, an account-agnostic environment, no role or lookup field, no docker image, file and zip sources with a `path` and nothing else, no path that leaves the assembly, no symbolic link, the bootstrap bucket and the `openbook-prod/` prefix, and a template asset that is the hashed template file) and refuses everything else. A CDK upgrade that adds an artifact or a field fails there until you review the change and extend the allowlist. The job does not run the build's pip, npm, cargo, or Vite code itself.

`make deploy STAGE=prod ALLOW_PROD=1` exists for recovery only. CDK writes assets under the key prefix `openbook-<stage>/` of the bootstrap bucket, and the deploy role may write only under `openbook-prod/`.

## GitHub settings

- Environment `production`, with a deployment branch policy that allows `main` only.
- Environment secrets (not variables, because this repository and its logs are public): `AWS_ACCOUNT_ID` (the twelve digits), `AWS_DEPLOY_ROLE_ARN` (the full ARN of the deploy role), and `AWS_CLOUDFORMATION_EXECUTION_ROLE_ARN` (the full ARN of the execution role). There are no variables. The workflow masks the account ID and sends the `cdk deploy` output to a file, and only a filtered summary reaches the log.
- Branch protection on `main` that requires a pull request and the `checks` jobs. The deploy workflow runs the checks again on the commit it deploys, so a direct push cannot skip them.
- No AWS access key anywhere. The OIDC token is the only credential.

## Notes

- Cognito uses the default email sender. It allows about 50 emails a day. Add Amazon SES before public sign-up.
- Reserved concurrency needs headroom: Lambda keeps 100 units of the account quota unreserved. The account needs a quota of at least 100 plus every reserved total (25 for prod, 5 for dev). A new account has a quota of 10, which is too low. This account has 400.
- The docs bucket CORS allows only the CloudFront domain and the custom domain. Local development goes through the Vite proxy (`/__aws`), so no localhost origin is allowed in any stage.
- The docs bucket has a fixed name, `openbook-<stage>-docs-<account>-<region>`. A fixed name avoids a dependency cycle between its CORS rule (which names the CloudFront domain) and the distribution (which names the bucket in the CSP and the function).
- Only the hashed files under `assets/` get the one-year immutable cache. Every other file, the favicon included, is sent with `no-cache`.
- The two bucket-deployment helper functions share one log group with the stage retention.
- `cdk.json` sets `cli-telemetry` to `false`.
