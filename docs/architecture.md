# Openbook architecture

Openbook is an open-source platform for tutoring that is grounded in course materials.
Ask (`apps/ask`, served at ask.steve.gg) is its web app for teachers and students.
A teacher uploads course PDFs, and students get tutoring answers that cite those PDFs.
Think of it as an open-book exam for the model: it answers from the course materials, not from memory.

## Hard rules

1. **Pay per use only.** No component may have a baseline monthly cost. Do not add OpenSearch, NAT gateways, RDS or Aurora, ElastiCache, always-on containers, WAF web ACLs, Secrets Manager secrets, or customer-managed KMS keys. An idle stack must cost cents.
2. **The server owns the prompt.** Clients send a course ID, a conversation ID, and the student's message. The server picks the system prompt, the token limits, and the sources. The model comes from the deployment's model catalog: a course owner sets the course default, any member can pick another catalog model for their own chats, and nobody can use a model outside the catalog.
3. **Tenancy is the course.** Every read and write of course data checks membership first. A non-member gets `404`, never `403`.
4. **Data stays in Australia by default.** Use the `ap-southeast-2` Region. The default chat and fast models must have residency `au` (an `au.` inference profile, or a model that runs on demand in Sydney). A catalog may also offer `global.` models (for example the frontier GPT models, which Bedrock offers only through global profiles); they are marked `global`, and the web app tells the user who picks such a model that it may process data outside Australia. No other profile prefixes (`us.`, `apac.`, and so on).
5. **Same code locally and in Lambda.** The API is one axum router. Lambda runs it through `lambda_http`; locally it runs on a TCP listener. Local runs use AWS Emulator for AWS services.
7. **Known risk until the public launch: PDF parsing is not isolated.** pdfium (C++) parses untrusted PDFs in the ingest worker, which holds credentials for every course's files, the table, and every vector index. A pdfium memory-safety bug could therefore cross tenants. Before public sign-up (phase 5), PDF opening, text extraction, and page rendering move to a separate function with no data permissions (a presigned GET in, text and images out), and a scheduled check flags new pdfium releases.
6. **Fail closed.** If the API runs in Lambda with local-only settings (stage `local`, a catalog entry with provider `fake`, `AWS_ENDPOINT_URL`, `OPENBOOK_COGNITO_ISSUER`, `OPENBOOK_COGNITO_JWKS_URL`, `OPENBOOK_CA_BUNDLE`, `OPENBOOK_PRESIGN_ENDPOINT`, or `OPENBOOK_BEDROCK_PROFILE`), it must refuse to start.

## Components

```
Browser ── https://<site> ──► CloudFront
                               ├─ /*      ─► S3 web bucket (OAC). SPA routes rewrite to /index.html by a CloudFront Function.
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

The prod site is served at the custom domain `ask.steve.gg` (`infra/cdk.json`, `stageDomains`). The ACM certificate (`us-east-1`) and the Route 53 hosted zone ID are parameters of the `Openbook-prod` stack that the deploy supplies, so the template holds no resource ID. Dev has no domain. See `infra/README.md`.

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
| `temperature` | Optional. Leave it out for models that reject it |
| `vision` | Optional, default `false`. `true` when the model accepts images; `OPENBOOK_TRANSCRIBE_MODEL` must name such an entry. Claude Sonnet and Haiku are `true`, gpt-oss-120b is `false`, local fake entries are `true` |

The API validates the catalog at startup and refuses to start when it is invalid. The CDK app holds the deployed catalog in context (`models`, `defaultChatModel`, `fastModel`), writes it to the Lambda environment, and derives the Bedrock IAM permissions from it: the inference-profile ARN for each profile ID plus the foundation-model ARNs it routes to (`au.`: Sydney and Melbourne; `global.`: every Region), or the Sydney foundation-model ARN for an ID without a prefix. The streaming code sends only text deltas to the client and ignores reasoning content blocks.

Deployed catalog (only models that this account can call today):

| `id` | Model | `modelId` | Residency |
|---|---|---|---|
| `claude-sonnet` (default chat) | Claude Sonnet 4.6 | `au.anthropic.claude-sonnet-4-6` | `au` |
| `claude-haiku` (fast) | Claude Haiku 4.5 | `au.anthropic.claude-haiku-4-5-20251001-v1:0` | `au` |
| `gpt-oss-120b` | gpt-oss-120b | `openai.gpt-oss-120b-1:0` | `au` |

Waiting for AWS access (the Bedrock agreements are accepted, but AWS blocks the account: Claude 5.5 has a tokens-per-minute quota of 0, and GPT-6.1 returns "not available for this account"): `au.anthropic.claude-sonnet-5-5`, `au.anthropic.claude-opus-5-5`, `au.anthropic.claude-haiku-5-5`, `global.openai.gpt-6.1-sol`, `global.openai.gpt-6.1-terra`. When access arrives, add them to the catalog in `infra/cdk.json` and make Claude Sonnet 5.5 the default chat model.

Locally, the catalog has one entry `{ "id": "fake", "provider": "fake", ... }` unless `OPENBOOK_BEDROCK_PROFILE` is set, in which case it is the deployed catalog on real Bedrock.

## Cost guards

- Per-user daily message quota, a global daily message cap, and a global daily token cap (above). Until the public launch the prod caps are 300 messages and 1,500,000 tokens a day, which cost at most about US$7 a day at a realistic input-to-output mix, and about US$22 if every token were output (Sonnet list prices). The monthly budget alerts below are the second line.
- Per-user and global daily caps on transcribed pages (about US$0.005 a page with Claude Haiku), a page limit per document, and the 50-document limit per course.
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
5. Public sign-up at ask.steve.gg, and removal of the old `SteveStack` and the old portal site.
