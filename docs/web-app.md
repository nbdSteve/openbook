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

- The config block is `demo` in `/config.json`: `enabled`, `model.label`, `sources` (`title`, `license`, `url`), and `examples`. When it is missing or `enabled` is false, the card shows the fixed example answer and a sign-up button.
- **The example answer is built at build time.** The Vite plugin `exampleAnswerPlugin` runs the same Markdown, math, and KaTeX plugins on the constant in `src/pages/landing/exampleContent.ts` and emits a HAST tree (`virtual:example-answer`). The page turns the tree into React elements with `hast-util-to-jsx-runtime`. It never inserts an HTML string, so the style attributes of the math are set through the DOM, which a strict CSP allows. The tree is our own constant, never model output, so there is nothing to sanitize. The first view needs no Markdown parser.
- **The interactive renderer loads late.** The Markdown and KaTeX renderer (the `DemoThread` chunk) loads when the visitor first focuses the box or asks, not on idle. Answers use the same bubble as the course chat.
- A visitor can ask follow-ups. The `conversationId` from the `meta` event goes back with each question. After 10 turns or 24 hours, the client starts a new conversation and says so. When the server answers with a different ID (it forgot the old one), the notice says so too.
- A refusal before the stream starts (`demo_rate_limited`, `demo_busy`, `demo_unavailable`, or a validation error) shows a friendly note under the box. The question is taken back, so the visitor can try again. A 429 or a 5xx response without the header `x-openbook-demo` did not come from the demo route (Lambda's own throttle, a CloudFront 502 or 504), and counts as busy. So does a 503 without a JSON error body.
- After the first event, the question and the partial answer always stay. A dropped connection, an `error` event, or a stream that ends without `done` marks the answer failed. A 200 response with no event at all does the same, so the bubble never thinks forever.
- The credit line under the card comes from the `attribution` of the last `meta` event, and before that from `demo.sources`. It names the title, the publisher, and the licence of each book.

## The tutoring style picker

`StylePicker` (`src/components/chat/StylePicker.tsx`) sits next to the model picker in the chat composer, and in the landing demo (the demo opens its list downward). It is a menu button: the button shows the current style, and the menu lists the five styles with a line each. The arrow keys, Home, and End move, Enter or Space picks, Escape closes the menu and returns the focus to the button, and Tab leaves it. The items are `menuitemradio` buttons, not form fields, so a key press cannot send the composer form. The menu moves left when it would leave the screen on a phone.

- **The state** is in `useChatController`: the style of the next message is the picked style, else the style of the conversation (from `GET /api/conversations/{id}` and the `meta` event), else the course default. The choice applies from the next message, and the conversation keeps it. A new chat starts with the course default. The picker is off while an answer streams, like the model picker.
- **The lock.** When the course has `allowStyleChoice: false`, the picker shows the course style with a lock icon and the reason ("Fixed by your teacher", or "Fixed in course settings" for the owner). It does not open. It uses `aria-disabled`, not `disabled`, so it stays in the tab order and a screen reader reads the reason. The chat then sends no `style`.
- **Portal, Settings** has the checkbox "Let learners change the style". The style field above it is the default.
- **The demo** starts in `DEMO_DEFAULT_STYLE` (`src/lib/constants.ts`), which a test keeps equal to `tutoringStyle` in `demo/course.json`. It sends `style` only after the visitor picks one, and forgets the pick on Start over. The control has no extra text, and it sits before the example questions.

## Benchmarks on the landing page

The page shows the retrieval numbers of the commit that it is built from. The Vite plugin `benchmarksPlugin` (in `vite.config.ts`) reads `evals/benchmarks.json` at build time and checks it with `parseBenchmarks` (`src/benchmarks/parse.ts`, the only reader of the file). A missing file, a bad number, or a renamed key fails the build, with the command that re-creates the file (`cargo run -p openbook-eval -- offline --write-baseline`). The plugin emits only the numbers that the page shows (`toLanding`): recall at 8 for each search method, recall at 1 of hybrid, the off-topic gate, the question count, and the number of textbooks. The page never shows nDCG, MRR, or the false-gate rate, although the reader keeps them. There is no fallback to sample numbers. A unit test keeps a fixture for the error paths and also parses the real file.

## Behavior to know

- **Titles and focus.** Every screen sets the tab title to "<screen> · Openbook". After a move to another page (not a new chat in the same course), the focus goes to the h1 (`src/routes/RouteFocus.tsx`).
- **The shell never scrolls.** The course shell is `h-dvh` with `overflow-hidden`, and the chat column is the only scroller. Text that is hidden from the eye (`sr-only`, which is `position: absolute`) must have a positioned parent inside that scroller. Without one, its parent is the page, so it stays at its place in the unscrolled column, far below the window. The document then grew taller than the window, and a wheel over the sidebar scrolled the whole shell up and left an empty band under the composer. The shell and the chat scroller are `relative` for this reason. Keep that when you add a new shell or scroller.
- **Leaving a chat while an answer streams.** The page stops reading the stream, because the chat controller lives in the course layout and a move to Portal unmounts it. The API still saves the whole answer, so it shows again from the saved conversation when the user comes back to that chat.
- **Inline styles.** A test with a strict policy (`style-src 'self'`, no `unsafe-inline`) found two things that still need it. First, `react-hot-toast` adds a `<style>` element at run time (`style-src-elem`), so toasts would lose their style. Second, a streamed answer with math raises `style-src-attr` violations from the Markdown and KaTeX path. The React `style` props of the app itself, the code blocks, the landing page, and the example answer raised none. So `unsafe-inline` must stay until the toast library is replaced (or given a nonce) and the math path is fixed.

## Tests

- `npm test`: unit tests, including the demo client, the demo state, the benchmarks reader, the legacy redirects, and the citation parser.
- `npm run e2e:visual`: the visual suite. It mocks the backend with `page.route`, takes a screenshot of every screen in both themes at 375 px and 1280 px, and runs axe on each one. Set `OPENBOOK_SCREENSHOT_DIR` to write the screenshots outside the repository. It also has `layout.spec.ts`: after two long streamed answers and wheel scrolling, the window must not scroll, the document must be as tall as the window, and the shell must end at the bottom of the window. It checks that the Ask, Portal, and landing screens have no hidden content below the page. It has screens for the style picker (open, changed, and locked) and a screen for each state of the demo: example, answered, streaming, not covered, rate limited, busy, unavailable, invalid, and error.
- `make e2e-local`: the end-to-end flows against the full local stack.
