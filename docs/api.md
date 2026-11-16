# Openbook HTTP API (v1)

This is the contract between the Ask web app (`apps/ask`), the Rust API (`crates/api`), the end-to-end tests, and the infrastructure.
Change this file in the same commit as any change to the API.

## Transport

All routes live under `/api` on the same origin as the web app.
CloudFront sends `/api/*` to the API Lambda function URL with origin access control (OAC).
Locally, the Vite dev server proxies `/api/*` to the API process on `127.0.0.1:8787`.

Request rules:

- Bodies are JSON (`content-type: application/json`) unless a route says otherwise.
- Every route except `GET /api/health` is authenticated and needs the header `x-auth-token: <Cognito ID token>`.
  Do not use `Authorization`: CloudFront OAC replaces that header with its own SigV4 signature.
- Every request that is not `GET` or `HEAD` must send `x-amz-content-sha256: <lowercase hex SHA-256 of the exact body bytes>`.
  For an empty body, send the hash of the empty string (`e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`).
  Lambda function URLs behind OAC reject `POST`, `PUT`, `PATCH`, and `DELETE` requests without it.
  The local API ignores this header, but the web client must always send it so that local and deployed behavior are the same.
- IDs are ULIDs (26 characters, Crockford base32, upper case) unless a field says otherwise.
- Timestamps are RFC 3339 strings in UTC with millisecond precision, for example `2026-11-16T01:02:03.456Z`.
- JSON field names are camelCase.

Error responses always have this shape and a matching status code:

```json
{ "error": { "code": "not_found", "message": "Course not found" } }
```

| Status | `code` values | When |
|---|---|---|
| 400 | `invalid_request` | Body or parameters fail validation |
| 401 | `unauthenticated` | Token missing, expired, or not valid |
| 403 | `forbidden` | Authenticated, but the role does not allow the action (for example, a student calls an owner route of a course they are a member of) |
| 404 | `not_found` | The resource does not exist, or the caller is not a member (never reveal that a course exists to a non-member) |
| 409 | `conflict`, `limit_reached` | State conflict, or a per-user resource limit (courses, documents) |
| 413 | `too_large` | Upload size above the limit |
| 429 | `quota_exceeded` | The caller used all of today's chat messages |
| 503 | `capacity_exhausted`, `model_unavailable` | The global daily cap is reached, or the model provider fails or throttles |
| 500 | `internal` | Anything else (the message must not leak internals) |

## Accounts

The Cognito user pool uses email as the username. Passwords: at least 8 characters, with at least one lower-case letter, one upper-case letter, and one digit. Symbols are allowed but not required. The web app checks the same rules before it calls Cognito.
The API accepts only ID tokens with `email_verified` equal to `true`. The pool keeps the original email until a new one is verified. Local tests that confirm users with `AdminConfirmSignUp` must also set `email_verified=true` with `AdminUpdateUserAttributes`, because the emulator (like Cognito for admin confirmation) leaves it `false`.
The web app signs in with the `USER_PASSWORD_AUTH` flow in every stage (the password goes only to Cognito, over HTTPS). The app client allows only `ALLOW_USER_PASSWORD_AUTH` and `ALLOW_REFRESH_TOKEN_AUTH`. AWS Emulator does not implement SRP, and one flow everywhere keeps the local end-to-end tests faithful to production.

## Types

```ts
type Role = 'owner' | 'student';
type TutoringStyle = 'socratic' | 'hints' | 'step_by_step' | 'concepts' | 'direct';
type DocumentStatus = 'uploading' | 'queued' | 'processing' | 'ready' | 'failed';

interface User { id: string /* Cognito sub */; email: string }

interface Limits {
  dailyMessages: number;
  maxCoursesOwned: number;
  maxDocumentsPerCourse: number;
  maxUploadBytes: number;
}

interface Usage { date: string /* YYYY-MM-DD, UTC */; messagesUsed: number; messagesLimit: number }

interface Course {
  id: string;
  name: string;            // 1..80 chars after trim
  description: string;     // 0..500 chars
  tutoringStyle: TutoringStyle;
  model: string;           // the course default model, a ModelInfo.id from GET /api/models
  role: Role;              // the caller's role in this course
  inviteCode?: string;     // present in every Course response when role === 'owner', never otherwise
  createdAt: string;
  updatedAt: string;
}

interface Member { userId: string; email: string; role: Role; joinedAt: string }

interface ModelInfo {
  id: string;              // catalog ID, for example "claude-sonnet"
  label: string;           // for example "Claude Sonnet 5.5"
  vendor: string;          // "anthropic", "openai", ...
  residency: 'au' | 'global'; // 'global' means the provider may process data outside Australia
  isDefault: boolean;      // true for the deployment's default chat model
}

interface Document {
  id: string;
  courseId: string;
  filename: string;        // 1..200 chars, must end in .pdf (case-insensitive)
  sizeBytes: number;
  status: DocumentStatus;
  error?: string;          // only when status === 'failed'; safe to show to the owner
  pageCount?: number;      // set by ingestion (phase 2)
  createdAt: string;
  updatedAt: string;
}

interface Conversation { id: string; courseId: string; title: string; model: string /* catalog ID of the last answer */; createdAt: string; updatedAt: string }

interface Source {          // phase 3; the API sends none before then
  n: number;               // citation number used in the answer text, starting at 1
  documentId: string;
  documentName: string;
  page?: number;           // 1-based page in the source PDF
  snippet: string;         // the passage given to the model
}

interface Message {
  id: string;
  role: 'user' | 'assistant';
  content: string;         // Markdown with $...$ and $$...$$ LaTeX
  createdAt: string;
  sources?: Source[];      // assistant messages only
  model?: string;          // assistant messages only: the catalog ID that wrote it
}

interface Question {        // teacher insights
  id: string;
  userId: string;
  email: string;
  question: string;
  conversationId: string;
  createdAt: string;
}
```

Tutoring styles (the server owns the prompts; clients only send the style ID):

| ID | Name | Behavior |
|---|---|---|
| `socratic` | Socratic | Asks guiding questions; does not give the final answer first |
| `hints` | Hints | Gives progressive hints; gives the full answer only when the student asks again |
| `step_by_step` | Step by step | Breaks the problem into small steps and checks understanding |
| `concepts` | Concepts | Links the question to the underlying concepts in the materials |
| `direct` | Direct | Explains clearly and directly, then checks understanding |

## Routes

### Health

`GET /api/health` (no auth) → `200 { "ok": true, "stage": "dev", "version": "0.1.0" }`

### Me

`GET /api/me` → `200 { "user": User, "usage": Usage, "limits": Limits }`.
The first call for a user creates their profile.

### Models

`GET /api/models` → `200 { "models": ModelInfo[] }`: the deployment's model catalog, in display order.

### Courses

`GET /api/courses` → `200 { "courses": Course[] }`: every course the caller is a member of, newest membership first.

`POST /api/courses` body `{ "name": string, "description"?: string, "tutoringStyle"?: TutoringStyle, "model"?: string }` → `201 { "course": Course }`.
The caller becomes the owner. Default style is `socratic`. Default model is the catalog default. An unknown `model` → `400`. `409 limit_reached` when the caller already owns `maxCoursesOwned` courses.

`GET /api/courses/{courseId}` → `200 { "course": Course }` for members, else `404`.

`PATCH /api/courses/{courseId}` (owner) body `{ "name"?, "description"?, "tutoringStyle"?, "model"? }` → `200 { "course": Course }`. An unknown `model` → `400`.
If a course's stored model leaves the catalog later, reads return the catalog default in `model`, and chat uses it.

`DELETE /api/courses/{courseId}` (owner) → `204`.
Deletes the course, its memberships, its documents (including S3 objects), and its invite code. Its insights questions become unreachable at once and expire by TTL (180 days), so the delete stays fast.
Conversations stay with their owners; later reads of a deleted course return `404`.

`POST /api/courses/{courseId}/invite-code` (owner, empty body) → `200 { "inviteCode": string }`.
Replaces the invite code. The old code stops working at once.
Invite codes are 8 characters from `ABCDEFGHJKMNPQRSTUVWXYZ23456789` (no `0`, `O`, `1`, `I`, `L`). The API matches them case-insensitively and ignores spaces and hyphens. Failed join attempts are limited per user per day (`429 quota_exceeded`).

`POST /api/join` body `{ "inviteCode": string }` → `200 { "course": Course }`.
Joins as `student`. If the caller is already a member, returns the course with their existing role. Unknown code → `404`.

`GET /api/courses/{courseId}/members` (owner) → `200 { "members": Member[] }`: the owner first, then students by `joinedAt`, oldest first.

`DELETE /api/courses/{courseId}/members/{userId}` → `204`.
An owner can remove a student. A student can remove only themselves (leave). Nobody can remove the owner (`409 conflict`).

### Documents

`GET /api/courses/{courseId}/documents` (member) → `200 { "documents": Document[] }`, newest first.
Owners see every status; students see only `ready` documents.

`POST /api/courses/{courseId}/documents` (owner) body `{ "filename": string, "sizeBytes": number, "contentType": "application/pdf" }` →
`201 { "document": Document, "upload": { "url": string, "method": "PUT", "headers": { "content-type": "application/pdf" }, "expiresAt": string } }`.
The document starts in `uploading`. The client must `PUT` the file bytes to `upload.url` with exactly `upload.headers` before `expiresAt` (15 minutes).
`413 too_large` above `maxUploadBytes`; `409 limit_reached` at `maxDocumentsPerCourse`.

`POST /api/courses/{courseId}/documents/{documentId}/complete` (owner, empty body) → `200 { "document": Document }`.
The API checks that the object exists in S3 and that its size matches `sizeBytes`, then sets the status to `queued` (phase 2 also sends the ingestion message).
Missing object → `409 conflict`. Calling it again on a document that is not `uploading` returns the document unchanged.

`DELETE /api/courses/{courseId}/documents/{documentId}` (owner) → `204`. Deletes the S3 objects and the record (phase 2 also deletes its vectors and BM25 entries).

### Conversations

`GET /api/courses/{courseId}/conversations` (member) → `200 { "conversations": Conversation[] }`: only the caller's own, most recently updated first (at most 100).

`GET /api/conversations/{conversationId}` → `200 { "conversation": Conversation, "messages": Message[] }`: the latest 200 messages, oldest first. Only the conversation owner; else `404`.

`DELETE /api/conversations/{conversationId}` → `204`. Only the conversation owner.

### Chat

`POST /api/courses/{courseId}/chat` (member) body `{ "conversationId"?: string, "message": string, "model"?: string }` → `200 text/event-stream`.

- `message` is 1..4000 characters after trim.
- Without `conversationId`, the API creates a conversation. Its title is the first 60 characters of the message (cut at a word boundary when possible).
- With `conversationId`, it must belong to the caller and to this course, else `404`.
- `model` is optional and must be a catalog ID (`GET /api/models`), else `400`. Any member may pick any catalog model. Without it, the API uses the conversation's model, or for a new conversation the course default. The conversation's `model` becomes the model used.
- The API checks the per-user daily quota and the global daily cap before it calls the model. Those failures, and all validation and auth failures, are normal JSON error responses, not stream events.
- The server builds the prompt: tutoring style, course name and description, conversation history (the last 20 messages), and (phase 3) retrieved sources. Clients never send a system prompt, a raw Bedrock model ID, or a token limit; they can only name a catalog model.

Stream events, in order (each is `event: <name>` plus one `data: <json>` line):

| Event | Data | Notes |
|---|---|---|
| `meta` | `{ "conversationId", "userMessageId", "assistantMessageId", "title", "model" }` | Always first; `model` is the catalog ID in use |
| `sources` | `{ "sources": Source[] }` | Phase 3, before the first `delta`; omitted when there are no sources |
| `delta` | `{ "text": string }` | Zero or more |
| `done` | `{ "usage": { "inputTokens": number, "outputTokens": number }, "stopReason": string }` | Last event on success |
| `error` | `{ "code": string, "message": string }` | Last event when the model fails or times out after the stream started. `code` is `model_unavailable` or `internal` |

The server also sends SSE comment lines (`: keep-alive`) at least every 15 seconds.
The API saves the user message before it calls the model, and saves the assistant message (with sources and usage) before it sends `done`, even if the client disconnects.

### Insights

`GET /api/courses/{courseId}/insights` (owner) → `200 { "questions": Question[], "totals": { "questionsLast7Days": number, "activeStudentsLast7Days": number } }`.
`questions` are the latest 100 questions from students (not from the owner), newest first.
The API records one question for each chat message a student sends.

## Web runtime config

The web app loads `/config.json` before it renders. The deployed stack writes this file. Locally, each stack (`make dev`, `make e2e-local`) has its own file under `.local/` (git-ignored), and the Vite dev server serves the file named by `OPENBOOK_WEB_CONFIG` at `/config.json`.

```json
{
  "stage": "dev",
  "region": "ap-southeast-2",
  "cognito": { "userPoolId": "ap-southeast-2_xxxx", "clientId": "xxxx", "endpoint": null }
}
```

`cognito.endpoint` is `null` for deployed stages. Locally it is the relative path `"/__aws/"`, which the Vite dev server proxies to AWS Emulator. The web app resolves it against `location.origin` and passes the absolute URL to `CognitoUserPool` as `endpoint`, so the file does not depend on the port.
