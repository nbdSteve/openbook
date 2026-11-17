import * as fs from 'fs';
import * as path from 'path';
import * as vm from 'vm';
import { describe, expect, test } from 'vitest';
import { resolveConfig } from '../lib/config';
import { redirectStatus, viewerRequestCode, type HostRedirect } from '../lib/spa-rewrite';
import { asArray, cdkJsonContext, newApp, synth, type Json } from './helpers';

// ---- The CloudFront Functions: the exact source runs in a sandbox -----------------------------

interface Item {
  value: string;
  multiValue?: { value: string }[];
}
interface Event {
  request: { uri: string; headers: Record<string, { value: string }>; querystring: Record<string, Item> };
}
type Handler = (event: Event) => Json;

const compile = (code: string): Handler => {
  const sandbox: { handler?: Handler } = {};
  vm.createContext(sandbox);
  vm.runInContext(`${code}\nthis.handler = handler;`, sandbox);
  return sandbox.handler as Handler;
};

const redirects: HostRedirect[] = [
  { host: 'ask.steve.gg', target: 'https://openbook.steve.gg', keepPath: true },
  { host: 'portal.steve.gg', target: 'https://openbook.steve.gg/portal', keepPath: false },
];
const withRedirects = compile(viewerRequestCode(redirects, true));
const event = (host: string, uri: string, querystring: Record<string, Item> = {}): Event => ({
  request: { uri, headers: { host: { value: host } }, querystring },
});
const location = (out: Json): string => (out.headers as { location: { value: string } }).location.value;

describe('the host redirect', () => {
  test('is a 302 for now (switch to 301 after the release is verified)', () => {
    expect(redirectStatus).toBe(302);
    const out = withRedirects(event('ask.steve.gg', '/'));
    expect(out.statusCode).toBe(302);
    expect(out.statusDescription).toBe('Found');
  });

  test('ask.steve.gg keeps the path and the query string, so an invite link keeps its code', () => {
    for (const [uri, target] of [
      ['/', 'https://openbook.steve.gg/'],
      ['/join/ABCD2345', 'https://openbook.steve.gg/join/ABCD2345'],
      ['/app', 'https://openbook.steve.gg/app'],
      ['/app/c/01HZZZ/chat', 'https://openbook.steve.gg/app/c/01HZZZ/chat'],
      ['/login', 'https://openbook.steve.gg/login'],
      ['/assets/app.js', 'https://openbook.steve.gg/assets/app.js'],
      ['/a%20b/c', 'https://openbook.steve.gg/a%20b/c'],
    ]) {
      expect(location(withRedirects(event('ask.steve.gg', uri))), uri).toBe(target);
    }
    const single = withRedirects(event('ask.steve.gg', '/join/ABCD2345', { ref: { value: 'mail' } }));
    expect(location(single)).toBe('https://openbook.steve.gg/join/ABCD2345?ref=mail');
  });

  test('portal.steve.gg goes to /portal, whatever the old path', () => {
    for (const uri of ['/', '/courses/01HZZZ', '/join/ABCD2345', '/assets/app.js', '/index.html']) {
      expect(location(withRedirects(event('portal.steve.gg', uri))), uri).toBe('https://openbook.steve.gg/portal');
    }
  });

  test('keeps repeated keys, and a key without a value as it is (no = is added)', () => {
    const many = withRedirects(
      event('ask.steve.gg', '/x', {
        a: { value: '1', multiValue: [{ value: '1' }, { value: '2' }] },
        b: { value: 'x%20y' },
        flag: { value: '' },
        list: { value: '', multiValue: [{ value: '' }, { value: 'z' }] },
      }),
    );
    expect(location(many)).toBe('https://openbook.steve.gg/x?a=1&a=2&b=x%20y&flag&list&list=z');
    // No query: no question mark.
    expect(location(withRedirects(event('ask.steve.gg', '/x')))).toBe('https://openbook.steve.gg/x');
  });

  test('never redirects /api, on any host', () => {
    for (const host of ['ask.steve.gg', 'portal.steve.gg']) {
      for (const uri of ['/api', '/api/', '/api/me', '/api/demo/chat', '/api/courses/x/chat']) {
        const out = withRedirects(event(host, uri));
        expect(out.statusCode, `${host}${uri}`).toBeUndefined();
        // And it is not rewritten to the SPA either: the API behaviors do not use this function, but
        // a request that reached it must pass unchanged in the redirect step.
      }
    }
    // A path that only starts like it is not the API.
    expect(withRedirects(event('ask.steve.gg', '/apiary')).statusCode).toBe(302);
    expect(withRedirects(event('ask.steve.gg', '/api-docs/x')).statusCode).toBe(302);
  });

  test('normalizes the host: case, :443, and a trailing dot', () => {
    for (const host of ['ASK.Steve.GG', 'ask.steve.gg:443', 'ask.steve.gg.', 'Ask.Steve.Gg.:443']) {
      expect(withRedirects(event(host, '/join/X')).statusCode, host).toBe(302);
      expect(location(withRedirects(event(host, '/join/X'))), host).toBe('https://openbook.steve.gg/join/X');
    }
  });

  test('leaves every other host alone', () => {
    for (const host of ['openbook.steve.gg', 'd111111abcdef8.cloudfront.net', 'constructor', '__proto__', 'ask.steve.gg.evil.test', 'ask.steve.gg:8443', 'xask.steve.gg', '']) {
      expect(withRedirects(event(host, '/x')).statusCode, host).toBeUndefined();
    }
    // A request without a Host header (not possible through CloudFront) passes too.
    const noHost: Event = { request: { uri: '/x', headers: {}, querystring: {} } };
    expect(withRedirects(noHost).uri).toBe('/index.html');
  });

  test('the SPA rewrite serves index.html for the site, /ask/*, /portal/*, and /join/*, and leaves files alone', () => {
    for (const uri of ['/', '/ask', '/ask/', '/ask/lesson/01HZZZ', '/portal', '/portal/courses/01HZZ/documents', '/join/ABCD2345', '/signin', '/a.b/c']) {
      expect(withRedirects(event('openbook.steve.gg', uri)).uri, uri).toBe('/index.html');
    }
    for (const uri of ['/index.html', '/config.json', '/assets/app-abc123.js', '/ask/assets/logo.svg', '/favicon.ico']) {
      expect(withRedirects(event('openbook.steve.gg', uri)).uri, uri).toBe(uri);
    }
  });

  test('a stage with no redirects has no redirect code, and the plain SPA rewrite stays', () => {
    const code = viewerRequestCode([], true);
    expect(code).not.toContain('REDIRECTS');
    expect(compile(code)(event('anything.test', '/x')).uri).toBe('/index.html');
  });

  test('does not use syntax that the CloudFront JS 2.0 runtime lacks', () => {
    for (const code of [viewerRequestCode(redirects, true), viewerRequestCode(redirects, false)]) {
      expect(code).not.toMatch(/=>|\bawait\b|\bimport\b|\bclass\b|\blet\b|\bconst\b/);
    }
  });
});

// ---- The stack --------------------------------------------------------------------------------

const prod = synth({ stage: 'prod' });
const dev = synth({ stage: 'dev' });
const resourcesOf = (s: ReturnType<typeof synth>, type: string): [string, Json][] =>
  Object.entries(s.json.Resources)
    .filter(([, r]) => r.Type === type)
    .map(([id, r]) => [id, (r.Properties ?? {}) as Json]);
const functionByMode = (s: ReturnType<typeof synth>, mode: string): Json =>
  resourcesOf(s, 'AWS::Lambda::Function').find(
    ([, p]) => ((p.Environment as { Variables: Record<string, string> } | undefined)?.Variables ?? {}).OPENBOOK_API_MODE === mode,
  )?.[1] as Json;
const envOf = (fn: Json): Record<string, string> => (fn.Environment as { Variables: Record<string, string> }).Variables;

describe('the domain of the prod stage', () => {
  const domains = (cdkJsonContext.stageDomains as { prod: { domainName: string; hostedZoneName: string; redirects: { host: string; path: string }[] } }).prod;

  test('is the site with two redirect hosts in cdk.json', () => {
    expect(resolveConfig(newApp({ ...cdkJsonContext, stage: 'prod' }).node).domain).toEqual({
      domainName: 'openbook.steve.gg',
      hostedZoneName: 'steve.gg',
      redirects: [
        { host: 'ask.steve.gg', path: '/', keepPath: true, recordIdPrefix: 'Alias' },
        { host: 'portal.steve.gg', path: '/portal' },
      ],
    });
    expect(domains.redirects.map((r) => r.path)).toEqual(['/', '/portal']);
  });

  test('the distribution has all three aliases', () => {
    const [[, dist]] = resourcesOf(prod, 'AWS::CloudFront::Distribution');
    expect((dist.DistributionConfig as { Aliases: string[] }).Aliases).toEqual(['openbook.steve.gg', 'ask.steve.gg', 'portal.steve.gg']);
  });

  test('has an A and an AAAA record for each name, and the ask records keep their logical IDs', () => {
    const records = resourcesOf(prod, 'AWS::Route53::RecordSet');
    const byId = Object.fromEntries(records.map(([id, p]) => [id, `${String(p.Name)} ${String(p.Type)}`]));
    // These two IDs are what the stack had before the move (`AliasA` and `AliasAaaa` for ask.steve.gg).
    // A new ID for ask.steve.gg would make CloudFormation create a second record before it deletes the first.
    expect(byId).toEqual({
      AliasAEAD92310: 'ask.steve.gg. A',
      AliasAaaa2919B812: 'ask.steve.gg. AAAA',
      OpenbookAliasA20271AB0: 'openbook.steve.gg. A',
      OpenbookAliasAaaa38C41430: 'openbook.steve.gg. AAAA',
      PortalAliasAC72FE65F: 'portal.steve.gg. A',
      PortalAliasAaaaD92C9CF1: 'portal.steve.gg. AAAA',
    });
    for (const [, p] of records) {
      expect(p.HostedZoneId).toEqual({ Ref: 'HostedZoneId' });
      expect((p.AliasTarget as { DNSName: unknown }).DNSName).toBeDefined();
    }
  });

  test('SiteUrl is the new site, and CORS allows it and the cloudfront.net name, and not ask.steve.gg', () => {
    const outputs = (prod.json as unknown as { Outputs: { SiteUrl: { Value: string } } }).Outputs;
    expect(outputs.SiteUrl.Value).toBe('https://openbook.steve.gg');
    const docs = resourcesOf(prod, 'AWS::S3::Bucket').find(([, p]) => p.CorsConfiguration)?.[1] as Json;
    const origins = (docs.CorsConfiguration as { CorsRules: { AllowedOrigins: unknown[] }[] }).CorsRules[0].AllowedOrigins;
    expect(origins).toHaveLength(2);
    expect(JSON.stringify(origins)).toContain('https://openbook.steve.gg');
    expect(JSON.stringify(origins)).not.toContain('ask.steve.gg');
    expect(JSON.stringify(origins)).not.toContain('portal.steve.gg');
  });

  test('the default behavior has the one viewer-request function, and the API behaviors have none', () => {
    const [[, dist]] = resourcesOf(prod, 'AWS::CloudFront::Distribution');
    const config = dist.DistributionConfig as { DefaultCacheBehavior: Json; CacheBehaviors: Json[] };
    const associations = (b: Json): { EventType: string; FunctionARN: { 'Fn::GetAtt': string[] } }[] =>
      (b.FunctionAssociations ?? []) as { EventType: string; FunctionARN: { 'Fn::GetAtt': string[] } }[];
    const defaults = associations(config.DefaultCacheBehavior);
    expect(defaults.map((f) => f.EventType)).toEqual(['viewer-request']);
    expect(defaults[0].FunctionARN['Fn::GetAtt'][0]).toMatch(/^SpaRewrite/);
    expect(config.CacheBehaviors.map((b) => b.PathPattern)).toEqual(['/api/demo/*', '/api/*']);
    // The API is never redirected and never rewritten, and no behavior has a viewer-response function
    // (CloudFront does not run one when the origin answers 400 or higher).
    for (const behavior of config.CacheBehaviors) expect(associations(behavior)).toEqual([]);
    expect(resourcesOf(prod, 'AWS::CloudFront::Function')).toHaveLength(1);
  });

  test('the function holds the redirects: ask keeps the path, portal goes to /portal, with a 302', () => {
    const code = resourcesOf(prod, 'AWS::CloudFront::Function').find(([id]) => id.startsWith('SpaRewrite'))?.[1].FunctionCode as string;
    expect(code).toContain('"ask.steve.gg":{"target":"https://openbook.steve.gg","keepPath":true}');
    expect(code).toContain('"portal.steve.gg":{"target":"https://openbook.steve.gg/portal","keepPath":false}');
    expect(code).toContain('statusCode: 302');
    expect(code).toContain('/index.html');
  });

  test('dev has no domain, no redirect function, and no host redirect in its default function', () => {
    const names = resourcesOf(dev, 'AWS::CloudFront::Function').map(([id]) => id.replace(/[0-9A-F]{8}$/, ''));
    expect(names.sort()).toEqual(['SpaRewrite']);
    expect(JSON.stringify(dev.json)).not.toContain('steve.gg');
  });
});

describe('the redirect settings of a domain are checked', () => {
  const ctx = (redirects: unknown) => ({
    stage: 'prod',
    stageDomains: { prod: { domainName: 'openbook.example.test', hostedZoneName: 'example.test', redirects } },
  });
  test('a good list is accepted', () => {
    expect(() => synth(ctx([{ host: 'ask.example.test', path: '/', keepPath: true }, { host: 'portal.example.test', path: '/portal' }]))).not.toThrow();
  });
  test.each([
    [[{ host: 'ask.other.test', path: '/ask' }], /hosted zone/],
    [[{ host: 'openbook.example.test', path: '/ask' }], /twice or is the site/],
    [[{ host: 'a.example.test', path: '/a' }, { host: 'a.example.test', path: '/b' }], /twice/],
    [[{ host: 'a.example.test', path: 'ask' }], /path like/],
    [[{ host: 'a.example.test', path: '/a/b' }], /path like/],
    [[{ host: 'a.example.test', path: '/a', keepPath: true }], /must be \//],
    [[{ host: 'a.example.test', path: '/', keepPath: 'yes' }], /keepPath must be/],
    [[{ host: 'A.example.test', path: '/a' }], /lower case DNS name/],
    [[{ host: 'a.example.test', path: '/a', recordIdPrefix: 'x y' }], /recordIdPrefix/],
    ['ask.example.test', /must be a list/],
  ])('refuses %j', (redirects, message) => {
    expect(() => synth(ctx(redirects))).toThrow(message);
  });
});

describe('the demo function', () => {
  test.each([
    ['prod', prod, 3],
    ['dev', dev, 2],
  ])('has its own small reserved concurrency in %s', (_stage, s, concurrency) => {
    const demo = functionByMode(s, 'demo');
    const api = functionByMode(s, 'full');
    expect(demo.ReservedConcurrentExecutions).toBe(concurrency);
    expect(api.ReservedConcurrentExecutions).toBe(_stage === 'prod' ? 25 : 5);
    expect(demo.ReservedConcurrentExecutions).toBeLessThan(api.ReservedConcurrentExecutions as number);
  });

  test('the concurrency, the output limit, and the visitor cap come from the context', () => {
    const s = synth({ stage: 'prod', demoConcurrency: '1', demoMaxOutputTokens: '500', demoVisitorBudgetUsdPerDay: '0.1' });
    const demo = functionByMode(s, 'demo');
    expect(demo.ReservedConcurrentExecutions).toBe(1);
    expect(envOf(demo).OPENBOOK_DEMO_MAX_OUTPUT_TOKENS).toBe('500');
    expect(envOf(demo).OPENBOOK_DEMO_VISITOR_BUDGET_USD_PER_DAY).toBe('0.1');
    expect(() => synth({ demoConcurrency: '0' })).toThrow(/demoConcurrency/);
    expect(() => synth({ demoMaxOutputTokens: '1.5' })).toThrow(/demoMaxOutputTokens/);
    expect(() => synth({ demoVisitorBudgetUsdPerDay: 'free' })).toThrow(/demoVisitorBudgetUsdPerDay/);
  });

  test.each([
    ['prod', prod],
    ['dev', dev],
  ])('%s: the demo settings are in the demo function only, and the main API has none', (_stage, s) => {
    const demo = envOf(functionByMode(s, 'demo'));
    const api = envOf(functionByMode(s, 'full'));
    expect(demo.OPENBOOK_DEMO_LIMIT_VISITOR_PER_10_MIN).toBe('10');
    expect(demo.OPENBOOK_DEMO_LIMIT_VISITOR_PER_DAY).toBe('40');
    expect(demo.OPENBOOK_DEMO_BUDGET_USD_PER_DAY).toBe('2');
    expect(demo.OPENBOOK_DEMO_VISITOR_BUDGET_USD_PER_DAY).toBe('0.3');
    expect(demo.OPENBOOK_DEMO_MAX_OUTPUT_TOKENS).toBe('1000');
    // The switch comes from stageDemo in cdk.json, so turning the demo on or off is a one-line change there.
    const stageDemo = (cdkJsonContext.stageDemo as Record<string, { enabled: boolean }>)[_stage];
    expect(demo.OPENBOOK_DEMO_ENABLED).toBe(String(stageDemo.enabled));
    expect(Object.keys(api).filter((k) => k.startsWith('OPENBOOK_DEMO'))).toEqual([]);
    expect(JSON.stringify(demo).length).toBeLessThan(4096);
    expect(JSON.stringify(api).length).toBeLessThan(4096);
  });

  test('has a function URL with IAM auth and streaming, behind its own behavior with OAC', () => {
    const urls = resourcesOf(prod, 'AWS::Lambda::Url');
    expect(urls).toHaveLength(2);
    for (const [, p] of urls) {
      expect(p.AuthType).toBe('AWS_IAM');
      expect(p.InvokeMode).toBe('RESPONSE_STREAM');
    }
    const [[, dist]] = resourcesOf(prod, 'AWS::CloudFront::Distribution');
    const config = dist.DistributionConfig as { CacheBehaviors: Json[]; Origins: Json[] };
    const [demoBehavior, apiBehavior] = config.CacheBehaviors;
    expect(demoBehavior.PathPattern).toBe('/api/demo/*');
    expect(demoBehavior.TargetOriginId).not.toBe(apiBehavior.TargetOriginId);
    for (const b of [demoBehavior, apiBehavior]) {
      expect(b.CachePolicyId).toBe('4135ea2d-6df8-44a3-9df3-4b5a84be39ad');
      // The managed policy "all viewer except Host" forwards CloudFront-Viewer-Address.
      expect(b.OriginRequestPolicyId).toBe('b689b0a8-53d0-40ab-baf2-68738e2966ac');
      expect(b.ViewerProtocolPolicy).toBe('https-only');
    }
    expect(resourcesOf(prod, 'AWS::CloudFront::OriginRequestPolicy')).toHaveLength(0);
    // Both origins are signed with OAC.
    const withOac = config.Origins.filter((o) => o.OriginAccessControlId !== undefined && o.CustomOriginConfig !== undefined);
    expect(withOac).toHaveLength(2);
    // CloudFront may invoke each function URL, and only through the distribution.
    const permissions = resourcesOf(prod, 'AWS::Lambda::Permission').filter(([, p]) => p.Principal === 'cloudfront.amazonaws.com');
    expect(permissions.map(([, p]) => p.Action).sort()).toEqual(['lambda:InvokeFunction', 'lambda:InvokeFunction', 'lambda:InvokeFunctionUrl', 'lambda:InvokeFunctionUrl']);
  });

  test('its role has the least that a turn needs, and nothing of the main API or the worker', () => {
    const policies = resourcesOf(prod, 'AWS::IAM::Policy').filter(([id]) => id.startsWith('DemoFunctionServiceRole'));
    expect(policies).toHaveLength(1);
    const statements = (policies[0][1].PolicyDocument as { Statement: Json[] }).Statement;
    const bySid = (sid: string): Json => statements.find((s) => s.Sid === sid) as Json;
    expect(asArray(bySid('DemoReadItems').Action as string[]).sort()).toEqual(['dynamodb:GetItem', 'dynamodb:Query']);
    // Only the demo items and the courses whose ID starts with `demo`: never the course of a user.
    expect(bySid('DemoReadItems').Condition).toEqual({
      'ForAllValues:StringLike': { 'dynamodb:LeadingKeys': ['DEMO#*', 'COURSE#demo*'] },
    });
    expect(asArray(bySid('DemoWriteItems').Action as string[]).sort()).toEqual(['dynamodb:DeleteItem', 'dynamodb:PutItem', 'dynamodb:UpdateItem']);
    expect(bySid('DemoWriteItems').Condition).toEqual({ 'ForAllValues:StringLike': { 'dynamodb:LeadingKeys': ['DEMO#*'] } });
    expect(bySid('DemoReadBm25Indexes').Action).toBe('s3:GetObject');
    expect(JSON.stringify(bySid('DemoReadBm25Indexes').Resource)).toContain('/courses/demo*/bm25/*');
    expect(JSON.stringify(bySid('DemoQueryCourseVectors').Resource)).toContain('/index/c-demo*');
    expect(asArray(bySid('DemoQueryCourseVectors').Action as string[]).sort()).toEqual(['s3vectors:GetVectors', 's3vectors:QueryVectors']);
    const actions = statements.flatMap((s) => asArray(s.Action as string | string[]));
    // No queue, no writes of files or vectors, no index or batch writes, no invalidation.
    for (const forbidden of ['sqs:', 's3:PutObject', 's3:DeleteObject', 's3:ListBucket', 'dynamodb:BatchWriteItem', 'dynamodb:BatchGetItem', 'cloudfront:', 's3vectors:PutVectors', 's3vectors:CreateIndex']) {
      expect(actions.filter((a) => a.startsWith(forbidden)), forbidden).toEqual([]);
    }
    // The models are the default chat model, the fast model, and the embedding model, not gpt-oss.
    const text = JSON.stringify(statements);
    expect(text).toContain('claude-sonnet-4-6');
    expect(text).toContain('claude-haiku-4-5');
    expect(text).toContain('titan-embed-text-v2');
    expect(text).not.toContain('gpt-oss');
    // The role is under the permissions boundary in prod, as every role of the stack.
    const roles = resourcesOf(prod, 'AWS::IAM::Role').filter(([id]) => id.startsWith('DemoFunctionServiceRole'));
    expect(roles).toHaveLength(1);
    expect(roles[0][1].PermissionsBoundary).toBeDefined();
  });
});

describe('the demo course ID and the IAM patterns of the demo role', () => {
  /** IAM `StringLike`: `*` matches any run of characters, and the rest is exact and case-sensitive. */
  const like = (pattern: string, value: string): boolean =>
    new RegExp(`^${pattern.split('*').map((p) => p.replace(/[.+?^${}()|[\]\\]/g, '\\$&')).join('.*')}$`).test(value);
  const demoId = 'demo01M7KXR2Q0RZNGKKBHQ2TQ9E8';
  const ulid = (): string => {
    const alphabet = '0123456789ABCDEFGHJKMNPQRSTVWXYZ';
    return Array.from({ length: 26 }, () => alphabet[Math.floor(Math.random() * 32)]).join('');
  };

  test('the patterns match the demo course and nothing that a normal course can be', () => {
    expect(like('COURSE#demo*', `COURSE#${demoId}`)).toBe(true);
    expect(like('courses/demo*/bm25/*', `courses/${demoId}/bm25/v2-X.tar.zst`)).toBe(true);
    expect(like('c-demo*', `c-${demoId.toLowerCase()}`)).toBe(true);
    for (let i = 0; i < 5000; i += 1) {
      const id = ulid();
      expect(like('COURSE#demo*', `COURSE#${id}`), id).toBe(false);
      expect(like('courses/demo*/bm25/*', `courses/${id}/bm25/v1-X.tar.zst`), id).toBe(false);
      expect(like('c-demo*', `c-${id.toLowerCase()}`), id).toBe(false);
    }
    // A ULID has no letter O, so even a clock that made it start with D, E, M cannot spell `demo`.
    expect('0123456789ABCDEFGHJKMNPQRSTVWXYZ').not.toContain('O');
    // The items of the demo are not the items of a user either.
    for (const other of ['USER#x', 'CONV#x', 'INVITE#x', 'GLOBAL']) expect(like('DEMO#*', other)).toBe(false);
  });

  test('the role reads the demo patterns, and the same patterns are in the stack and in the demo item', () => {
    const policies = resourcesOf(prod, 'AWS::IAM::Policy').filter(([id]) => id.startsWith('DemoFunctionServiceRole'));
    const text = JSON.stringify(policies[0][1].PolicyDocument);
    expect(text).toContain('COURSE#demo*');
    expect(text).not.toContain('COURSE#*');
    expect(text).not.toContain('courses/*/bm25');
    expect(text).not.toContain('index/c-*');
    // The Rust side makes the ID with this prefix (crates/core/src/ids.rs: DEMO_COURSE_PREFIX).
    const ids = fs.readFileSync(path.join(__dirname, '../../crates/core/src/ids.rs'), 'utf8');
    expect(ids).toContain('pub const DEMO_COURSE_PREFIX: &str = "demo";');
  });
});
