# Web app

The web app lives in `apps/ask`. It is React 19, Vite, and Tailwind 4. The product is Openbook. Ask and Portal are the two areas of the app, and the landing page is the front door.

## Routes

| Route | What |
|---|---|
| `/` | The Openbook landing page. Signed-in users go on to `/ask`. |
| `/signin`, `/signup`, `/verify`, `/forgot` | Sign-in pages, shared by both areas |
| `/join/:inviteCode` | The invite link. It asks the user to confirm, joins the course, and opens it in Ask. |
| `/ask` | Ask: the courses the user can ask in, joined or their own |
| `/ask/courses/:courseId` | The course chat. `/ask/courses/:courseId/c/:conversationId` opens one conversation. |
| `/portal` | Portal: the courses the user owns, and the buttons to make a new one |
| `/portal/courses/:courseId` | Manage one course: documents, invite, members, settings, insights. `?tab=` picks the tab. Only the owner may open it. |

Anyone who is signed in can use Portal, because a learner makes a personal course there. An Ask | Portal switch sits in the header and at the top of the course sidebar. These old routes still work and redirect: `/login`, `/app`, `/app/c/:id`, `/app/c/:id/chat/:conversationId`, and `/app/c/:id/manage`. The list is in `src/routes/legacyRoutes.ts`.

## Design system

The look is a well-loved textbook with a small study buddy in the margin. The tokens are in `src/index.css`, in the Tailwind `@theme` block.

- **Color.** Warm paper and ink by day. Night reading by night: warm brown-black, never blue-black. One accent, ink blue. Every text pair meets WCAG AA, and the borders of form controls meet 3:1.
- **Type.** Newsreader (serif) for headings, questions, and answers. Instrument Sans for the interface. Both are self-hosted Latin subsets from `@fontsource-variable`, because the CSP allows `font-src 'self'` only. Answers use the reading size (`text-reading`) and a measure of 38rem.
- **Citations.** A citation is a footnote number in the text. The sources it cites are notes in the right margin on wide screens (a container query on the chat column), and a list under the answer on narrow screens. A source that was found but not cited sits behind "N more passages found".
- **Mascot.** `components/ui/Mascot.tsx` is an inline SVG cloud with four moods: idle, thinking (an answer streams), happy (an answer has sources), and puzzled (an answer has no sources). The animations are small and stop when the reader asks for less motion.
- **Motion.** Sections below the fold fade in once. The global `prefers-reduced-motion` rule turns all animation off.

## The landing page demo

The demo asks `POST /api/demo/chat` and shows a small version of the course chat. The only module that knows the endpoint name and its error codes is `src/services/demo/demoClient.ts`, so a rename is one change. The state is in `src/pages/landing/useDemoChat.ts`.

- The config block is `demo` in `/config.json`: `enabled`, `model.label`, `sources`, and `examples`. When it is missing or `enabled` is false, the card shows a fixed example answer and a sign-up button.
- The answer uses the same bubble as the course chat. The Markdown and KaTeX renderer is large, so the landing page loads it after the first paint (or when a visitor first asks), as the chunk `DemoThread`.
- A visitor can ask follow-ups. The `conversationId` from the `meta` event goes back with each question. After 10 turns or 24 hours, the client starts a new conversation and says so. If the server no longer knows the conversation, the client does the same once.
- A refusal before the stream starts (`demo_rate_limited`, `demo_busy`, `demo_unavailable`, or a validation error) shows a friendly note under the box. The question is taken back, so the visitor can try again.

## Benchmarks on the landing page

The page shows the retrieval numbers of the commit that it is built from. The Vite plugin `benchmarksPlugin` (in `vite.config.ts`) serves `evals/benchmarks.json` as the module `virtual:benchmarks`. `src/benchmarks/parse.ts` is the only reader of that file, so a schema change is a change there. The page shows only the pooled numbers: recall at 8, recall at 1, and the off-topic gate. It never shows nDCG or MRR.

Without the file, a developer machine falls back to `src/benchmarks/benchmarks.fixture.json`, and the page shows a "Sample numbers" tag. In CI (`CI` is set), a missing file fails the build.

## Tests

- `npm test`: unit tests, including the demo client, the demo state, the benchmarks reader, the legacy redirects, and the citation parser.
- `npm run e2e:visual`: the visual suite. It mocks the backend with `page.route`, takes a screenshot of every screen in both themes at 375 px and 1280 px, and runs axe on each one. Set `OPENBOOK_SCREENSHOT_DIR` to write the screenshots outside the repository. It has a screen for each state of the demo: example, answered, streaming, not covered, rate limited, busy, unavailable, invalid, and error.
- `make e2e-local`: the end-to-end flows against the full local stack.
