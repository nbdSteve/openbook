import * as vm from 'vm';
import { describe, expect, test } from 'vitest';
import { spaRewriteCode } from '../lib/spa-rewrite';

type Handler = (event: { request: { uri: string } }) => { uri: string };

const sandbox: { handler?: Handler } = {};
vm.createContext(sandbox);
vm.runInContext(`${spaRewriteCode}\nthis.handler = handler;`, sandbox);
const rewrite = (uri: string): string => (sandbox.handler as Handler)({ request: { uri } }).uri;

describe('the SPA rewrite function', () => {
  test.each(['/', '/courses', '/courses/01HZZZ', '/join/', '/a.b/c'])('serves the route %s from /index.html', (uri) => {
    expect(rewrite(uri)).toBe('/index.html');
  });

  test.each(['/index.html', '/config.json', '/assets/app-abc123.js', '/assets/font.woff2', '/favicon.ico'])(
    'leaves the file %s alone',
    (uri) => {
      expect(rewrite(uri)).toBe(uri);
    },
  );

  test('does not use syntax that the CloudFront JS 2.0 runtime lacks', () => {
    expect(spaRewriteCode).not.toMatch(/=>|\bawait\b|\bimport\b|\bclass\b/);
  });
});
