# Local development and end-to-end tests

The whole system runs on one machine with no AWS account: the web app, the API, and (phase 2) the ingest worker, with [AWS Emulator](https://github.com/aws/aws-emulator) in place of DynamoDB, S3, S3 Vectors, SQS, and Cognito.
Bedrock has no models in the emulator, so local runs use a deterministic fake model provider by default.

## What runs where

```
Browser or Playwright ─► http://localhost:5173  (Vite dev server, apps/ask)
                           ├─ /api/*   ─► http://127.0.0.1:8787  (openbook-api, local TCP listener)
                           └─ /__aws/* ─► https://127.0.0.1:9297 (AWS Emulator in Docker; Cognito calls and presigned S3 PUTs from the browser)
openbook-api ─► AWS Emulator (DynamoDB, S3, Cognito JWKS; phase 2: S3 Vectors, SQS)
             ─► fake models (default) or real Bedrock (OPENBOOK_BEDROCK_PROFILE=<profile>)
```

The browser never talks to the emulator directly, because the emulator's TLS certificate is new on every start.
The Vite proxy trusts it through `NODE_EXTRA_CA_CERTS`.

## Emulator settings

- Image: built from the latest `LTS/latest` AWS Emulator RPM (`scripts/emulator/Dockerfile`). Rebuild with `docker build --pull --no-cache` to upgrade. Never pin a version.
- Container: `openbook-emulator-<port>`, published on `127.0.0.1:<port>` only, never on all interfaces. Default port 9297 (`OPENBOOK_EMULATOR_PORT`).
- Authentication off (`-a off`). This is safe only because the port is on loopback. It is necessary because the Vite proxy changes the path of presigned S3 URLs, which breaks their signatures. Use any key pair (`test`/`test`).
- Telemetry: the emulator turns it on by default. The image sets `EMULATOR_TELEMETRY=off`, and the scripts pass `EMULATOR_TELEMETRY=off` unless you set `EMULATOR_TELEMETRY=on`.
- State is in memory only. Restart the container for a clean state, then run `make bootstrap` again.
- `scripts/emulator.sh up` creates the folder `~/.aws/emulator/<port>` (mode 700) before `docker run` and starts every run with a new, empty folder. The emulator fails with "Operation not permitted" when the folder holds the files of an earlier run. The container runs as your user (`--user $(id -u):$(id -g)`), because the folder is yours: on GitHub runners that is uid 1001, not the uid 1000 of the image. The script waits for `/__admin/health` with the new `ca.pem` before it returns.
- The bootstrap script never uses real AWS. It drops every `AWS_*` variable, sets test keys and the emulator endpoint, and refuses to run unless `aws sts get-caller-identity` returns account `000000000000`. The integration runner (`scripts/test-integration.sh`) drops every `AWS_*` variable too, and then passes only `OPENBOOK_TEST_EMULATOR_ENDPOINT` and `OPENBOOK_TEST_EMULATOR_CA` to the tests, so a test cannot reach any endpoint except the emulator it started.
- Use one Region (`us-east-1`) for everything that talks to the emulator.
- The API trusts the emulator through `OPENBOOK_CA_BUNDLE=~/.aws/emulator/<port>/ca.pem`, which it adds to the system roots of its AWS SDK and JWKS HTTP clients. Real Bedrock (hybrid mode) still works because the system roots stay. The file path stays the same across emulator restarts, but its contents change, so restart the API after you restart the emulator.

## Resources that `make bootstrap` creates

| Resource | Local name | Must match |
|---|---|---|
| DynamoDB table | `openbook-local` | Keys and index in `docs/architecture.md` (and the CDK table) |
| S3 docs bucket | `openbook-local-docs` | CORS: `PUT` from `http://localhost:<webPort>` |
| Cognito user pool and app client | `openbook-local` | Email sign-in, no client secret, `ALLOW_USER_PASSWORD_AUTH` and `ALLOW_REFRESH_TOKEN_AUTH` only (the emulator has no SRP, and the app uses one flow in every stage) |
| Seed users | `teacher@openbook.test`, `student@openbook.test` | Password `Openbook-local-1`, confirmed, `email_verified=true` |
| Phase 2: vector bucket, FIFO queue | `openbook-local-vectors`, `openbook-local-ingest.fifo` | |

It also writes two files for each stack, in a folder of its own (`.local/dev` for `make dev`, `.local/e2e` for `make e2e-local`):

- `<stack>/api.env`: the environment for `openbook-api`, written so that Bash can source it. It has stage `local`, `OPENBOOK_LISTEN_ADDR=127.0.0.1:<apiPort>`, `AWS_ENDPOINT_URL`, `AWS_REGION=us-east-1`, test keys, `AWS_CONFIG_FILE=/dev/null` and `AWS_SHARED_CREDENTIALS_FILE=/dev/null` (so no AWS profile can leak in), `OPENBOOK_CA_BUNDLE`, `OPENBOOK_COGNITO_REGION=us-east-1`, `OPENBOOK_COGNITO_JWKS_URL`, `OPENBOOK_PRESIGN_ENDPOINT=http://localhost:<webPort>/__aws`, and the model catalog (`OPENBOOK_MODELS`, `OPENBOOK_DEFAULT_CHAT_MODEL`, `OPENBOOK_FAST_MODEL`). The local catalog has two fake entries: `fake` (residency `au`, the default chat and fast model) and `fake-global` (residency `global`, to test the picker label). When `OPENBOOK_BEDROCK_PROFILE` is set, the file holds the deployed catalog from `infra/cdk.json` and that variable instead, and leaves the AWS file settings alone so that the profile can load.
- `<stack>/web-config.json`: the web runtime config, with `cognito.endpoint = "/__aws/"` and `region` `us-east-1`. The scripts set `OPENBOOK_WEB_CONFIG` to this file, and the Vite dev server serves it at `/config.json`. The endpoint is relative, so the file does not depend on the web port. Nothing writes `apps/ask/public/config.json` any more.

All of these are git-ignored. The script is safe to run twice: it finds the table, bucket, user pool, client, and users by name, and sets the seed passwords again.
`scripts/bootstrap-local.sh` takes `--emulator-port`, `--web-port`, `--api-port`, `--stack-dir` (default `.local/dev`), `--env-file`, `--web-config`, and `--daily-messages-per-user`. The defaults are the ones above.
The emulator supports `AdminCreateUser` and `AdminSetUserPassword`, so the seed users need no sign-up flow. The S3 Vectors bucket and the FIFO queue also work in the emulator today, so the script creates them.

## Commands

| Command | What it does |
|---|---|
| `make emulator-up` / `make emulator-down` | Build (if needed) and start, or stop, the emulator container |
| `make bootstrap` | Create the local resources and write the files above |
| `make dev` | Emulator, bootstrap, API, and Vite together; Ctrl-C stops all of them |
| `make e2e-local` | Start an isolated stack on other ports, run the Playwright suite headless, and stop everything |
| `make test-integration` | Start an emulator on port 9320, run `cargo test --workspace -- --ignored` against it (`OPENBOOK_TEST_EMULATOR_ENDPOINT`, `OPENBOOK_TEST_EMULATOR_CA`), and stop it |
| `OPENBOOK_BEDROCK_PROFILE=<profile> make dev` | Same as `make dev`, but with the deployed model catalog on real Bedrock through that AWS profile |

Ports for `make e2e-local` are separate from `make dev` (emulator 9310, API 8790, web 5180), and so are its files (`.local/e2e`), so both can run at the same time.
The e2e script sets the daily message limit to 8 per user in its API environment (the web flow tests send messages until they hit the quota), never uses real Bedrock, always starts a new emulator, and exits with the Playwright result after it stops every process.

`make dev` prefixes each log line with `[api]` or `[web]`. It stops the emulator on exit only when it started it. The first API start compiles the Rust workspace, so the wait for `/api/health` can take a few minutes.
Both scripts start the API and the web server with stdin from `/dev/null` (so that pressing Enter does not freeze Vite), start the API from an empty environment plus a short allowlist (so an exported `OPENBOOK_LIMIT_*` or `AWS_PROFILE` cannot change it), and send `SIGCONT` after `SIGTERM` when they stop.

## Fake model provider

The catalog entry with provider `fake` streams a fixed, deterministic answer in small deltas.
The answer repeats the student's question, uses Markdown, and includes one inline formula (`$a^2 + b^2 = c^2$`) and one block formula, so the UI rendering paths get exercised.
Token usage is estimated as one token per 4 characters.
Phase 2 and 3 add deterministic fake embeddings (hashed bag of words, 1024 dimensions, L2-normalized) and plain-text page extraction instead of model transcription.

## Sign-up locally

The emulator sends no email. Local end-to-end tests sign up through the UI, then confirm the user with `AdminConfirmSignUp` through the emulator (a Playwright helper), then sign in.
For manual use, sign in with a seed user.
