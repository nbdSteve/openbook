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
- Telemetry: the emulator turns it on by default. Our scripts start it with `EMULATOR_TELEMETRY=off` unless you set `EMULATOR_TELEMETRY=on`.
- State is in memory only. Restart the container for a clean state, then run `make bootstrap` again.
- Use one Region (`us-east-1`) for everything that talks to the emulator.
- The API trusts the emulator through `OPENBOOK_CA_BUNDLE=~/.aws/emulator/<port>/ca.pem`, which it adds to the system roots of its AWS SDK and JWKS HTTP clients. Real Bedrock (hybrid mode) still works because the system roots stay. The file path stays the same across emulator restarts, but its contents change, so restart the API after you restart the emulator.

## Resources that `make bootstrap` creates

| Resource | Local name | Must match |
|---|---|---|
| DynamoDB table | `openbook-local` | Keys and index in `docs/architecture.md` (and the CDK table) |
| S3 docs bucket | `openbook-local-docs` | CORS: `PUT` from `http://localhost:<webPort>` |
| Cognito user pool and app client | `openbook-local` | Email sign-in, no client secret, `ALLOW_USER_SRP_AUTH`, `ALLOW_USER_PASSWORD_AUTH`, `ALLOW_REFRESH_TOKEN_AUTH` |
| Seed users | `teacher@openbook.test`, `student@openbook.test` | Password `Openbook-local-1`, confirmed |
| Phase 2: vector bucket, FIFO queue | `openbook-local-vectors`, `openbook-local-ingest.fifo` | |

It also writes:

- `.local/api.env`: the environment for `openbook-api` (stage `local`, `AWS_ENDPOINT_URL`, `AWS_REGION=us-east-1`, test keys, `OPENBOOK_CA_BUNDLE`, `OPENBOOK_COGNITO_REGION=us-east-1`, `OPENBOOK_COGNITO_JWKS_URL`, `OPENBOOK_PRESIGN_ENDPOINT=http://localhost:<webPort>/__aws`, and a model catalog with one `fake` entry as the default chat and fast model).
- `apps/ask/public/config.json`: the web runtime config with `cognito.endpoint = "http://localhost:<webPort>/__aws/"`.

All of these are git-ignored.

## Commands

| Command | What it does |
|---|---|
| `make emulator-up` / `make emulator-down` | Build (if needed) and start, or stop, the emulator container |
| `make bootstrap` | Create the local resources and write the files above |
| `make dev` | Emulator, bootstrap, API, and Vite together; Ctrl-C stops all of them |
| `make e2e-local` | Start an isolated stack on other ports, run the Playwright suite headless, and stop everything |
| `OPENBOOK_BEDROCK_PROFILE=<profile> make dev` | Same as `make dev`, but with the deployed model catalog on real Bedrock through that AWS profile |

Ports for `make e2e-local` are separate from `make dev` (emulator 9310, API 8790, web 5180), so both can run at the same time.

## Fake model provider

The catalog entry with provider `fake` streams a fixed, deterministic answer in small deltas.
The answer repeats the student's question, uses Markdown, and includes one inline formula (`$a^2 + b^2 = c^2$`) and one block formula, so the UI rendering paths get exercised.
Token usage is estimated as one token per 4 characters.
Phase 2 and 3 add deterministic fake embeddings (hashed bag of words, 1024 dimensions, L2-normalized) and plain-text page extraction instead of model transcription.

## Sign-up locally

The emulator sends no email. Local end-to-end tests sign up through the UI, then confirm the user with `AdminConfirmSignUp` through the emulator (a Playwright helper), then sign in.
For manual use, sign in with a seed user.

## Run the browser flows against a deployed stack

The same flows in `apps/ask/e2e/flows` can run against a deployed stack, for example `Openbook-dev`.
They need AWS credentials that can call `cognito-idp:AdminCreateUser`, `AdminSetUserPassword`, and `AdminDeleteUser` on that stack's user pool.

```
cd apps/ask
npx playwright install chromium   # once
AWS_PROFILE=<profile> E2E_BASE_URL=https://<stack>.cloudfront.net npm run e2e:deployed
```

What changes in deployed mode (`E2E_TARGET=deployed`):

- The user pool ID and Region come from `<E2E_BASE_URL>/config.json`, and the AWS SDK uses the default credential chain with the real endpoint.
- Users come from `AdminCreateUser` with `MessageAction: SUPPRESS` and `AdminSetUserPassword`, then sign in through the UI. The suite sends no email. The UI sign-up flow stays in local mode only.
- The model flow switches to `claude-haiku` and expects no global note. The quota flow is skipped, because it needs the local limit of 8.
- Answers come from a real model, so the flows check structure (streamed text, a rendered formula, the message after a reload), not exact text.
- After the run, the suite deletes the course through the UI and every user it created (`AdminDeleteUser`), also when a test fails. Conversations of those users stay in the table until their time to live ends.
- Every flow fails on a `securitypolicyviolation` event, a console error, or a page error. The deployed site sends the production Content-Security-Policy.

Do not run this against production without a plan for the test users and courses it creates.
