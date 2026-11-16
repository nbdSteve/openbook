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

const globalModels: Model[] = [
  model({ id: 'g-sol', label: 'G Sol', vendor: 'openai', modelId: 'global.openai.gpt-6.1-sol', residency: 'global' }),
  model({ id: 'g-terra', label: 'G Terra', vendor: 'openai', modelId: 'global.openai.gpt-6.1-terra', residency: 'global' }),
];

/** The Bedrock resources of a template, as plain strings with the partition filled in. */
function bedrockOf(s: ReturnType<typeof synth>): { statements: Json[]; resources: string[] } {
  const statements = policyStatements(s.json).filter((st) =>
    asArray(st.Action as string | string[]).some((a) => a.startsWith('bedrock:')),
  );
  const resources = statements.flatMap((st) =>
    asArray(st.Resource as string | unknown[]).map((r) => {
      const parts = ((r as { 'Fn::Join'?: [string, unknown[]] })['Fn::Join'] ?? ['', [r]])[1];
      return parts.map((p) => (typeof p === 'string' ? p : 'aws')).join('');
    }),
  );
  return { statements, resources };
}

const profile = (id: string): string => `arn:aws:bedrock:ap-southeast-2:111111111111:inference-profile/${id}`;
const fm = (region: string, id: string): string => `arn:aws:bedrock:${region}::foundation-model/${id}`;

/** What the catalog rules say the policy must contain, written out independently of the code. */
function expectedResources(models: Model[]): string[] {
  const out: string[] = [];
  for (const m of models) {
    const id = m.modelId as string;
    if (id.startsWith('au.')) {
      const bare = id.slice(3);
      out.push(profile(id), fm('ap-southeast-2', bare), fm('ap-southeast-4', bare));
    } else if (id.startsWith('global.')) {
      const bare = id.slice(7);
      out.push(profile(id), fm('ap-southeast-2', bare), fm('', bare));
    } else {
      out.push(fm('ap-southeast-2', id));
    }
  }
  return [...new Set(out)].sort();
}

describe('the Bedrock policy of the deployed catalog', () => {
  const { statements, resources } = bedrockOf(synth());

  test('allows only InvokeModel and InvokeModelWithResponseStream, with Allow', () => {
    expect(statements.length).toBeGreaterThan(0);
    for (const st of statements) {
      expect(st.Effect).toBe('Allow');
      expect(asArray(st.Action as string[]).sort()).toEqual([
        'bedrock:InvokeModel',
        'bedrock:InvokeModelWithResponseStream',
      ]);
    }
  });

  test('the resources are exactly what the catalog derives', () => {
    expect([...resources].sort()).toEqual(expectedResources(deployed));
  });

  test('has no wildcard resource, no other profile prefix, and no Region but Sydney and Melbourne', () => {
    const text = resources.join('\n');
    expect(text).not.toContain('*');
    for (const prefix of ['us.', 'eu.', 'apac.', 'jp.', 'global.']) {
      expect(text).not.toContain(`inference-profile/${prefix}`);
    }
    const regions = [...text.matchAll(/:bedrock:([a-z0-9-]*):/g)].map((m) => m[1]);
    expect(new Set(regions)).toEqual(new Set(['ap-southeast-2', 'ap-southeast-4']));
  });
});

describe('the Bedrock policy of a catalog with global. models', () => {
  const models = [model({ id: 'm' }), ...globalModels];
  const s = synth({ models, defaultChatModel: 'm', fastModel: 'm' });
  const { statements, resources } = bedrockOf(s);

  test('the resources are exactly what the catalog derives', () => {
    expect([...resources].sort()).toEqual(expectedResources(models));
  });

  test('allows only the two invoke actions', () => {
    for (const st of statements) {
      expect(asArray(st.Action as string[]).sort()).toEqual([
        'bedrock:InvokeModel',
        'bedrock:InvokeModelWithResponseStream',
      ]);
    }
  });

  test('global models are reachable only through their own profile, as the Bedrock guide requires', () => {
    for (const n of ['gpt-6.1-sol', 'gpt-6.1-terra']) {
      const routed = statements.find((st) =>
        asArray(st.Resource as string).some((r) => JSON.stringify(r).includes(`:bedrock:::foundation-model/openai.${n}`)),
      );
      expect(routed?.Condition).toEqual({
        StringEquals: {
          'aws:RequestedRegion': 'unspecified',
          'bedrock:InferenceProfileArn': expect.anything(),
        },
      });
      const inRegion = statements.find((st) =>
        asArray(st.Resource as string).some((r) =>
          JSON.stringify(r).includes(`:bedrock:ap-southeast-2::foundation-model/openai.${n}`),
        ),
      );
      expect(inRegion?.Condition).toEqual({
        StringEquals: {
          'aws:RequestedRegion': 'ap-southeast-2',
          'bedrock:InferenceProfileArn': expect.anything(),
        },
      });
    }
    const profiles = statements.find((st) => JSON.stringify(st.Resource).includes('inference-profile/global.'));
    expect(profiles?.Condition).toEqual({ StringEquals: { 'aws:RequestedRegion': 'ap-southeast-2' } });
  });

  test('the environment carries the same catalog', () => {
    const fn = Object.values(s.json.Resources).find(
      (r) => r.Type === 'AWS::Lambda::Function' && r.Properties?.Handler === 'bootstrap',
    );
    const env = (fn?.Properties?.Environment as { Variables: Record<string, string> }).Variables;
    expect(JSON.parse(env.OPENBOOK_MODELS) as unknown).toEqual(models);
  });

  test('a global default or fast model is still refused', () => {
    expect(() => synth({ models, defaultChatModel: 'g-sol', fastModel: 'm' })).toThrow(/must have residency "au"/);
  });
});

describe('catalog changes', () => {
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
