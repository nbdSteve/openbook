import * as path from 'path';
import type { Node } from 'constructs';
import { validateCatalog, type Catalog } from './catalog';

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
}

export interface DomainConfig {
  domainName: string;
  hostedZoneName: string;
  hostedZoneId: string;
}

export interface OpenbookConfig {
  stage: Stage;
  apiAssetDir: string;
  webAssetDir: string;
  catalog: Catalog;
  limits: Limits;
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
};

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

  const domainName = str(node, 'domainName');
  const hostedZoneName = str(node, 'hostedZoneName');
  const hostedZoneId = str(node, 'hostedZoneId');
  let domain: DomainConfig | undefined;
  if (domainName || hostedZoneName || hostedZoneId) {
    if (!domainName || !hostedZoneName || !hostedZoneId) {
      throw new Error('a custom domain needs the context keys domainName, hostedZoneName, and hostedZoneId together');
    }
    if (domainName !== hostedZoneName && !domainName.endsWith(`.${hostedZoneName}`)) {
      throw new Error('domainName must be the hosted zone name or a subdomain of it');
    }
    domain = { domainName, hostedZoneName, hostedZoneId };
  }

  return {
    stage,
    apiAssetDir: path.resolve(infraRoot, str(node, 'apiAssetDir') ?? '../target/lambda/openbook-api'),
    webAssetDir: path.resolve(infraRoot, str(node, 'webAssetDir') ?? '../apps/ask/dist'),
    catalog,
    limits: {
      dailyMessagesPerUser: limit(node, stage, 'limitDailyMessagesPerUser', defaultLimits.dailyMessagesPerUser),
      dailyMessagesGlobal: limit(node, stage, 'limitDailyMessagesGlobal', defaultLimits.dailyMessagesGlobal),
      dailyTokensGlobal: limit(node, stage, 'limitDailyTokensGlobal', defaultLimits.dailyTokensGlobal),
      failedJoinsPerDay: limit(node, stage, 'limitFailedJoinsPerDay', defaultLimits.failedJoinsPerDay),
      coursesOwned: limit(node, stage, 'limitCoursesOwned', defaultLimits.coursesOwned),
      documentsPerCourse: limit(node, stage, 'limitDocumentsPerCourse', defaultLimits.documentsPerCourse),
      uploadBytes: limit(node, stage, 'limitUploadBytes', defaultLimits.uploadBytes),
    },
    ...(domain ? { domain } : {}),
  };
}
