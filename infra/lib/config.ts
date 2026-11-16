import * as path from 'path';
import type { Node } from 'constructs';

export type Stage = 'dev' | 'prod';

export const region = 'ap-southeast-2';

/** The infra directory. Relative asset paths in context resolve against it. */
export const infraRoot = path.resolve(__dirname, '..');

export interface Limits {
  dailyMessagesPerUser: number;
  dailyMessagesGlobal: number;
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
  chatModelId: string;
  fastModelId: string;
  limits: Limits;
  domain?: DomainConfig;
}

/** Defaults from docs/architecture.md (API configuration table). */
export const defaultChatModelId = 'au.anthropic.claude-sonnet-4-6';
export const defaultFastModelId = 'au.anthropic.claude-haiku-4-5-20251001-v1:0';

export const defaultLimits: Limits = {
  dailyMessagesPerUser: 50,
  dailyMessagesGlobal: 500,
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

function positiveInt(node: Node, key: string, fallback: number): number {
  const value: unknown = node.tryGetContext(key);
  if (value === undefined || value === null || value === '') return fallback;
  const n = typeof value === 'number' ? value : Number(value);
  if (!Number.isInteger(n) || n <= 0) throw new Error(`context ${key} must be a positive integer`);
  return n;
}

/** Bedrock calls use `au.` inference profiles only (docs/architecture.md, hard rule 4). */
export function assertAuProfile(key: string, modelId: string): void {
  if (!/^au\.anthropic\.[A-Za-z0-9.:-]+$/.test(modelId)) {
    throw new Error(`context ${key} must be an au.anthropic.* inference profile ID, got "${modelId}"`);
  }
}

export function resolveConfig(node: Node): OpenbookConfig {
  const stage = str(node, 'stage') ?? 'dev';
  if (stage !== 'dev' && stage !== 'prod') {
    throw new Error(`context stage must be "dev" or "prod", got "${stage}"`);
  }

  const chatModelId = str(node, 'chatModelId') ?? defaultChatModelId;
  const fastModelId = str(node, 'fastModelId') ?? defaultFastModelId;
  assertAuProfile('chatModelId', chatModelId);
  assertAuProfile('fastModelId', fastModelId);

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
    chatModelId,
    fastModelId,
    limits: {
      dailyMessagesPerUser: positiveInt(node, 'limitDailyMessagesPerUser', defaultLimits.dailyMessagesPerUser),
      dailyMessagesGlobal: positiveInt(node, 'limitDailyMessagesGlobal', defaultLimits.dailyMessagesGlobal),
      coursesOwned: positiveInt(node, 'limitCoursesOwned', defaultLimits.coursesOwned),
      documentsPerCourse: positiveInt(node, 'limitDocumentsPerCourse', defaultLimits.documentsPerCourse),
      uploadBytes: positiveInt(node, 'limitUploadBytes', defaultLimits.uploadBytes),
    },
    ...(domain ? { domain } : {}),
  };
}
