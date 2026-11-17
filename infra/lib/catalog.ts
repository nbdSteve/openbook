import * as iam from 'aws-cdk-lib/aws-iam';

/**
 * The deployed model catalog (docs/architecture.md, "Model catalog").
 *
 * The catalog lives in `cdk.json` context (`models`, `defaultChatModel`, `fastModel`).
 * This module validates it, and derives the Bedrock IAM permissions from it, so the
 * Lambda environment and its policy cannot disagree.
 */

export type Residency = 'au' | 'global';

export interface CatalogModel {
  id: string;
  label: string;
  vendor: string;
  provider: 'bedrock';
  modelId: string;
  residency: Residency;
  maxOutputTokens?: number;
  temperature?: number;
  /** True when the model accepts images. The transcription model must have it. */
  vision?: boolean;
  /** List price in US dollars per million input tokens (the demo budget and billing use it). */
  inputUsdPerMillion: number;
  /** List price in US dollars per million output tokens. */
  outputUsdPerMillion: number;
}

export interface Catalog {
  models: CatalogModel[];
  defaultChatModel: string;
  fastModel: string;
}

/** The Region the API runs in. `au.` profiles route to Sydney and Melbourne from it. */
export const sourceRegion = 'ap-southeast-2';
export const auDestinationRegions = ['ap-southeast-2', 'ap-southeast-4'] as const;

/**
 * The model ID allowlist of docs/architecture.md ("Model catalog"). The Rust API uses the same
 * rule and the same vendor list. A model ID has exactly one of three shapes, and anything
 * else is refused:
 *
 * - `au.<vendor>.<model>`: an Australian inference profile (residency `au`)
 * - `<vendor>.<model>`: an on-demand model in Sydney (residency `au`)
 * - `global.<vendor>.<model>`: a global inference profile (residency `global`)
 */
export const allowedVendors = ['anthropic', 'openai', 'amazon', 'meta', 'mistral', 'cohere', 'deepseek', 'qwen'] as const;

/** The largest `maxOutputTokens` that the API accepts (MAX_OUTPUT_TOKENS_LIMIT in catalog.rs). */
export const maxOutputTokensLimit = 64_000;

/** The largest price per million tokens (MAX_USD_PER_MILLION in catalog.rs). It catches a typo. */
export const maxUsdPerMillion = 1000;

const idPattern = /^[a-z0-9-]{1,40}$/;
const modelIdPattern = new RegExp(`^(?:(au|global)\\.)?(?:${allowedVendors.join('|')})\\.[A-Za-z0-9][A-Za-z0-9._:-]*$`);

export interface ParsedModelId {
  /** `au`, `global`, or `none` for an on-demand model in Sydney. */
  prefix: 'au' | 'global' | 'none';
  /** The foundation model ID without any profile prefix, such as `anthropic.claude-sonnet-5-5`. */
  foundationModel: string;
}

/** Splits a catalog `modelId`, and refuses every shape that is not on the allowlist. */
export function parseModelId(modelId: string): ParsedModelId {
  const match = modelIdPattern.exec(modelId);
  if (!match) {
    throw new Error(
      `model ID "${modelId}" is not allowed: use au.<vendor>.<model>, <vendor>.<model>, or global.<vendor>.<model>, ` +
        `with a vendor of ${allowedVendors.join(', ')}`,
    );
  }
  const prefix = (match[1] ?? 'none') as ParsedModelId['prefix'];
  return { prefix, foundationModel: prefix === 'none' ? modelId : modelId.slice(prefix.length + 1) };
}

function fail(message: string): never {
  throw new Error(`invalid model catalog: ${message}`);
}

/** Validates the catalog context. Throws on the first rule that it breaks. */
export function validateCatalog(raw: unknown, defaultChatModel: unknown, fastModel: unknown): Catalog {
  let models = raw;
  if (typeof models === 'string') {
    try {
      models = JSON.parse(models);
    } catch {
      fail('context "models" is not valid JSON');
    }
  }
  if (!Array.isArray(models) || models.length === 0) fail('context "models" must be a non-empty array');
  if (typeof defaultChatModel !== 'string' || typeof fastModel !== 'string') {
    fail('context "defaultChatModel" and "fastModel" must be catalog IDs');
  }

  const seen = new Set<string>();
  const out: CatalogModel[] = [];
  for (const entry of models as Record<string, unknown>[]) {
    if (typeof entry !== 'object' || entry === null) fail('every entry must be an object');
    const { id, label, vendor, provider, modelId, residency, maxOutputTokens, temperature, vision } = entry;
    const { inputUsdPerMillion, outputUsdPerMillion } = entry;
    if (typeof id !== 'string' || !idPattern.test(id)) fail(`id "${String(id)}" must match [a-z0-9-]{1,40}`);
    if (seen.has(id)) fail(`duplicate id "${id}"`);
    seen.add(id);
    if (typeof label !== 'string' || label === '') fail(`"${id}" needs a label`);
    if (typeof vendor !== 'string' || vendor === '') fail(`"${id}" needs a vendor`);
    if (provider !== 'bedrock') {
      fail(`"${id}" has provider "${String(provider)}"; a deployed catalog allows only "bedrock" (the fake provider is local only)`);
    }
    if (typeof modelId !== 'string') fail(`"${id}" needs a modelId`);
    if (residency !== 'au' && residency !== 'global') fail(`"${id}" residency must be "au" or "global"`);
    const parsed = parseModelId(modelId);
    const expected: Residency = parsed.prefix === 'global' ? 'global' : 'au';
    if (residency !== expected) {
      fail(`"${id}" has residency "${residency}" but modelId "${modelId}" is "${expected}"`);
    }
    // The same ranges as crates/core/src/catalog.rs, so a bad value fails the synth and not the
    // start of the Lambda.
    if (
      maxOutputTokens !== undefined &&
      (typeof maxOutputTokens !== 'number' || !Number.isInteger(maxOutputTokens) || maxOutputTokens < 1 || maxOutputTokens > maxOutputTokensLimit)
    ) {
      fail(`"${id}" maxOutputTokens must be an integer from 1 to ${maxOutputTokensLimit}`);
    }
    if (temperature !== undefined && (typeof temperature !== 'number' || !Number.isFinite(temperature) || temperature < 0 || temperature > 2)) {
      fail(`"${id}" temperature must be a number from 0 to 2`);
    }
    if (vision !== undefined && typeof vision !== 'boolean') fail(`"${id}" vision must be true or false`);
    // A deployed model must have prices: the demo budget reserves cost before each call.
    for (const [name, price] of [
      ['inputUsdPerMillion', inputUsdPerMillion],
      ['outputUsdPerMillion', outputUsdPerMillion],
    ] as const) {
      if (typeof price !== 'number' || !Number.isFinite(price) || price < 0 || price > maxUsdPerMillion) {
        fail(`"${id}" ${name} must be a number from 0 to ${maxUsdPerMillion} (US dollars per million tokens)`);
      }
    }
    out.push({
      id,
      label,
      vendor,
      provider,
      modelId,
      residency,
      ...(maxOutputTokens !== undefined ? { maxOutputTokens: maxOutputTokens as number } : {}),
      ...(temperature !== undefined ? { temperature } : {}),
      ...(vision !== undefined ? { vision } : {}),
      inputUsdPerMillion: inputUsdPerMillion as number,
      outputUsdPerMillion: outputUsdPerMillion as number,
    });
  }

  for (const [key, id] of [
    ['defaultChatModel', defaultChatModel],
    ['fastModel', fastModel],
  ] as const) {
    const model = out.find((m) => m.id === id);
    if (!model) fail(`${key} "${id}" is not in the catalog`);
    if (model.residency !== 'au') fail(`${key} "${id}" must have residency "au"`);
  }
  return { models: out, defaultChatModel, fastModel };
}

const pascal = (id: string): string =>
  id
    .split('-')
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join('');

/**
 * The Bedrock permissions the catalog needs: only `InvokeModel` and
 * `InvokeModelWithResponseStream`, on exactly these resources.
 *
 * - `au.` profile: the profile ARN, plus its foundation model in Sydney and Melbourne.
 * - No prefix: the foundation model in Sydney.
 * - `global.` profile: the three-part policy of "Global cross-Region inference" in the
 *   Bedrock guide: the profile, the foundation model in the source Region (only through
 *   that profile), and the Region-less foundation model (only through that profile, with
 *   `aws:RequestedRegion` of `unspecified`).
 */
export function bedrockStatements(models: CatalogModel[], partition: string, account: string): iam.PolicyStatement[] {
  const actions = ['bedrock:InvokeModel', 'bedrock:InvokeModelWithResponseStream'];
  const profileArn = (modelId: string): string =>
    `arn:${partition}:bedrock:${sourceRegion}:${account}:inference-profile/${modelId}`;
  const fmArn = (region: string, fm: string): string => `arn:${partition}:bedrock:${region}::foundation-model/${fm}`;

  const plain: string[] = [];
  const globals: CatalogModel[] = [];
  for (const model of models) {
    const { prefix, foundationModel } = parseModelId(model.modelId);
    if (prefix === 'au') {
      plain.push(profileArn(model.modelId), ...auDestinationRegions.map((r) => fmArn(r, foundationModel)));
    } else if (prefix === 'none') {
      plain.push(fmArn(sourceRegion, foundationModel));
    } else {
      globals.push(model);
    }
  }

  const statements: iam.PolicyStatement[] = [];
  const unique = (list: string[]): string[] => [...new Set(list)];
  if (plain.length > 0) {
    statements.push(
      new iam.PolicyStatement({ sid: 'InvokeAuAndRegionalModels', actions, resources: unique(plain) }),
    );
  }
  if (globals.length > 0) {
    statements.push(
      new iam.PolicyStatement({
        sid: 'InvokeGlobalProfiles',
        actions,
        resources: unique(globals.map((m) => profileArn(m.modelId))),
        conditions: { StringEquals: { 'aws:RequestedRegion': sourceRegion } },
      }),
    );
    for (const model of globals) {
      const { foundationModel } = parseModelId(model.modelId);
      const viaProfile = { 'bedrock:InferenceProfileArn': profileArn(model.modelId) };
      const name = pascal(model.id);
      statements.push(
        new iam.PolicyStatement({
          sid: `InvokeGlobal${name}InSourceRegion`,
          actions,
          resources: [fmArn(sourceRegion, foundationModel)],
          conditions: { StringEquals: { 'aws:RequestedRegion': sourceRegion, ...viaProfile } },
        }),
        new iam.PolicyStatement({
          sid: `InvokeGlobal${name}Routed`,
          actions,
          resources: [fmArn('', foundationModel)],
          conditions: { StringEquals: { 'aws:RequestedRegion': 'unspecified', ...viaProfile } },
        }),
      );
    }
  }
  return statements;
}

/**
 * Every Bedrock resource ARN that the catalog needs, as strings, with `account` in the
 * inference-profile ARNs. This is the same list as `bedrockStatements` (a test compares them).
 * The boundary template takes its Bedrock resources from here, so the boundary allows what
 * the catalog uses and nothing else.
 */
export function bedrockResourceArns(models: CatalogModel[], account: string, partition = 'aws', embeddingModelId?: string): string[] {
  const out: string[] = embeddingModelId ? [embeddingModelArn(embeddingModelId, partition)] : [];
  for (const model of models) {
    const { prefix, foundationModel } = parseModelId(model.modelId);
    const profile = `arn:${partition}:bedrock:${sourceRegion}:${account}:inference-profile/${model.modelId}`;
    const fm = (region: string): string => `arn:${partition}:bedrock:${region}::foundation-model/${foundationModel}`;
    if (prefix === 'au') out.push(profile, ...auDestinationRegions.map(fm));
    else if (prefix === 'none') out.push(fm(sourceRegion));
    else out.push(profile, fm(sourceRegion), fm(''));
  }
  return [...new Set(out)];
}

/**
 * The transcription model renders pages to images, so it must be a catalog entry that accepts
 * images (`vision: true`) and has residency `au` (page images are course material).
 */
export function assertTranscribeModel(catalog: Catalog, id: string): CatalogModel {
  const model = catalog.models.find((m) => m.id === id);
  if (!model) fail(`transcribeModel "${id}" is not in the catalog`);
  if (model.vision !== true) fail(`transcribeModel "${id}" must have vision: true`);
  if (model.residency !== 'au') fail(`transcribeModel "${id}" must have residency "au"`);
  return model;
}

/** The embedding model runs on demand in Sydney: a bare `<vendor>.<model>` ID, no profile prefix. */
export function assertEmbeddingModel(modelId: string): string {
  const { prefix } = parseModelId(modelId);
  if (prefix !== 'none') {
    throw new Error(`embeddingModelId "${modelId}" must be an on-demand model in Sydney (<vendor>.<model>), not a profile`);
  }
  return modelId;
}

/** The foundation-model ARN of the embedding model, which runs in Sydney only. */
export function embeddingModelArn(modelId: string, partition = 'aws'): string {
  return `arn:${partition}:bedrock:${sourceRegion}::foundation-model/${assertEmbeddingModel(modelId)}`;
}
