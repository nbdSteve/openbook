import * as fs from 'fs';
import * as path from 'path';
import type { Node } from 'constructs';
import { assertEmbeddingModel, assertTranscribeModel, validateCatalog, type Catalog } from './catalog';

export type Stage = 'dev' | 'prod';

export const region = 'ap-southeast-2';

/** The infra directory. Relative asset paths in context resolve against it. */
export const infraRoot = path.resolve(__dirname, '..');

export interface Limits {
  dailyMessagesPerUser: number;
  dailyMessagesGlobal: number;
  dailyTokensGlobal: number;
  failedJoinsPerDay: number;
  coursesOwned: number;
  documentsPerCourse: number;
  uploadBytes: number;
  pagesPerDocument: number;
  transcribedPagesPerUser: number;
  transcribedPagesGlobal: number;
}

/**
 * A custom domain. Both names are public and static. The certificate ARN and the hosted zone ID
 * are CloudFormation parameters that the deployment supplies, so the template has no resource ID.
 */
export interface DomainConfig {
  domainName: string;
  hostedZoneName: string;
}

/** One credit line of the demo, as the public web config shows it. */
export interface DemoSource {
  title: string;
  license: string;
  url: string;
}

/**
 * The public demo (docs/api.md, "Demo"). The sources and the example questions come from
 * `demo/course.json`, the same manifest that `make demo-provision` loads, so the landing page
 * and the demo course cannot disagree.
 */
export interface DemoConfig {
  /** The kill switch. Off means the API returns 404 and the landing page hides the demo. */
  enabled: boolean;
  visitorPer10Min: number;
  visitorPerDay: number;
  /** What the whole demo may spend in a day (UTC), in US dollars. */
  budgetUsdPerDay: number;
  /** The label of the default chat model, which answers the demo. */
  modelLabel: string;
  sources: DemoSource[];
  examples: string[];
}

export interface OpenbookConfig {
  stage: Stage;
  apiAssetDir: string;
  ingestAssetDir: string;
  webAssetDir: string;
  catalog: Catalog;
  /** The catalog ID of the model that transcribes pages. */
  transcribeModel: string;
  embeddingModelId: string;
  limits: Limits;
  demo: DemoConfig;
  domain?: DomainConfig;
}

/** Defaults from docs/architecture.md (API configuration table). */
export const defaultLimits: Limits = {
  dailyMessagesPerUser: 50,
  dailyMessagesGlobal: 500,
  dailyTokensGlobal: 3_000_000,
  failedJoinsPerDay: 20,
  coursesOwned: 10,
  documentsPerCourse: 50,
  uploadBytes: 26_214_400,
  pagesPerDocument: 500,
  transcribedPagesPerUser: 300,
  transcribedPagesGlobal: 1000,
};

export const defaultEmbeddingModelId = 'amazon.titan-embed-text-v2:0';

function str(node: Node, key: string): string | undefined {
  const value: unknown = node.tryGetContext(key);
  if (value === undefined || value === null || value === '') return undefined;
  if (typeof value !== 'string') throw new Error(`context ${key} must be a string`);
  return value;
}

/**
 * A limit comes from, in order: its own context key (`-c limitDailyMessagesGlobal=10`), the
 * stage defaults in cdk.json (`stageLimits.<stage>.<key>`), and the API default.
 */
function limit(node: Node, stage: Stage, key: string, fallback: number): number {
  const stageLimits = node.tryGetContext('stageLimits') as Record<string, Record<string, unknown>> | undefined;
  const value: unknown = node.tryGetContext(key) ?? stageLimits?.[stage]?.[key];
  if (value === undefined || value === null || value === '') return fallback;
  const n = typeof value === 'number' ? value : Number(value);
  if (!Number.isInteger(n) || n <= 0) throw new Error(`context ${key} must be a positive integer`);
  return n;
}

/**
 * The demo limits when the context says nothing: 10 questions per visitor per 10 minutes, 40 a day,
 * and 2 US dollars a day for the whole demo (about 100 answers of the default chat model).
 */
export const defaultDemoLimits = { visitorPer10Min: 10, visitorPerDay: 40, budgetUsdPerDay: 2 };

/**
 * The daily budget comes from, in order: the context key `demoBudgetUsdPerDay`, the stage defaults in
 * cdk.json (`stageDemo.<stage>.budgetUsdPerDay`), and the default. It is a positive number of dollars.
 */
function demoBudget(node: Node, stage: Stage): number {
  const stageDemo = node.tryGetContext('stageDemo') as Record<string, Record<string, unknown>> | undefined;
  const value: unknown = node.tryGetContext('demoBudgetUsdPerDay') ?? stageDemo?.[stage]?.budgetUsdPerDay;
  if (value === undefined || value === null || value === '') return defaultDemoLimits.budgetUsdPerDay;
  const n = typeof value === 'number' ? value : Number(value);
  if (!Number.isFinite(n) || n <= 0 || n > 100_000) throw new Error('context demoBudgetUsdPerDay must be a positive number of US dollars');
  return n;
}

/**
 * The kill switch comes from, in order: the context key `demoEnabled` (`-c demoEnabled=true`), the
 * stage defaults in cdk.json (`stageDemo.<stage>.enabled`), and off.
 */
function demoEnabled(node: Node, stage: Stage): boolean {
  const stageDemo = node.tryGetContext('stageDemo') as Record<string, Record<string, unknown>> | undefined;
  const value: unknown = node.tryGetContext('demoEnabled') ?? stageDemo?.[stage]?.enabled;
  if (value === undefined || value === null || value === '') return false;
  if (value === true || value === 'true') return true;
  if (value === false || value === 'false') return false;
  throw new Error('context demoEnabled must be true or false');
}

/** Reads the credits and the example questions from the demo manifest. */
function demoManifest(node: Node): { sources: DemoSource[]; examples: string[] } {
  const file = path.resolve(infraRoot, str(node, 'demoManifest') ?? '../demo/course.json');
  let manifest: { sources?: { attribution?: Record<string, unknown> }[]; examples?: unknown[] };
  try {
    manifest = JSON.parse(fs.readFileSync(file, 'utf8')) as typeof manifest;
  } catch (error) {
    throw new Error(`cannot read the demo manifest ${file}`, { cause: error });
  }
  const sources = (manifest.sources ?? []).map((source) => {
    const a = source.attribution ?? {};
    const { title, license, sourceUrl } = a;
    if (typeof title !== 'string' || typeof license !== 'string' || typeof sourceUrl !== 'string' || !sourceUrl.startsWith('https://')) {
      throw new Error('every source of the demo manifest needs a title, a license, and an https sourceUrl');
    }
    return { title, license, url: sourceUrl };
  });
  const examples = manifest.examples ?? [];
  if (sources.length === 0) throw new Error('the demo manifest needs at least one source');
  if (examples.length < 4 || examples.length > 6 || examples.some((e) => typeof e !== 'string' || e.trim() === '')) {
    throw new Error('the demo manifest needs 4 to 6 example questions');
  }
  return { sources, examples: examples as string[] };
}

const dnsName = /^(?=.{1,253}$)([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,63}$/;

/**
 * The domain comes from the context keys `domainName` and `hostedZoneName`, or else from
 * `stageDomains.<stage>` in cdk.json. A stage with no domain has none. Half a domain is an error.
 */
function resolveDomain(node: Node, stage: Stage): DomainConfig | undefined {
  const stageDomains = node.tryGetContext('stageDomains') as Record<string, Record<string, unknown>> | undefined;
  const fromStage = stageDomains?.[stage];
  const pick = (key: string): string | undefined => {
    const direct = str(node, key);
    if (direct !== undefined) return direct;
    const value = fromStage?.[key];
    if (value === undefined || value === null || value === '') return undefined;
    if (typeof value !== 'string') throw new Error(`context stageDomains.${stage}.${key} must be a string`);
    return value;
  };
  const domainName = pick('domainName');
  const hostedZoneName = pick('hostedZoneName');
  if (domainName === undefined && hostedZoneName === undefined) return undefined;
  if (domainName === undefined || hostedZoneName === undefined) {
    throw new Error('a custom domain needs the context keys domainName and hostedZoneName together');
  }
  for (const [key, value] of [['domainName', domainName], ['hostedZoneName', hostedZoneName]]) {
    if (!dnsName.test(value)) throw new Error(`context ${key} must be a lower case DNS name`);
  }
  if (domainName !== hostedZoneName && !domainName.endsWith(`.${hostedZoneName}`)) {
    throw new Error('domainName must be the hosted zone name or a subdomain of it');
  }
  return { domainName, hostedZoneName };
}

export function resolveConfig(node: Node): OpenbookConfig {
  const stage = str(node, 'stage') ?? 'dev';
  if (stage !== 'dev' && stage !== 'prod') {
    throw new Error(`context stage must be "dev" or "prod", got "${stage}"`);
  }

  // The deployed catalog is in cdk.json context. It has no default in code.
  const catalog = validateCatalog(
    node.tryGetContext('models'),
    node.tryGetContext('defaultChatModel'),
    node.tryGetContext('fastModel'),
  );

  // The transcription model defaults to the fast model. It must accept images, and be `au`.
  const transcribeModel = str(node, 'transcribeModel') ?? catalog.fastModel;
  assertTranscribeModel(catalog, transcribeModel);
  const embeddingModelId = assertEmbeddingModel(str(node, 'embeddingModelId') ?? defaultEmbeddingModelId);

  const domain = resolveDomain(node, stage);

  return {
    stage,
    apiAssetDir: path.resolve(infraRoot, str(node, 'apiAssetDir') ?? '../target/lambda/openbook-api'),
    ingestAssetDir: path.resolve(infraRoot, str(node, 'ingestAssetDir') ?? '../target/lambda/openbook-ingest'),
    webAssetDir: path.resolve(infraRoot, str(node, 'webAssetDir') ?? '../apps/ask/dist'),
    catalog,
    transcribeModel,
    embeddingModelId,
    limits: {
      dailyMessagesPerUser: limit(node, stage, 'limitDailyMessagesPerUser', defaultLimits.dailyMessagesPerUser),
      dailyMessagesGlobal: limit(node, stage, 'limitDailyMessagesGlobal', defaultLimits.dailyMessagesGlobal),
      dailyTokensGlobal: limit(node, stage, 'limitDailyTokensGlobal', defaultLimits.dailyTokensGlobal),
      failedJoinsPerDay: limit(node, stage, 'limitFailedJoinsPerDay', defaultLimits.failedJoinsPerDay),
      coursesOwned: limit(node, stage, 'limitCoursesOwned', defaultLimits.coursesOwned),
      documentsPerCourse: limit(node, stage, 'limitDocumentsPerCourse', defaultLimits.documentsPerCourse),
      uploadBytes: limit(node, stage, 'limitUploadBytes', defaultLimits.uploadBytes),
      pagesPerDocument: limit(node, stage, 'limitPagesPerDocument', defaultLimits.pagesPerDocument),
      transcribedPagesPerUser: limit(node, stage, 'limitDailyTranscribedPagesPerUser', defaultLimits.transcribedPagesPerUser),
      transcribedPagesGlobal: limit(node, stage, 'limitDailyTranscribedPagesGlobal', defaultLimits.transcribedPagesGlobal),
    },
    demo: {
      enabled: demoEnabled(node, stage),
      visitorPer10Min: limit(node, stage, 'limitDemoVisitorPer10Min', defaultDemoLimits.visitorPer10Min),
      visitorPerDay: limit(node, stage, 'limitDemoVisitorPerDay', defaultDemoLimits.visitorPerDay),
      budgetUsdPerDay: demoBudget(node, stage),
      modelLabel: catalog.models.find((m) => m.id === catalog.defaultChatModel)?.label ?? catalog.defaultChatModel,
      ...demoManifest(node),
    },
    ...(domain ? { domain } : {}),
  };
}
