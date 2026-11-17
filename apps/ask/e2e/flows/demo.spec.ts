import { expect, test, type APIRequestContext } from '@playwright/test';
import { DEPLOYED } from './helpers';

// The public demo, at the API level: no sign-in, a streamed answer with sources, and a follow-up on
// the conversation that the server keeps. The local stack provisions the demo course with the fake
// models before the tests start (scripts/e2e-local.sh), and the deployed demo needs a real provision.
test.skip(DEPLOYED, 'The demo flow is checked on the local stack. A deployed stack has its own limits.');

interface SseEvent {
  event: string;
  data: Record<string, unknown>;
}

function parseSse(text: string): SseEvent[] {
  const events: SseEvent[] = [];
  for (const block of text.split('\n\n')) {
    const event = /^event: (.+)$/m.exec(block)?.[1];
    const data = /^data: (.+)$/m.exec(block)?.[1];
    if (event && data) events.push({ event, data: JSON.parse(data) as Record<string, unknown> });
  }
  return events;
}

async function ask(request: APIRequestContext, body: Record<string, unknown>) {
  const payload = JSON.stringify(body);
  return request.post('/api/demo/chat', {
    // No x-auth-token: the demo is anonymous by design.
    headers: { 'content-type': 'application/json' },
    data: payload,
  });
}

test('the config says that the demo is on, with credits and examples', async ({ request }) => {
  const config = (await (await request.get('/config.json')).json()) as {
    demo: { enabled: boolean; model: { label: string }; sources: { title: string; license: string; url: string }[]; examples: string[] };
  };
  expect(config.demo.enabled).toBe(true);
  expect(config.demo.model.label).not.toBe('');
  expect(config.demo.sources.length).toBeGreaterThanOrEqual(2);
  for (const source of config.demo.sources) {
    expect(source.license).toBe('CC BY 4.0');
    expect(source.url).toMatch(/^https:\/\/openstax\.org\//);
  }
  expect(config.demo.examples.length).toBeGreaterThanOrEqual(4);
  expect(config.demo.examples.length).toBeLessThanOrEqual(6);
  for (const example of config.demo.examples) expect(example.trim().split(/\s+/).length).toBeLessThanOrEqual(5);
});

test('a visitor without an account gets a streamed answer with sources, then a follow-up', async ({ request }) => {
  const first = await ask(request, { message: 'What is the product rule of exponents?' });
  expect(first.status()).toBe(200);
  expect(first.headers()['content-type']).toContain('text/event-stream');
  const events = parseSse(await first.text());
  const names = events.map((e) => e.event);
  expect(names[0]).toBe('meta');
  expect(names[1]).toBe('sources');
  expect(names.at(-1)).toBe('done');
  expect(names.slice(2, -1).every((n) => n === 'delta')).toBe(true);

  const meta = events[0].data as { conversationId: string; model: string; attribution: { license: string }[] };
  expect(meta.conversationId).toMatch(/^[0-9a-f]{48}$/);
  expect(meta.attribution.length).toBeGreaterThanOrEqual(2);
  expect(meta.attribution.every((a) => a.license === 'CC BY 4.0')).toBe(true);
  const sources = (events[1].data as { sources: { documentName: string; page?: number; snippet: string }[] }).sources;
  expect(sources.length).toBeGreaterThan(0);
  expect(sources[0].snippet.length).toBeGreaterThan(0);
  const answer = events
    .filter((e) => e.event === 'delta')
    .map((e) => e.data.text as string)
    .join('');
  expect(answer.length).toBeGreaterThan(0);

  // The follow-up carries the conversation ID, and the server remembers the first turn.
  const second = await ask(request, { message: 'Can you give an example?', conversationId: meta.conversationId });
  expect(second.status()).toBe(200);
  const followUp = parseSse(await second.text());
  expect(followUp[0].data.conversationId).toBe(meta.conversationId);
  expect(followUp.at(-1)?.event).toBe('done');
});

test('a question outside the materials still gets an answer, and bad input gets an error', async ({ request }) => {
  const outside = await ask(request, { message: 'xyzzy plugh quux' });
  expect(outside.status()).toBe(200);
  const events = parseSse(await outside.text());
  expect(events.map((e) => e.event)).not.toContain('sources');
  expect(events.at(-1)?.event).toBe('done');

  for (const body of [{ message: '' }, { message: 'x'.repeat(301) }, { message: 'hi', history: [] }]) {
    const bad = await ask(request, body);
    expect(bad.status()).toBe(400);
    expect(((await bad.json()) as { error: { code: string } }).error.code).toBe('invalid_request');
  }
});

test('the other API routes still need a sign-in', async ({ request }) => {
  for (const path of ['/api/me', '/api/courses', '/api/models']) {
    expect((await request.get(path)).status(), path).toBe(401);
  }
});
