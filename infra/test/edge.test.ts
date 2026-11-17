import * as vm from 'vm';
import { describe, expect, test } from 'vitest';
import { resolveConfig } from '../lib/config';
import { demoResponseCode, viewerRequestCode, type HostRedirect } from '../lib/spa-rewrite';
import { exampleValues, readTemplate, renderIamDocument, type Values } from '../scripts/render-iam';
import { asArray, cdkJsonContext, fakeAccount, newApp, synth, type Json } from './helpers';

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
  { host: 'ask.steve.gg', target: 'https://openbook.steve.gg/ask' },
  { host: 'portal.steve.gg', target: 'https://openbook.steve.gg/portal' },
];
const withRedirects = compile(viewerRequestCode(redirects, true));
const redirectOnly = compile(viewerRequestCode(redirects, false));
const event = (host: string, uri: string, querystring: Record<string, Item> = {}): Event => ({
  request: { uri, headers: { host: { value: host } }, querystring },
});
const location = (out: Json): string => (out.headers as { location: { value: string } }).location.value;

describe('the host redirect', () => {
  test.each([
    ['ask.steve.gg', 'https://openbook.steve.gg/ask'],
    ['portal.steve.gg', 'https://openbook.steve.gg/portal'],
  ])('sends %s to %s with a 301', (host, target) => {
    for (const handler of [withRedirects, redirectOnly]) {
      const out = handler(event(host, '/'));
      expect(out.statusCode).toBe(301);
      expect(out.statusDescription).toBe('Moved Permanently');
      expect(location(out)).toBe(target);
    }
  });

  test('maps every old path to the area root', () => {
    for (const uri of ['/', '/courses/01HZZZ', '/join/ABCD2345', '/assets/app.js', '/api/me', '/api/courses/x/chat', '/index.html']) {
      expect(location(withRedirects(event('ask.steve.gg', uri)))).toBe('https://openbook.steve.gg/ask');
      expect(location(redirectOnly(event('portal.steve.gg', uri)))).toBe('https://openbook.steve.gg/portal');
    }
  });

  test('keeps the query string, including repeated keys', () => {
    const single = withRedirects(event('ask.steve.gg', '/x', { code: { value: 'ABCD2345' } }));
    expect(location(single)).toBe('https://openbook.steve.gg/ask?code=ABCD2345');
    const many = withRedirects(
      event('portal.steve.gg', '/', { a: { value: '1', multiValue: [{ value: '1' }, { value: '2' }] }, b: { value: 'x%20y' } }),
    );
    expect(location(many)).toBe('https://openbook.steve.gg/portal?a=1&a=2&b=x%20y');
  });

  test('matches the host in any case, and leaves every other host alone', () => {
    expect(withRedirects(event('ASK.Steve.GG', '/')).statusCode).toBe(301);
    for (const host of ['openbook.steve.gg', 'd111111abcdef8.cloudfront.net', 'constructor', '__proto__', 'ask.steve.gg.evil.test', '']) {
      expect(withRedirects(event(host, '/x')).statusCode, host).toBeUndefined();
    }
    // A request without a Host header (not possible through CloudFront) passes too.
    const noHost: Event = { request: { uri: '/x', headers: {}, querystring: {} } };
    expect(withRedirects(noHost).uri).toBe('/index.html');
  });

  test('the SPA rewrite serves index.html for the site, /ask/*, and /portal/*, and leaves files alone', () => {
    for (const uri of ['/', '/ask', '/ask/', '/ask/lesson/01HZZZ', '/portal', '/portal/courses/01HZZ/documents', '/courses', '/a.b/c']) {
      expect(withRedirects(event('openbook.steve.gg', uri)).uri, uri).toBe('/index.html');
    }
    for (const uri of ['/index.html', '/config.json', '/assets/app-abc123.js', '/ask/assets/logo.svg', '/favicon.ico']) {
      expect(withRedirects(event('openbook.steve.gg', uri)).uri, uri).toBe(uri);
    }
  });

  test('the redirect-only function never rewrites the path (API behaviors)', () => {
    for (const uri of ['/api/me', '/api/demo/chat', '/api/courses/x']) {
      expect(redirectOnly(event('openbook.steve.gg', uri)).uri).toBe(uri);
    }
  });

  test('a stage with no redirects has no redirect code, and the plain SPA rewrite stays', () => {
    const code = viewerRequestCode([], true);
    expect(code).not.toContain('REDIRECTS');
    expect(compile(code)(event('anything.test', '/x')).uri).toBe('/index.html');
  });

  test('does not use syntax that the CloudFront JS 2.0 runtime lacks', () => {
    for (const code of [viewerRequestCode(redirects, true), viewerRequestCode(redirects, false), demoResponseCode]) {
      expect(code).not.toMatch(/=>|\bawait\b|\bimport\b|\bclass\b|\blet\b|\bconst\b/);
    }
  });
});

describe('the demo response function', () => {
  const handler = compile(demoResponseCode) as unknown as (e: { response: Json }) => Json;
  const response = (statusCode: number, ours: boolean): { response: Json } => ({
    response: { statusCode, headers: ours ? { 'x-openbook-demo': { value: '1' } } : {} },
  });

  test.each([429, 500, 502, 503, 504])('turns a %s that is not ours (a throttle or a timeout) into demo_busy', (code) => {
    const out = handler(response(code, false));
    expect(out.statusCode).toBe(503);
    const body = JSON.parse((out.body as { data: string }).data) as { error: { code: string; message: string } };
    expect(body.error.code).toBe('demo_busy');
    expect(body.error.message).toContain('busy');
    expect((out.headers as Record<string, { value: string }>)['content-type'].value).toBe('application/json');
    expect((out.headers as Record<string, { value: string }>)['cache-control'].value).toBe('no-store');
  });

  test('leaves every answer of the demo route alone, errors included', () => {
    for (const code of [200, 400, 404, 429, 500, 503]) {
      const input = response(code, true);
      expect(handler(input)).toBe(input.response);
    }
    // A 4xx that is not a throttle stays too.
    const missing = response(404, false);
    expect(handler(missing)).toBe(missing.response);
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
        { host: 'ask.steve.gg', path: '/ask', recordIdPrefix: 'Alias' },
        { host: 'portal.steve.gg', path: '/portal' },
      ],
    });
    expect(domains.redirects.map((r) => r.path)).toEqual(['/ask', '/portal']);
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

  test('every behavior has a viewer-request function: the merged one on the site, the redirect on the API', () => {
    const [[, dist]] = resourcesOf(prod, 'AWS::CloudFront::Distribution');
    const config = dist.DistributionConfig as { DefaultCacheBehavior: Json; CacheBehaviors: Json[] };
    const functionIds = (b: Json, event: string): string[] =>
      (b.FunctionAssociations as { EventType: string; FunctionARN: { 'Fn::GetAtt': string[] } }[])
        .filter((f) => f.EventType === event)
        .map((f) => f.FunctionARN['Fn::GetAtt'][0]);
    const request = (b: Json) => functionIds(b, 'viewer-request');
    expect(request(config.DefaultCacheBehavior)).toEqual([expect.stringMatching(/^SpaRewrite/)]);
    expect(config.CacheBehaviors.map((b) => b.PathPattern)).toEqual(['/api/demo/*', '/api/*']);
    for (const behavior of config.CacheBehaviors) expect(request(behavior)).toEqual([expect.stringMatching(/^HostRedirect/)]);
    // One function of each kind per event: a behavior has a single viewer-request function.
    expect(functionIds(config.CacheBehaviors[0], 'viewer-response')).toEqual([expect.stringMatching(/^DemoResponse/)]);
    expect(functionIds(config.CacheBehaviors[1], 'viewer-response')).toEqual([]);
  });

  test('the merged function holds the redirects and the rewrite, and the targets are the areas of the site', () => {
    const code = (resourcesOf(prod, 'AWS::CloudFront::Function').find(([id]) => id.startsWith('SpaRewrite'))?.[1].FunctionCode as string);
    expect(code).toContain('https://openbook.steve.gg/ask');
    expect(code).toContain('https://openbook.steve.gg/portal');
    expect(code).toContain('/index.html');
  });

  test('dev has no domain, no redirect function, and no host redirect in its default function', () => {
    const names = resourcesOf(dev, 'AWS::CloudFront::Function').map(([id]) => id.replace(/[0-9A-F]{8}$/, ''));
    expect(names.sort()).toEqual(['DemoResponse', 'SpaRewrite']);
    expect(JSON.stringify(dev.json)).not.toContain('steve.gg');
  });
});

describe('the redirect settings of a domain are checked', () => {
  const ctx = (redirects: unknown) => ({
    stage: 'prod',
    stageDomains: { prod: { domainName: 'openbook.example.test', hostedZoneName: 'example.test', redirects } },
  });
  test('a good list is accepted', () => {
    expect(() => synth(ctx([{ host: 'ask.example.test', path: '/ask' }]))).not.toThrow();
  });
  test.each([
    [[{ host: 'ask.other.test', path: '/ask' }], /hosted zone/],
    [[{ host: 'openbook.example.test', path: '/ask' }], /twice or is the site/],
    [[{ host: 'a.example.test', path: '/a' }, { host: 'a.example.test', path: '/b' }], /twice/],
    [[{ host: 'a.example.test', path: 'ask' }], /path like/],
    [[{ host: 'a.example.test', path: '/a/b' }], /path like/],
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
    expect(demo.OPENBOOK_DEMO_ENABLED).toBe(_stage === 'dev' ? 'true' : 'false');
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
    expect(bySid('DemoReadItems').Condition).toEqual({
      'ForAllValues:StringLike': { 'dynamodb:LeadingKeys': ['DEMO#*', 'COURSE#*'] },
    });
    expect(asArray(bySid('DemoWriteItems').Action as string[]).sort()).toEqual(['dynamodb:DeleteItem', 'dynamodb:PutItem', 'dynamodb:UpdateItem']);
    expect(bySid('DemoWriteItems').Condition).toEqual({ 'ForAllValues:StringLike': { 'dynamodb:LeadingKeys': ['DEMO#*'] } });
    expect(bySid('DemoReadBm25Indexes').Action).toBe('s3:GetObject');
    expect(JSON.stringify(bySid('DemoReadBm25Indexes').Resource)).toContain('/courses/*/bm25/*');
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

describe('the execution role policy', () => {
  test('stays under the IAM limit for an inline policy of 10,240 characters, whitespace excluded', () => {
    const values: Values = { ...exampleValues(), ACCOUNT_ID: fakeAccount };
    const text = renderIamDocument(readTemplate('cloudformation-execution-role-policy'), values);
    const compact = JSON.stringify(JSON.parse(text));
    expect(compact.length, `the rendered policy has ${compact.length} characters`).toBeLessThan(10_240);
  });
});
