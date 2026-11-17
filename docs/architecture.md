# Openbook architecture

Openbook is an open-source platform for tutoring that is grounded in course materials.
Ask (`apps/ask`, served at openbook.steve.gg) is its web app for teachers and students.
A teacher uploads course PDFs, and students get tutoring answers that cite those PDFs.
Think of it as an open-book exam for the model: it answers from the course materials, not from memory.

## Hard rules

1. **Pay per use only.** No component may have a baseline monthly cost. Do not add OpenSearch, NAT gateways, RDS or Aurora, ElastiCache, always-on containers, WAF web ACLs, Secrets Manager secrets, or customer-managed KMS keys. An idle stack must cost cents.
2. **The server owns the prompt.** Clients send a course ID, a conversation ID, and the student's message. The server picks the system prompt, the token limits, and the sources. The model comes from the deployment's model catalog: a course owner sets the course default, any member can pick another catalog model for their own chats, and nobody can use a model outside the catalog.
3. **Tenancy is the course.** Every read and write of course data checks membership first. A non-member gets `404`, never `403`. The one exception is the public demo route, which reads one demo course that has no members (see "Public demo").
4. **Data stays in Australia by default.** Use the `ap-southeast-2` Region. The default chat and fast models must have residency `au` (an `au.` inference profile, or a model that runs on demand in Sydney). A catalog may also offer `global.` models (for example the frontier GPT models, which Bedrock offers only through global profiles); they are marked `global`, and the web app tells the user who picks such a model that it may process data outside Australia. No other profile prefixes (`us.`, `apac.`, and so on).
5. **Same code locally and in Lambda.** The API is one axum router. Lambda runs it through `lambda_http`; locally it runs on a TCP listener. Local runs use AWS Emulator for AWS services.
7. **Known risk until the public launch: PDF parsing is not isolated.** pdfium (C++) parses untrusted PDFs in the ingest worker, which holds credentials for every course's files, the table, and every vector index. A pdfium memory-safety bug could therefore cross tenants. Before public sign-up (phase 5), PDF opening, text extraction, and page rendering move to a separate function with no data permissions (a presigned GET in, text and images out), and a scheduled check flags new pdfium releases.
6. **Fail closed.** If the API runs in Lambda with local-only settings (stage `local`, a catalog entry with provider `fake`, `AWS_ENDPOINT_URL`, `OPENBOOK_COGNITO_ISSUER`, `OPENBOOK_COGNITO_JWKS_URL`, `OPENBOOK_CA_BUNDLE`, `OPENBOOK_PRESIGN_ENDPOINT`, or `OPENBOOK_BEDROCK_PROFILE`), it must refuse to start.

## Components

```
Browser ── https://<site> ──► CloudFront
                               ├─ /*      ─► S3 web bucket (OAC). SPA routes rewrite to /index.html by a CloudFront Function.
                               ├─ /api/demo/* ─► Lambda function URL (OAC, AWS_IAM auth, RESPONSE_STREAM) ─► openbook-api in demo mode (own function, own role, reserved concurrency 3)
                               └─ /api/*  ─► Lambda function URL (OAC, AWS_IAM auth, RESPONSE_STREAM) ─► openbook-api (Rust, arm64)
Browser ── presigned PUT ─────► S3 docs bucket
Browser ── Cognito sign-in ───► Cognito user pool (self sign-up, email verification, USER_PASSWORD_AUTH)

openbook-api ─► DynamoDB (one table, on-demand)
             ─► S3 docs bucket
             ─► Bedrock Runtime (ConverseStream; au. profiles)
             ─► S3 Vectors + Tantivy BM25 indexes in S3 (phase 3)
             ─► SQS FIFO ingest queue (phase 2)

openbook-ingest (phase 2) ◄─ SQS FIFO (one message group per course, so each course index has one writer)
             ─► S3 (source PDF, page Markdown, BM25 segments), Bedrock (page transcription, embeddings), S3 Vectors
```

| Part | Choice | Why |
|---|---|---|
| Web | React 19, Vite, Tailwind 4, on S3 + CloudFront | Static, cached, almost free when idle |
| API | Rust, axum, `lambda_http`, `provided.al2023`, arm64, zip | Fast cold start, low memory, one deployable for all routes |
| Auth | Cognito user pool (Lite plan), ID token in `x-auth-token` | Free up to 10,000 monthly active users; OAC owns the `Authorization` header |
| Data | DynamoDB on-demand, single table | Pay per request, no capacity planning |
| Files | S3 with presigned PUT | The browser uploads directly; the API never streams file bytes |
| Models | A model catalog in configuration, all on Bedrock (Converse API) | One provider for Claude and GPT models; no API keys. See "Model catalog" |
| Embeddings | Bedrock `amazon.titan-embed-text-v2:0`, 1024 dimensions (phase 2) | On-demand in Sydney. Cohere Embed v4 exists only as a `global.` profile, which breaks the data residency rule |
| Dense retrieval | S3 Vectors, one index per course (phase 3) | Query cost grows with index size, so small per-course indexes keep queries cheap and isolate tenants |
| Lexical retrieval | Tantivy BM25, one index per course in S3 (phase 3) | Native Rust, no server; Bedrock Knowledge Bases do only semantic search on S3 Vectors |
| Fusion | Reciprocal rank fusion, k = 60 (phase 3) | BM25 and cosine scores do not need a common scale |
| Ingest queue | SQS FIFO with a dead-letter queue | Serial writes per course index, parallel across courses |
| Ingest worker | Rust Lambda `openbook-ingest` (arm64), pdfium for PDFs | See `docs/retrieval.md` |
| Vector store | S3 Vectors bucket per stage, one index per course | Pay per use; created by CDK (bucket) and the worker (indexes) |

## DynamoDB table

One table. Key attributes: `pk` (S), `sk` (S). Global secondary index `gsi1` on `gsi1pk` (S) and `gsi1sk` (S), projection ALL. Time to live attribute: `ttl` (epoch seconds).
Every item has a `type` attribute. Other attributes are camelCase.

| Item | `pk` | `sk` | `gsi1pk` | `gsi1sk` | Main attributes |
|---|---|---|---|---|---|
| Profile | `USER#<userId>` | `PROFILE` | | | `email`, `createdAt` |
| Counters (user) | `USER#<userId>` | `COUNTERS` | | | `courses` (courses owned; changed only inside the create and delete transactions, with a condition on the limit) |
| Failed joins | `USER#<userId>` | `JOINFAIL#D#<yyyy-mm-dd>` | | | `attempts`, `ttl` (40 days) |
| Course | `COURSE#<courseId>` | `META` | | | `name`, `description`, `tutoringStyle`, `model`, `ownerId`, `inviteCode`, `docCount`, `bm25Version`, `createdAt`, `updatedAt` |
| Membership | `COURSE#<courseId>` | `MEMBER#<userId>` | `USER#<userId>` | `COURSE#<joinedAt>#<courseId>` | `courseId`, `userId`, `email`, `role`, `joinedAt` |
| Invite code | `INVITE#<code>` | `INVITE` | | | `courseId`, `createdAt` |
| Document | `COURSE#<courseId>` | `DOC#<documentId>` | | | `filename`, `sizeBytes`, `s3Key`, `status`, `error`, `pageCount`, `chunkCount`, `transcribedPages`, `warnings`, `createdAt`, `updatedAt` |
| Conversation | `USER#<userId>` | `CONV#<conversationId>` | `CONVS#<userId>#<courseId>` | `<updatedAt>#<conversationId>` | `courseId`, `title`, `model`, `createdAt`, `updatedAt` (each chat message updates `model`, `updatedAt`, and `gsi1sk`) |
| Message | `CONV#<conversationId>` | `MSG#<messageId>` | | | `role`, `content`, `sources`, `model`, `inputTokens`, `outputTokens`, `createdAt` |
| Question | `COURSE#<courseId>` | `Q#<questionId>` | | | `userId`, `email`, `question`, `conversationId`, `createdAt`, `ttl` (180 days) |
| Daily usage (user) | `USER#<userId>` | `USAGE#D#<yyyy-mm-dd>` | | | `messages`, `inputTokens`, `outputTokens`, `transcribedPages`, `ttl` (40 days) |
| Monthly usage (user) | `USER#<userId>` | `USAGE#M#<yyyy-mm>` | | | `messages`, `inputTokens`, `outputTokens` (kept for billing later) |
| Daily usage (global) | `GLOBAL` | `USAGE#D#<yyyy-mm-dd>` | | | `messages`, `inputTokens`, `outputTokens`, `transcribedPages`, `ttl` (40 days) |
| Demo course (active) | `DEMO#COURSE` | `ACTIVE` | | | `courseId`, `fingerprint`, `attribution` (list), `provisionedAt`. The only way the API finds the demo course. `make demo-provision` writes it |
| Demo course (pending) | `DEMO#COURSE` | `PENDING` | | | `courseId` of a provisioning run that is not finished |
| Demo salt | `DEMO#SALT` | `D#<yyyy-mm-dd>` | | | `salt` (32 random bytes), `ttl` (2 days). The API creates it on first use with a conditional put |
| Demo visitor counter | `DEMO#V#<visitor>` | `W#<epoch/600>`, `D#<yyyy-mm-dd>`, or `B#<yyyy-mm-dd>` | | | `count` (questions, or millionths of a US dollar for `B#`), `ttl` (the 10-minute window plus a margin, or 2 days). `<visitor>` is 32 hex characters of the HMAC |
| Demo budget | `DEMO#GLOBAL` | `D#<yyyy-mm-dd>` | | | `count` (millionths of a US dollar, settled costs plus open reservations), `ttl` (2 days) |
| Demo turn reservation | `DEMO#RES#<turnId>` | `R` | | | `amount`, `ttl` (2 days). It exists while the turn holds a reservation; the settlement deletes it |
| Demo conversation | `DEMO#CONV#<id>` | `META` and `M#<messageId>` | | | `META`: `turns`, `ttl` (24 hours after the first turn). `M#`: `role`, `content`, `outputTokens` (answers only: the real output tokens), `ttl`. No `gsi1` keys and no user ID |

Rules:

- Create a course, its owner membership, and its invite code in one `TransactWriteItems`, with `attribute_not_exists(pk)` on the invite code. Retry with a new code on a conflict.
- Quotas: before the model call, reserve one message with `UpdateItem` `ADD messages :one` and the condition `attribute_not_exists(messages) OR messages < :limit`, first on the user's daily item, then on the global daily item. If the global reservation fails, give back the user reservation (`ADD messages :minusOne`). After the call, add the token counts to the daily, monthly, and global items (best effort; log failures).
- Limits on courses owned and documents per course are exact: the create transaction updates the counter (`COUNTERS.courses`, `META.docCount`) with a condition `< :limit`, and the delete transaction gives the slot back.
- Authorization reads (membership, course, invite, conversation, document) use strongly consistent reads.
- ULIDs make `sk` values sort by creation time. List queries that need newest first use `ScanIndexForward = false`.

## S3 layout

Docs bucket (private, SSE-S3, TLS only, CORS allows `PUT` from the site origin):

- `courses/<courseId>/documents/<documentId>/source.pdf`: the uploaded file.
- `courses/<courseId>/documents/<documentId>/pages/<nnnn>.md`: one Markdown file per page, written by the worker.
- `courses/<courseId>/bm25/v<version>.tar.zst`: the course BM25 index (see `docs/retrieval.md`).

Web bucket (private, read by CloudFront OAC): the built `apps/ask/dist` plus `config.json`.

The prod site is served at the custom domain `openbook.steve.gg` (`infra/cdk.json`, `stageDomains`): the landing page at `/`, the learner area at `/ask`, and course management at `/portal`. The old hosts `ask.steve.gg` and `portal.steve.gg` are aliases of the same distribution and redirect, in the viewer-request CloudFront Function of the default behavior (`infra/lib/spa-rewrite.ts`): `ask.steve.gg/<path>?<query>` goes to `https://openbook.steve.gg/<path>?<query>` (the web app redirects the old app routes on the client and keeps `/join/:code`, so an invite link keeps its code), and `portal.steve.gg/<anything>` goes to `https://openbook.steve.gg/portal`. The host is normalized first (lower case, no `:443`, no trailing dot), and a query key without a value stays as it is. `/api` and `/api/*` are never redirected: the API behaviors have no function, and the API works on any host. The status is `302` until the release is verified; then change `redirectStatus` to 301 (a browser keeps a 301 with no end date). On the default behavior the same function does the single-page-app rewrite (every path without a file extension is `/index.html`, so `/ask/*`, `/portal/*`, and `/join/*` work). The `*.steve.gg` certificate covers the three names. The ACM certificate (`us-east-1`) and the Route 53 hosted zone ID are parameters of the `Openbook-prod` stack that the deploy supplies, so the template holds no resource ID. Dev has no domain. See `infra/README.md`.

## API configuration

The API reads only environment variables. Names and defaults:

| Variable | Default | Notes |
|---|---|---|
| `OPENBOOK_STAGE` | (required) | `local`, `dev`, or `prod` |
| `OPENBOOK_TABLE_NAME` | (required) | |
| `OPENBOOK_DOCS_BUCKET` | (required) | |
| `OPENBOOK_COGNITO_USER_POOL_ID` | (required) | |
| `OPENBOOK_COGNITO_CLIENT_ID` | (required) | The token `aud` must equal this |
| `OPENBOOK_COGNITO_REGION` | `AWS_REGION` | Used to build the issuer `https://cognito-idp.<region>.amazonaws.com/<poolId>` |
| `OPENBOOK_COGNITO_ISSUER` | `https://cognito-idp.<region>.amazonaws.com/<poolId>` | Local only: the issuer that local tokens carry. AWS Emulator (since v1.0.471) issues `https://127.0.0.1:<port>/<poolId>` |
| `OPENBOOK_COGNITO_JWKS_URL` | `<issuer>/.well-known/jwks.json` | Local only (AWS Emulator serves JWKS at `https://127.0.0.1:<port>/<poolId>/.well-known/jwks.json`) |
| `OPENBOOK_MODELS` | (required) | The model catalog as a JSON array (see "Model catalog") |
| `OPENBOOK_DEFAULT_CHAT_MODEL` | (required) | A catalog ID with residency `au` |
| `OPENBOOK_FAST_MODEL` | (required) | A catalog ID with residency `au`, for query rewrite and page transcription |
| `OPENBOOK_BEDROCK_PROFILE` | (unset) | Local only: use real Bedrock with this AWS profile from `~/.aws`, while every other service uses the emulator |
| `OPENBOOK_CA_BUNDLE` | (unset) | Local only: a PEM file (the emulator's `ca.pem`) that the AWS SDK and JWKS clients trust in addition to the system roots |
| `OPENBOOK_PRESIGN_ENDPOINT` | (unset) | Local only: base URL for presigned S3 URLs that the browser can reach, for example `http://localhost:5173/__aws` |
| `OPENBOOK_LIMIT_DAILY_MESSAGES_PER_USER` | `50` | |
| `OPENBOOK_LIMIT_DAILY_MESSAGES_GLOBAL` | `500` | The kill switch for model spend. Until the public launch, `cdk.json` sets 300 (prod) and 100 (dev) |
| `OPENBOOK_INGEST_QUEUE_URL` | (required) | The ingest FIFO queue (API sends, worker receives) |
| `OPENBOOK_VECTOR_BUCKET` | (required) | The stage's S3 Vectors bucket |
| `OPENBOOK_EMBEDDING_MODEL_ID` | `amazon.titan-embed-text-v2:0` | A Bedrock embedding model that runs on demand in Sydney, or `fake` locally |
| `OPENBOOK_TRANSCRIBE_MODEL` | the fast model | A catalog ID with residency `au` that accepts images |
| `OPENBOOK_PDFIUM_PATH` | the executable's directory | Where `libpdfium` is; local runs and tests point it at `.local/pdfium/<platform>/lib` |
| `OPENBOOK_LIMIT_PAGES_PER_DOCUMENT` | `500` | |
| `OPENBOOK_LIMIT_DAILY_TRANSCRIBED_PAGES_PER_USER` | `300` | Counted for the document owner |
| `OPENBOOK_LIMIT_DAILY_TRANSCRIBED_PAGES_GLOBAL` | `1000` | Until the public launch, `cdk.json` sets 300 (prod) and 100 (dev) |
| `OPENBOOK_LIMIT_DAILY_TOKENS_GLOBAL` | `3000000` | Second spend guard: no new chat once today's global input plus output tokens reach it. Until the public launch, `cdk.json` sets 1500000 (prod) and 500000 (dev). A chat turn with retrieval sends about 5,000 input tokens (the sources) plus the answer, so the message caps (300 and 100) stay the binding limit and the token cap is the backstop. Worst case at Sonnet list prices (US$3 per million input tokens, US$15 per million output tokens), for prod: about US$7 a day at a realistic mix (85% input, 15% output), and at most about US$22 a day if every token were output. Dev is a third of that |
| `OPENBOOK_DEMO_ENABLED` | `false` | The kill switch of the public demo (`true` or `false`). When it is on, the default chat model and the fast model must have prices. CDK context `demoEnabled`, or `stageDemo.<stage>.enabled` in `cdk.json` (dev on, prod off until the demo course is provisioned) |
| `OPENBOOK_DEMO_LIMIT_VISITOR_PER_10_MIN` | `10` | Questions per demo visitor in a fixed 10-minute window. CDK context `limitDemoVisitorPer10Min` |
| `OPENBOOK_DEMO_LIMIT_VISITOR_PER_DAY` | `40` | Questions per demo visitor per UTC day. CDK context `limitDemoVisitorPerDay` |
| `OPENBOOK_DEMO_BUDGET_USD_PER_DAY` | `2` | What the whole demo may spend per UTC day, in US dollars. CDK context `demoBudgetUsdPerDay`, or `stageDemo.<stage>.budgetUsdPerDay` |
| `OPENBOOK_DEMO_VISITOR_BUDGET_USD_PER_DAY` | `0.3` | What one visitor may spend per UTC day. CDK context `demoVisitorBudgetUsdPerDay` |
| `OPENBOOK_DEMO_MAX_OUTPUT_TOKENS` | `1000` | The output limit of a demo answer, at most the limit of the default chat model. CDK context `demoMaxOutputTokens` |
| `OPENBOOK_API_MODE` | `all` locally, `full` in Lambda | Which routes this process mounts: `all` (every route; local runs only, Lambda refuses it), `full` (every route except `/api/demo/*`), or `demo` (only health and the demo). The stack sets `full` on the API function and `demo` on the demo function |
| `OPENBOOK_LIMIT_FAILED_JOINS_PER_DAY` | `20` | Per user; slows down guessing invite codes |
| `OPENBOOK_LIMIT_COURSES_OWNED` | `10` | |
| `OPENBOOK_LIMIT_DOCUMENTS_PER_COURSE` | `50` | |
| `OPENBOOK_LIMIT_UPLOAD_BYTES` | `26214400` | 25 MiB |
| `OPENBOOK_LISTEN_ADDR` | `127.0.0.1:8787` | Local server only |
| `AWS_REGION`, `AWS_ENDPOINT_URL`, `RUST_LOG` | standard | |

## Model catalog

`OPENBOOK_MODELS` is a JSON array. The order is the display order.

```json
[
  { "id": "claude-sonnet", "label": "Claude Sonnet 5.5", "vendor": "anthropic", "provider": "bedrock", "modelId": "au.anthropic.claude-sonnet-5-5", "residency": "au", "maxOutputTokens": 2000 }
]
```

| Field | Rules |
|---|---|
| `id` | Stable catalog ID stored on courses and messages. `[a-z0-9-]{1,40}`, unique |
| `label`, `vendor` | Display only |
| `provider` | `bedrock` (Converse and ConverseStream), or `fake` (local only; the API refuses it in Lambda) |
| `modelId` | The Bedrock model or inference profile ID |
| `residency` | `au` or `global`. It must match `modelId`, which must have one of three shapes (an allowlist; anything else is refused): `au.<vendor>.<model>` is `au`; `<vendor>.<model>` (an on-demand model in Sydney) is `au`; `global.<vendor>.<model>` is `global`. `<vendor>` must be one of `anthropic`, `openai`, `amazon`, `meta`, `mistral`, `cohere`, `deepseek`, `qwen`. The API and the CDK app use the same rule and the same vendor list. Entries with provider `fake` (local only, refused in Lambda) skip the shape rule and keep the residency they declare, so local tests can have an `au` and a `global` fake model |
| `maxOutputTokens` | Server-fixed output limit (default 2000) |
| `inputUsdPerMillion`, `outputUsdPerMillion` | List price in US dollars per million input and output tokens, from 0 to 1000. Set both or neither. The CDK app requires them on every deployed model, and the API requires them on the default chat model when the demo is on. The demo budget uses them, and billing will too |
| `temperature` | Optional. Leave it out for models that reject it |
| `vision` | Optional, default `false`. `true` when the model accepts images; `OPENBOOK_TRANSCRIBE_MODEL` must name such an entry. Claude Sonnet and Haiku are `true`, gpt-oss-120b is `false`, local fake entries are `true` |

The API validates the catalog at startup and refuses to start when it is invalid. The CDK app holds the deployed catalog in context (`models`, `defaultChatModel`, `fastModel`), writes it to the Lambda environment, and derives the Bedrock IAM permissions from it: the inference-profile ARN for each profile ID plus the foundation-model ARNs it routes to (`au.`: Sydney and Melbourne; `global.`: every Region), or the Sydney foundation-model ARN for an ID without a prefix. The streaming code sends only text deltas to the client and ignores reasoning content blocks.

Deployed catalog (only models that this account can call today):

| `id` | Model | `modelId` | Residency | USD per million tokens (in / out) |
|---|---|---|---|---|
| `claude-sonnet` (default chat) | Claude Sonnet 4.6 | `au.anthropic.claude-sonnet-4-6` | `au` | 3.30 / 16.50 |
| `claude-haiku` (fast) | Claude Haiku 4.5 | `au.anthropic.claude-haiku-4-5-20251001-v1:0` | `au` | 1.10 / 5.50 |
| `gpt-oss-120b` | gpt-oss-120b | `openai.gpt-oss-120b-1:0` | `au` | 0.1545 / 0.618 |

The prices are the ap-southeast-2 list prices (the "Regional" cross-Region inference prices of the `au.` profiles, about 10 percent above the global ones). Update them in `infra/cdk.json` when AWS changes the list.

Waiting for AWS access (the Bedrock agreements are accepted, but AWS blocks the account: Claude 5.5 has a tokens-per-minute quota of 0, and GPT-6.1 returns "not available for this account"): `au.anthropic.claude-sonnet-5-5`, `au.anthropic.claude-opus-5-5`, `au.anthropic.claude-haiku-5-5`, `global.openai.gpt-6.1-sol`, `global.openai.gpt-6.1-terra`. When access arrives, add them to the catalog in `infra/cdk.json` and make Claude Sonnet 5.5 the default chat model.

Locally, the catalog has one entry `{ "id": "fake", "provider": "fake", ... }` unless `OPENBOOK_BEDROCK_PROFILE` is set, in which case it is the deployed catalog on real Bedrock.

## Public demo

The landing page at `openbook.steve.gg` has a "try it" box: a visitor with no account asks a question and gets a real streamed answer with citations from a demo course. It behaves like the product (the default chat model, the same retrieval and prompt, follow-up questions), and a daily price cap is the safety net. If it is abused, the kill switch is the pivot.

- **The demo course** holds two OpenStax excerpts (College Algebra and American Government 2e, CC BY 4.0, listed in `demo/course.json`, where each document has the title of its book as its name, for example `College Algebra.pdf`, because the UI shows the name; a new name changes the fingerprint and makes the next provisioning build the course again; the examples are golden questions that hybrid retrieval answers at rank 1). `make demo-provision` creates it through the normal ingestion path: a course item (owner `demo`, which no Cognito user can have; no membership and no invite code), the PDFs in the docs bucket, the same `ingest` messages that the API sends, and the real worker (pages, chunks, embeddings, vectors, BM25). When every document is `ready`, it writes the `DEMO#COURSE / ACTIVE` item, then deletes the previous demo course. A second run with the same content changes nothing ("up to date"). A run that stopped half way leaves a `PENDING` item, and the next run removes that course first, unless it is the active course.
- **Its own function.** The demo runs in a second Lambda function, `DemoFunction`: the same binary as the API, started with `OPENBOOK_API_MODE=demo`, which mounts only `GET /api/health` and `POST /api/demo/chat`. The main API (`OPENBOOK_API_MODE=full`) does not serve `/api/demo/*` at all. CloudFront has a behavior `/api/demo/*` with its own function URL and OAC, ahead of `/api/*`. The demo function has a small reserved concurrency (3 in prod, 2 in dev; CDK context `stageDemo.<stage>.concurrency`), so a few visitors cannot take the executions of signed-in users (a stream holds an execution for up to 100 seconds), and in dev one visitor cannot block the whole API. A throttle (Lambda answers `429`) or a timeout (`5xx`) is not an answer of ours: every response of the demo route carries the header `x-openbook-demo`, and the client treats a `429` or `5xx` without it as `demo_busy` (`docs/api.md`). A viewer-response CloudFront Function cannot do it, because CloudFront does not run one when the origin answers 400 or higher.
- **Its own role.** The demo function has its own least-privilege role, under the permissions boundary in prod: `GetItem` and `Query` on the table where the leading key is `DEMO#*` or `COURSE#demo*`; `PutItem`, `UpdateItem`, and `DeleteItem` where the leading key is `DEMO#*`; `GetObject` on `courses/demo*/bm25/*`; `GetVectors` and `QueryVectors` on the index `c-demo*`; and `InvokeModel` for the default chat model, the fast model, and the embedding model. No queue, no file or vector writes, no list. The ID of the demo course is `demo` and a ULID (`ids::new_demo_course_id`, made by `make demo-provision`). A normal course ID is a ULID of upper-case Crockford base32, which has no letter `o`, so no ID of a user's course can start with `demo`, and the role cannot read any course of a user, even if the code had a bug. The demo ID is not a ULID either, so the course routes answer `404` for it before they read anything.
- **The route** reads the demo course and writes only under `DEMO#` partitions: counters, reservations, the salt, and demo conversations. It touches no user item, no course item of a user, no usage item, no question record, and no S3 object. No normal route changes: the demo course has no membership, so every course route returns `404` for everyone (the tests check all of them, for a teacher and a student, and check that every other route still needs a sign-in).
- **The visitor** is an HMAC-SHA256 of the client address, with a salt that the API creates in the table on its first use of each UTC day (a conditional put, so servers agree; time to live 2 days). Hashes of different days cannot be linked, and no secret service with a monthly fee is needed. An IPv6 visitor is the `/56`, because a home gets a `/56` or a `/64`, and a visitor must not be able to change the address inside it. In Lambda, the address is the header `CloudFront-Viewer-Address`. CloudFront sets it itself and replaces a value from the viewer. The `/api/*` and `/api/demo/*` behaviors use the managed origin request policy "all viewer except Host", which forwards the CloudFront viewer headers, `CloudFront-Viewer-Address` among them, and not Host (the function URL needs its own Host). After a deploy, check on dev that the header arrives (`docs/local-dev.md`, "Deploy checks for the demo"). If the address is missing or unreadable in Lambda, the route fails closed with `503 demo_unavailable`. Locally, it is the socket address. Neither the address nor the hash is logged.
- **Per-visitor limits** stop one visitor from using up the budget for everyone. They are DynamoDB counters with a conditional update and a time to live: 10 questions per 10 minutes (a fixed window, so a burst across a window edge can reach 20), 40 questions per day, and a dollar cap of 0.30 USD a day (`OPENBOOK_DEMO_VISITOR_BUDGET_USD_PER_DAY`), which a turn reserves and settles in the same way as the global budget. One visitor can therefore use at most 15 percent of the day (0.30 of 2.00 USD), and it takes at least 7 visitors to use up the day. The output limit of a demo answer is 1,000 tokens (`OPENBOOK_DEMO_MAX_OUTPUT_TOKENS`, never above the catalog limit of the model), which is long enough for a real answer.
- **The budget** is a pay-per-request counter of millionths of a US dollar per UTC day (2.00 USD by default). A turn reserves its worst-case cost before any model call, in one `TransactWriteItems` with a client request token: it adds the amount to the global counter and to the visitor's dollar counter, each with a condition that the sum stays within its limit, and it writes an item for the turn (`DEMO#RES#<turn>`). When something does not fit, nothing changes: the global budget gives `503 demo_busy`, and the visitor cap gives `429 demo_rate_limited`. The reservation is an upper bound:
  - the answer: the course prompt, the most sources that retrieval can return (8 passages of 2,600 characters at 2 characters per token), the history, the question, and the full output limit, at the catalog prices;
  - the history is counted with the real output tokens of each stored answer (the conversation items keep them), and with a safe bound for the questions: one token per 3 letters or spaces, one token per digit, punctuation mark, or LaTeX symbol, and one token per UTF-8 byte of any other character (up to 4 for an emoji, because a byte-level tokenizer can spend one token on each byte);
  - for a follow-up, the rewrite by the fast model;
  - the embedding of the query: 500 tokens (the longest query), at the price of the fast model, which is far above the real price.
  After the model calls, the turn settles in one transaction that deletes its item and gives back the part of the reservation that it did not use. The delete has a condition that the item exists, so a settlement happens once: a retry, or a second task of the same turn, gives back nothing. A cost above the reservation is a bug of the estimate. It is logged as an error and the reservation is kept as the charge, so the counter never goes above the budget. When a stream fails, or a model gives no usage, the turn keeps its whole reservation: money is never given back on an estimate. A model call that returns a clear error before it starts costs only the retrieval tokens, which are known, and gives the rest back. A call that times out before it starts is not clear (it may have run), so the turn keeps the whole reservation. Every reserve and settle is a transaction on the same day counter, and DynamoDB cancels a transaction that conflicts with another one that runs at that moment (the SDK does not retry it). The code retries a cancellation with `TransactionConflict` up to 5 times with a jittered pause. A reservation that still conflicts is answered with `demo_busy`, and a settlement that still conflicts is logged and keeps the reservation, so a conflict never gives money back by mistake. The settlement runs also when the client leaves, because the task does not depend on the connection. The sum of the settled costs and the open reservations never goes over the budget, so turns at the same moment cannot overspend, and a day has its own counter, so the budget starts again at midnight UTC.
- **Worst case.** The budget is the cap: at most 2.00 USD a day for model calls, as long as the catalog prices are right (they are the ap-southeast-2 prices of the `au.` profiles, which cost about 10 percent more than the global ones: Sonnet 4.6 at 3.30 and 16.50, and Haiku 4.5 at 1.10 and 5.50 USD per million input and output tokens). A turn without history reserves about 0.055 USD (about 11,500 input tokens and 1,000 output tokens), and a turn late in a long conversation up to about 0.10 USD. A normal turn costs about 0.025 USD (about 5,000 input tokens of sources and about 500 output tokens), so 2.00 USD is about 80 answers, and one visitor can have about 12 of them. Query embeddings (Titan, 0.02 USD per million tokens) and the other services (Lambda, CloudFront, DynamoDB) cost a few cents at most. A refused request costs one Lambda call and a few DynamoDB requests, and the reserved concurrency of 3 limits how many run at once.
- **The kill switch** is `OPENBOOK_DEMO_ENABLED`, from CDK context per stage (`stageDemo` in `cdk.json`). Off, the route returns `404 demo_unavailable` before it reads the body, and the public `config.json` says `demo.enabled: false`, so the landing page hides the demo. The deploy of one context change is the pivot if the demo is abused. The budget, the visitor limits, the output limit, and the concurrency change the same way.
- **Credits.** CC BY 4.0 needs the credit line. The `meta` event carries the attribution of each book, and `config.json` carries the title, the licence, and the link.

## Cost guards

- Per-user daily message quota, a global daily message cap, and a global daily token cap (above). Until the public launch the prod caps are 300 messages and 1,500,000 tokens a day, which cost at most about US$7 a day at a realistic input-to-output mix, and about US$22 if every token were output (Sonnet list prices). The monthly budget alerts below are the second line.
- Per-user and global daily caps on transcribed pages (about US$0.005 a page with Claude Haiku), a page limit per document, and the 50-document limit per course.
- The public demo has its own daily budget in US dollars (2.00 USD by default), reserved before every model call and settled after it, plus per-visitor limits and a kill switch (see "Public demo"). It does not use the message and token caps above, which are for signed-in users.
- Reserved concurrency on the API function (dev 5, prod 25).
- Server-fixed `maxTokens` (2,000 for chat) and a 20-message history window.
- The account already has a US$20 monthly AWS Budget with alerts at 85% and 100% actual and 100% forecast.
- CloudWatch log retention: 1 week (dev), 1 month (prod). No custom metrics.

## Phases

0. Monorepo, CI, deployment role, dev stack. (done)
1. Rust API with the routes in `docs/api.md` (no retrieval yet), quotas, the web app on the new API, local end-to-end tests on AWS Emulator. (done)
2. (current) Ingestion: PDF pages to Markdown with LaTeX (Claude for pages with math or poor text), chunks split by heading with page numbers, embeddings, S3 Vectors and Tantivy writes.
3. Hybrid retrieval with reciprocal rank fusion, citations with page numbers, rewrites of follow-up questions.
4. Retrieval evaluation in CI (Recall@k, nDCG@10, citation accuracy) and tuning against those numbers.
5. Public sign-up at openbook.steve.gg, and removal of the old `SteveStack` and the old portal site.
