# Openbook architecture

Openbook is an open-source platform for tutoring that is grounded in course materials.
Ask (`apps/ask`, served at ask.steve.gg) is its web app for teachers and students.
A teacher uploads course PDFs, and students get tutoring answers that cite those PDFs.
Think of it as an open-book exam for the model: it answers from the course materials, not from memory.

## Hard rules

1. **Pay per use only.** No component may have a baseline monthly cost. Do not add OpenSearch, NAT gateways, RDS or Aurora, ElastiCache, always-on containers, WAF web ACLs, Secrets Manager secrets, or customer-managed KMS keys. An idle stack must cost cents.
2. **The server owns the prompt.** Clients send a course ID, a conversation ID, and the student's message. The server picks the model, the system prompt, the token limits, and the sources.
3. **Tenancy is the course.** Every read and write of course data checks membership first. A non-member gets `404`, never `403`.
4. **Data stays in Australia.** Use the `ap-southeast-2` Region and Bedrock `au.` inference profiles. Do not use `global.` or other Regions' profiles.
5. **Same code locally and in Lambda.** The API is one axum router. Lambda runs it through `lambda_http`; locally it runs on a TCP listener. Local runs use AWS Emulator for AWS services.
6. **Fail closed.** If the API runs in Lambda with local-only settings (stage `local`, the fake model provider, `AWS_ENDPOINT_URL`, `OPENBOOK_COGNITO_JWKS_URL`, `OPENBOOK_CA_BUNDLE`, `OPENBOOK_PRESIGN_ENDPOINT`, or `OPENBOOK_BEDROCK_PROFILE`), it must refuse to start.

## Components

```
Browser ── https://<site> ──► CloudFront
                               ├─ /*      ─► S3 web bucket (OAC). SPA routes rewrite to /index.html by a CloudFront Function.
                               └─ /api/*  ─► Lambda function URL (OAC, AWS_IAM auth, RESPONSE_STREAM) ─► openbook-api (Rust, arm64)
Browser ── presigned PUT ─────► S3 docs bucket
Browser ── Cognito SRP ───────► Cognito user pool (self sign-up, email verification)

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
| Chat model | Bedrock `au.anthropic.claude-sonnet-5-5` | Best Australian-resident model for tutoring quality |
| Fast model | Bedrock `au.anthropic.claude-haiku-5-5` | Query rewrite and page transcription (phase 2 and 3) |
| Dense retrieval | S3 Vectors, one index per course (phase 3) | Query cost grows with index size, so small per-course indexes keep queries cheap and isolate tenants |
| Lexical retrieval | Tantivy BM25, one index per course in S3 (phase 3) | Native Rust, no server; Bedrock Knowledge Bases do only semantic search on S3 Vectors |
| Fusion | Reciprocal rank fusion, k = 60 (phase 3) | BM25 and cosine scores do not need a common scale |
| Ingest queue | SQS FIFO (phase 2) | Serial writes per course index, parallel across courses |

## DynamoDB table

One table. Key attributes: `pk` (S), `sk` (S). Global secondary index `gsi1` on `gsi1pk` (S) and `gsi1sk` (S), projection ALL. Time to live attribute: `ttl` (epoch seconds).
Every item has a `type` attribute. Other attributes are camelCase.

| Item | `pk` | `sk` | `gsi1pk` | `gsi1sk` | Main attributes |
|---|---|---|---|---|---|
| Profile | `USER#<userId>` | `PROFILE` | | | `email`, `createdAt` |
| Course | `COURSE#<courseId>` | `META` | | | `name`, `description`, `tutoringStyle`, `ownerId`, `inviteCode`, `createdAt`, `updatedAt` |
| Membership | `COURSE#<courseId>` | `MEMBER#<userId>` | `USER#<userId>` | `COURSE#<joinedAt>#<courseId>` | `courseId`, `userId`, `email`, `role`, `joinedAt` |
| Invite code | `INVITE#<code>` | `INVITE` | | | `courseId`, `createdAt` |
| Document | `COURSE#<courseId>` | `DOC#<documentId>` | | | `filename`, `sizeBytes`, `s3Key`, `status`, `error`, `pageCount`, `createdAt`, `updatedAt` |
| Conversation | `USER#<userId>` | `CONV#<conversationId>` | `CONVS#<userId>#<courseId>` | `<updatedAt>#<conversationId>` | `courseId`, `title`, `createdAt`, `updatedAt` (each chat message updates `updatedAt` and `gsi1sk`) |
| Message | `CONV#<conversationId>` | `MSG#<messageId>` | | | `role`, `content`, `sources`, `inputTokens`, `outputTokens`, `createdAt` |
| Question | `COURSE#<courseId>` | `Q#<questionId>` | | | `userId`, `email`, `question`, `conversationId`, `createdAt`, `ttl` (180 days) |
| Daily usage (user) | `USER#<userId>` | `USAGE#D#<yyyy-mm-dd>` | | | `messages`, `inputTokens`, `outputTokens`, `ttl` (40 days) |
| Monthly usage (user) | `USER#<userId>` | `USAGE#M#<yyyy-mm>` | | | `messages`, `inputTokens`, `outputTokens` (kept for billing later) |
| Daily usage (global) | `GLOBAL` | `USAGE#D#<yyyy-mm-dd>` | | | `messages`, `inputTokens`, `outputTokens`, `ttl` (40 days) |

Rules:

- Create a course, its owner membership, and its invite code in one `TransactWriteItems`, with `attribute_not_exists(pk)` on the invite code. Retry with a new code on a conflict.
- Quotas: before the model call, reserve one message with `UpdateItem` `ADD messages :one` and the condition `attribute_not_exists(messages) OR messages < :limit`, first on the user's daily item, then on the global daily item. If the global reservation fails, give back the user reservation (`ADD messages :minusOne`). After the call, add the token counts to the daily, monthly, and global items (best effort; log failures).
- ULIDs make `sk` values sort by creation time. List queries that need newest first use `ScanIndexForward = false`.

## S3 layout

Docs bucket (private, SSE-S3, TLS only, CORS allows `PUT` from the site origin):

- `courses/<courseId>/documents/<documentId>/source.pdf`: the uploaded file.
- Phase 2 adds `courses/<courseId>/documents/<documentId>/pages/<nnnn>.md` and `courses/<courseId>/bm25/...`.

Web bucket (private, read by CloudFront OAC): the built `apps/ask/dist` plus `config.json`.

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
| `OPENBOOK_COGNITO_JWKS_URL` | `<issuer>/.well-known/jwks.json` | Local only (AWS Emulator serves JWKS at `https://127.0.0.1:<port>/<poolId>/.well-known/jwks.json`) |
| `OPENBOOK_MODEL_PROVIDER` | `bedrock` | `bedrock` or `fake` (local only) |
| `OPENBOOK_CHAT_MODEL_ID` | `au.anthropic.claude-sonnet-5-5` | |
| `OPENBOOK_FAST_MODEL_ID` | `au.anthropic.claude-haiku-5-5` | |
| `OPENBOOK_BEDROCK_PROFILE` | (unset) | Local only: use real Bedrock with this AWS profile from `~/.aws`, while every other service uses the emulator |
| `OPENBOOK_CA_BUNDLE` | (unset) | Local only: a PEM file (the emulator's `ca.pem`) that the AWS SDK and JWKS clients trust in addition to the system roots |
| `OPENBOOK_PRESIGN_ENDPOINT` | (unset) | Local only: base URL for presigned S3 URLs that the browser can reach, for example `http://localhost:5173/__aws` |
| `OPENBOOK_LIMIT_DAILY_MESSAGES_PER_USER` | `50` | |
| `OPENBOOK_LIMIT_DAILY_MESSAGES_GLOBAL` | `500` | The kill switch for model spend |
| `OPENBOOK_LIMIT_COURSES_OWNED` | `10` | |
| `OPENBOOK_LIMIT_DOCUMENTS_PER_COURSE` | `50` | |
| `OPENBOOK_LIMIT_UPLOAD_BYTES` | `26214400` | 25 MiB |
| `OPENBOOK_LISTEN_ADDR` | `127.0.0.1:8787` | Local server only |
| `AWS_REGION`, `AWS_ENDPOINT_URL`, `RUST_LOG` | standard | |

## Cost guards

- Per-user daily message quota and a global daily message cap (above).
- Reserved concurrency on the API function (dev 5, prod 25).
- Server-fixed `maxTokens` (2,000 for chat) and a 20-message history window.
- The account already has a US$20 monthly AWS Budget with alerts at 85% and 100% actual and 100% forecast.
- CloudWatch log retention: 1 week (dev), 1 month (prod). No custom metrics.

## Phases

0. Monorepo, CI, deployment role, dev stack. (current)
1. Rust API with the routes in `docs/api.md` (no retrieval yet), quotas, the web app on the new API, local end-to-end tests on AWS Emulator. (current)
2. Ingestion: PDF pages to Markdown with LaTeX (Claude for pages with math or poor text), chunks split by heading with page numbers, embeddings, S3 Vectors and Tantivy writes.
3. Hybrid retrieval with reciprocal rank fusion, citations with page numbers, rewrites of follow-up questions.
4. Retrieval evaluation in CI (Recall@k, nDCG@10, citation accuracy) and tuning against those numbers.
5. Public sign-up at ask.steve.gg, and removal of the old `SteveStack` and the old portal site.
