import { describe, expect, test } from 'vitest';
import { parseModelId, validateCatalog } from '../lib/catalog';
import { asArray, cdkJsonContext, policyStatements, synth, type Json } from './helpers';

type Model = Record<string, unknown>;
const deployed = cdkJsonContext.models as Model[];
const model = (over: Model): Model => ({
  id: 'm',
  label: 'M',
  vendor: 'anthropic',
  provider: 'bedrock',
  modelId: 'au.anthropic.claude-sonnet-5-5',
  residency: 'au',
  maxOutputTokens: 2000,
  ...over,
});
const withCatalog = (models: Model[], defaultChatModel = 'm', fastModel = 'm') =>
  () => synth({ models, defaultChatModel, fastModel });

describe('catalog validation fails the synth', () => {
  test('accepts the deployed catalog', () => {
    expect(() => synth()).not.toThrow();
  });

  test('accepts the catalog as a JSON string, as a --context flag gives it', () => {
    expect(() => synth({ models: JSON.stringify(deployed) })).not.toThrow();
  });

  test.each([
    ['a residency that does not match the prefix', model({ modelId: 'global.anthropic.x', residency: 'au' }), /residency "au"/],
    ['a global residency on an au. profile', model({ residency: 'global' }), /residency "global"/],
    ['a global residency on an unprefixed model', model({ modelId: 'openai.gpt-oss-120b-1:0', residency: 'global' }), /residency "global"/],
    ['the us. prefix', model({ modelId: 'us.anthropic.claude-sonnet-5-5' }), /"us\." prefix/],
    ['the apac. prefix', model({ modelId: 'apac.anthropic.claude-sonnet-5-5' }), /"apac\." prefix/],
    ['the eu. prefix', model({ modelId: 'eu.anthropic.claude-sonnet-5-5' }), /"eu\." prefix/],
    ['the fake provider', model({ provider: 'fake' }), /fake provider is local only/],
    ['an unknown provider', model({ provider: 'openai' }), /allows only "bedrock"/],
    ['a bad id', model({ id: 'Bad_ID' }), /must match/],
    ['an unknown residency', model({ residency: 'eu' }), /residency must be/],
    ['a model ID that is not a Bedrock ID', model({ modelId: 'sonnet' }), /not a Bedrock/],
    ['a negative token limit', model({ maxOutputTokens: -1 }), /positive integer/],
  ])('refuses %s', (_name, bad, message) => {
    expect(withCatalog([bad])).toThrow(message);
  });

  test('refuses duplicate IDs', () => {
    expect(withCatalog([model({}), model({})])).toThrow(/duplicate id "m"/);
  });

  test('refuses a default chat model that is global', () => {
    const global = model({ id: 'g', modelId: 'global.openai.gpt-6.1-sol', residency: 'global' });
    expect(withCatalog([model({}), global], 'g', 'm')).toThrow(/defaultChatModel "g" must have residency "au"/);
  });

  test('refuses a fast model that is global', () => {
    const global = model({ id: 'g', modelId: 'global.openai.gpt-6.1-sol', residency: 'global' });
    expect(withCatalog([model({}), global], 'm', 'g')).toThrow(/fastModel "g" must have residency "au"/);
  });

  test('refuses a default or fast model that is not in the catalog', () => {
    expect(withCatalog([model({})], 'missing', 'm')).toThrow(/not in the catalog/);
    expect(withCatalog([model({})], 'm', 'missing')).toThrow(/not in the catalog/);
  });

  test('refuses a missing or empty catalog', () => {
    expect(() => validateCatalog(undefined, 'a', 'b')).toThrow(/non-empty array/);
    expect(() => validateCatalog([], 'a', 'b')).toThrow(/non-empty array/);
    expect(() => validateCatalog('{not json', 'a', 'b')).toThrow(/valid JSON/);
  });
});

describe('parseModelId', () => {
  test.each([
    ['au.anthropic.claude-sonnet-5-5', 'au', 'anthropic.claude-sonnet-5-5'],
    ['global.openai.gpt-6.1-sol', 'global', 'openai.gpt-6.1-sol'],
    ['openai.gpt-oss-120b-1:0', 'none', 'openai.gpt-oss-120b-1:0'],
  ])('splits %s', (id, prefix, fm) => {
    expect(parseModelId(id)).toEqual({ prefix, foundationModel: fm });
  });
});

describe('the Bedrock policy of the deployed catalog', () => {
  const s = synth();
  const bedrock = policyStatements(s.json).filter((st) =>
    asArray(st.Action as string | string[]).some((a) => a.startsWith('bedrock:')),
  );
  const resources = (st: Json): string[] => asArray(st.Resource as string | unknown[]).map((r) => JSON.stringify(r));

  test('allows only InvokeModel and InvokeModelWithResponseStream, with Allow', () => {
    expect(bedrock.length).toBeGreaterThan(0);
    for (const st of bedrock) {
      expect(st.Effect).toBe('Allow');
      expect(asArray(st.Action as string[]).sort()).toEqual([
        'bedrock:InvokeModel',
        'bedrock:InvokeModelWithResponseStream',
      ]);
    }
  });

  test('has no wildcard resource and no other Region or profile prefix', () => {
    const text = bedrock.flatMap(resources).join('\n');
    expect(text).not.toContain('"*"');
    expect(text).not.toContain('foundation-model/*');
    expect(text).not.toContain('foundation-model/anthropic.*');
    for (const prefix of ['us.', 'eu.', 'apac.', 'jp.']) expect(text).not.toContain(`inference-profile/${prefix}`);
    const regions = [...text.matchAll(/:bedrock:([a-z0-9-]*):/g)].map((m) => m[1]);
    expect(new Set(regions)).toEqual(new Set(['ap-southeast-2', 'ap-southeast-4', '']));
  });

  test('the resources are exactly what the catalog derives', () => {
    const profile = (id: string): string =>
      `arn:aws:bedrock:ap-southeast-2:111111111111:inference-profile/${id}`;
    const fm = (region: string, id: string): string => `arn:aws:bedrock:${region}::foundation-model/${id}`;
    const expected = [
      // au. profiles: the profile and its foundation model in Sydney and Melbourne.
      ...['claude-sonnet-5-5', 'claude-opus-5-5', 'claude-haiku-5-5'].flatMap((n) => [
        profile(`au.anthropic.${n}`),
        fm('ap-southeast-2', `anthropic.${n}`),
        fm('ap-southeast-4', `anthropic.${n}`),
      ]),
      // An unprefixed on-demand model: Sydney only.
      fm('ap-southeast-2', 'openai.gpt-oss-120b-1:0'),
      // global. profiles: the profile, the model in Sydney, and the Region-less model.
      ...['gpt-6.1-sol', 'gpt-6.1-terra'].flatMap((n) => [
        profile(`global.openai.${n}`),
        fm('ap-southeast-2', `openai.${n}`),
        fm('', `openai.${n}`),
      ]),
    ].sort();
    const actual = bedrock
      .flatMap(resources)
      .map((r) => JSON.parse(r) as unknown)
      .map((r) => {
        // Flatten an Fn::Join of literal parts and AWS::Partition into a string.
        const parts = ((r as { 'Fn::Join': [string, unknown[]] })['Fn::Join'] ?? ['', [r]])[1];
        return parts.map((p) => (typeof p === 'string' ? p : 'aws')).join('');
      })
      .sort();
    expect(actual).toEqual(expected);
  });

  test('global models are reachable only through their own profile, as the Bedrock guide requires', () => {
    for (const n of ['gpt-6.1-sol', 'gpt-6.1-terra']) {
      const routed = bedrock.find((st) => resources(st).join('').includes(`:bedrock:::foundation-model/openai.${n}`));
      expect(routed?.Condition).toEqual({
        StringEquals: {
          'aws:RequestedRegion': 'unspecified',
          'bedrock:InferenceProfileArn': expect.anything(),
        },
      });
      const inRegion = bedrock.find((st) =>
        resources(st).join('').includes(`:bedrock:ap-southeast-2::foundation-model/openai.${n}`),
      );
      expect(inRegion?.Condition).toEqual({
        StringEquals: {
          'aws:RequestedRegion': 'ap-southeast-2',
          'bedrock:InferenceProfileArn': expect.anything(),
        },
      });
    }
    const profiles = bedrock.find((st) => resources(st).join('').includes('inference-profile/global.'));
    expect(profiles?.Condition).toEqual({ StringEquals: { 'aws:RequestedRegion': 'ap-southeast-2' } });
  });

  test('a catalog change updates the environment and the policy together', () => {
    const changed = synth({
      models: [model({ id: 'm', modelId: 'au.anthropic.claude-sonnet-9-9' })],
      defaultChatModel: 'm',
      fastModel: 'm',
    });
    const text = JSON.stringify(changed.json);
    expect(text).toContain('inference-profile/au.anthropic.claude-sonnet-9-9');
    expect(text).toContain('foundation-model/anthropic.claude-sonnet-9-9');
    expect(text).not.toContain('claude-sonnet-5-5');
    expect(text).not.toContain('openai');
  });

  test('a catalog with no global model has no Region-less ARN', () => {
    const changed = synth({ models: [model({})], defaultChatModel: 'm', fastModel: 'm' });
    expect(JSON.stringify(changed.json)).not.toContain(':bedrock:::');
  });
});
