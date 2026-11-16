import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { Match } from 'aws-cdk-lib/assertions';
import { describe, expect, test } from 'vitest';
import { exampleValues, readTemplate, renderIamDocument } from '../scripts/render-iam';
import { asArray, cdkJsonContext, fixtures, synth, type Json } from './helpers';

type Synth = ReturnType<typeof synth>;
const prod = synth({ stage: 'prod' });
const dev = synth({ stage: 'dev' });
const models = cdkJsonContext.models as Json[];

const resourcesOf = (s: Synth, type: string): [string, { Properties?: Json; DeletionPolicy?: string }][] =>
  Object.entries(s.json.Resources).filter(([, r]) => r.Type === type);

const functionBy = (s: Synth, logicalPrefix: string): Json =>
  resourcesOf(s, 'AWS::Lambda::Function').find(([id]) => id.startsWith(logicalPrefix))?.[1].Properties as Json;

const envOf = (fn: Json): Record<string, string> => (fn.Environment as { Variables: Record<string, string> }).Variables;

/** The statements of one role's inline policy, with ARNs flattened to readable text. */
function roleStatements(s: Synth, rolePrefix: string): Json[] {
  return resourcesOf(s, 'AWS::IAM::Policy')
    .filter(([id]) => id.startsWith(rolePrefix))
    .flatMap(([, r]) => (r.Properties?.PolicyDocument as { Statement: Json[] }).Statement);
}

const text = (value: unknown): string => JSON.stringify(value);

/** The course index ARN pattern. AWS authorizes every index action, CreateIndex included, on it. */
const indexARN = 'arn:aws:s3vectors:ap-southeast-2:111111111111:bucket/openbook-prod-vectors/index/c-*';

/** An ARN value with the partition and the account filled in (the stack account is fake). */
function flat(value: unknown): string {
  if (typeof value === 'string') return value;
  const v = value as { 'Fn::Join'?: [string, unknown[]]; Ref?: string };
  if (v['Fn::Join']) return v['Fn::Join'][1].map(flat).join(v['Fn::Join'][0]);
  if (v.Ref === 'AWS::Partition') return 'aws';
  if (v.Ref === 'AWS::AccountId') return '111111111111';
  return `<${JSON.stringify(value)}>`;
}

describe.each([
  ['dev', dev, 2],
  ['prod', prod, 5],
])('the %s ingest pipeline', (stage, s, concurrency) => {
  test('the queue is a FIFO queue with the settings of docs/retrieval.md', () => {
    s.template.hasResourceProperties('AWS::SQS::Queue', {
      QueueName: `Openbook-${stage}-ingest.fifo`,
      FifoQueue: true,
      ContentBasedDeduplication: false,
      VisibilityTimeout: 960,
      MessageRetentionPeriod: 4 * 24 * 3600,
      SqsManagedSseEnabled: true,
      RedrivePolicy: { maxReceiveCount: 3, deadLetterTargetArn: Match.anyValue() },
    });
  });

  test('the dead-letter queue is a FIFO queue that keeps messages for 14 days', () => {
    s.template.hasResourceProperties('AWS::SQS::Queue', {
      QueueName: `Openbook-${stage}-ingest-dlq.fifo`,
      FifoQueue: true,
      MessageRetentionPeriod: 14 * 24 * 3600,
      SqsManagedSseEnabled: true,
    });
    s.template.resourceCountIs('AWS::SQS::Queue', 2);
  });

  test('both queues refuse plain HTTP', () => {
    const policies = resourcesOf(s, 'AWS::SQS::QueuePolicy');
    expect(policies).toHaveLength(2);
    for (const [, p] of policies) {
      expect(text(p.Properties?.PolicyDocument)).toContain('"aws:SecureTransport":"false"');
    }
  });

  test('the worker is arm64 provided.al2023 with 3008 MB, 900 s, 4096 MB of /tmp, and reserved concurrency', () => {
    const fn = functionBy(s, 'IngestFunction');
    expect(fn).toMatchObject({
      Runtime: 'provided.al2023',
      Architectures: ['arm64'],
      Handler: 'bootstrap',
      MemorySize: 3008,
      Timeout: 900,
      EphemeralStorage: { Size: 4096 },
      ReservedConcurrentExecutions: concurrency,
    });
  });

  test('the worker has its own log group with the stage retention', () => {
    const fn = functionBy(s, 'IngestFunction');
    expect(text(fn.LoggingConfig)).toContain('IngestLogGroup');
    const group = resourcesOf(s, 'AWS::Logs::LogGroup').find(([id]) => id.startsWith('IngestLogGroup'));
    expect(group?.[1].Properties?.RetentionInDays).toBe(stage === 'prod' ? 30 : 7);
    expect(text(functionBy(s, 'ApiFunction').LoggingConfig)).not.toContain('IngestLogGroup');
  });

  test('the event source mapping has batch size 1, partial batch responses, and the reserved concurrency as its cap', () => {
    s.template.hasResourceProperties('AWS::Lambda::EventSourceMapping', {
      BatchSize: 1,
      FunctionResponseTypes: ['ReportBatchItemFailures'],
      ScalingConfig: { MaximumConcurrency: concurrency },
      EventSourceArn: Match.anyValue(),
    });
    s.template.resourceCountIs('AWS::Lambda::EventSourceMapping', 1);
    const mapping = resourcesOf(s, 'AWS::Lambda::EventSourceMapping')[0][1].Properties as Json;
    expect(text(mapping.FunctionName)).toContain('IngestFunction');
    expect(text(mapping.EventSourceArn)).toContain('IngestQueue');
    expect(text(mapping.EventSourceArn)).not.toContain('DeadLetter');
  });

  test('the vector bucket has a fixed stage name and SSE', () => {
    s.template.hasResource('AWS::S3Vectors::VectorBucket', {
      Properties: { VectorBucketName: `openbook-${stage}-vectors`, EncryptionConfiguration: { SseType: 'AES256' } },
      DeletionPolicy: stage === 'prod' ? 'Retain' : 'Delete',
    });
    s.template.resourceCountIs('AWS::S3Vectors::VectorBucket', 1);
  });

  test('both functions get the same environment, with the phase 2 variables', () => {
    const api = envOf(functionBy(s, 'ApiFunction'));
    const ingest = envOf(functionBy(s, 'IngestFunction'));
    expect(ingest).toEqual(api);
    expect(text(api.OPENBOOK_INGEST_QUEUE_URL)).toContain('IngestQueue');
    expect(api.OPENBOOK_VECTOR_BUCKET).toBe(`openbook-${stage}-vectors`);
    expect(api.OPENBOOK_EMBEDDING_MODEL_ID).toBe('amazon.titan-embed-text-v2:0');
    expect(api.OPENBOOK_TRANSCRIBE_MODEL).toBe('claude-haiku');
    expect(api.OPENBOOK_LIMIT_PAGES_PER_DOCUMENT).toBe('500');
    expect(api.OPENBOOK_LIMIT_DAILY_TRANSCRIBED_PAGES_PER_USER).toBe('300');
    // Pre-launch caps from cdk.json, below the API default of 1000.
    expect(api.OPENBOOK_LIMIT_DAILY_TRANSCRIBED_PAGES_GLOBAL).toBe(stage === 'prod' ? '300' : '100');
    expect(JSON.stringify(api).length).toBeLessThan(4096);
    for (const name of Object.keys(api)) expect(name).not.toMatch(/^AWS_ENDPOINT_URL|PDFIUM|BEDROCK_PROFILE|CA_BUNDLE/);
    expect((JSON.parse(api.OPENBOOK_MODELS) as Json[]).map((m) => [m.id, m.vision])).toEqual([
      ['claude-sonnet', true],
      ['claude-haiku', true],
      ['gpt-oss-120b', false],
    ]);
  });
});

describe('the IAM statements of the two functions', () => {
  const api = roleStatements(prod, 'ApiFunctionServiceRoleDefaultPolicy');
  const ingest = roleStatements(prod, 'IngestFunctionServiceRoleDefaultPolicy');
  const bySid = (list: Json[], sid: string): Json => list.find((st) => st.Sid === sid) as Json;
  const actionsOf = (st: Json): string[] => asArray(st.Action as string | string[]).sort();

  test('the API sends messages to the ingest queue, and only that', () => {
    const send = bySid(api, 'SendIngestMessages');
    expect(actionsOf(send)).toEqual(['sqs:SendMessage']);
    expect(text(send.Resource)).toContain('IngestQueue');
    expect(text(send.Resource)).not.toContain('DeadLetter');
    expect(api.flatMap(actionsOf).filter((a) => a.startsWith('sqs:'))).toEqual(['sqs:SendMessage']);
  });

  test('the API deletes and queries course indexes, and nothing else in S3 Vectors', () => {
    const indexes = bySid(api, 'CourseVectorIndexes');
    expect(actionsOf(indexes)).toEqual([
      's3vectors:DeleteIndex',
      's3vectors:GetIndex',
      's3vectors:GetVectors',
      's3vectors:QueryVectors',
    ]);
    expect(flat(indexes.Resource)).toBe(indexARN);
    expect(api.flatMap(actionsOf).filter((a) => a.startsWith('s3vectors:')).sort()).toEqual(actionsOf(indexes));
  });

  test('the worker consumes from the ingest queue through the mapping, and cannot send', () => {
    const consume = ingest.filter((st) => actionsOf(st).some((a) => a.startsWith('sqs:')));
    expect(consume).toHaveLength(1);
    expect(actionsOf(consume[0])).toEqual([
      'sqs:ChangeMessageVisibility',
      'sqs:DeleteMessage',
      'sqs:GetQueueAttributes',
      'sqs:GetQueueUrl',
      'sqs:ReceiveMessage',
    ]);
    expect(text(consume[0].Resource)).toContain('IngestQueue');
  });

  test('the worker creates indexes in the stage bucket, and reads and writes only its indexes', () => {
    const indexes = bySid(ingest, 'CourseVectorIndexes');
    expect(actionsOf(indexes)).toEqual([
      's3vectors:CreateIndex',
      's3vectors:DeleteVectors',
      's3vectors:GetIndex',
      's3vectors:GetVectors',
      's3vectors:PutVectors',
      's3vectors:QueryVectors',
    ]);
    expect(flat(indexes.Resource)).toBe(indexARN);
    // It can delete vectors, but not an index: only the API deletes an index (with the course).
    expect(ingest.flatMap(actionsOf)).not.toContain('s3vectors:DeleteIndex');
  });

  test('the worker uses the table and the docs bucket like the API does', () => {
    expect(bySid(ingest, 'ApplicationTable')).toEqual(bySid(api, 'ApplicationTable'));
    expect(bySid(ingest, 'DocsBucketObjects')).toEqual(bySid(api, 'DocsBucketObjects'));
    expect(bySid(ingest, 'DocsBucketList')).toEqual(bySid(api, 'DocsBucketList'));
  });

  test('the worker invokes only the transcribe model and the embedding model', () => {
    const bedrock = ingest.filter((st) => actionsOf(st).some((a) => a.startsWith('bedrock:')));
    const resources = bedrock.flatMap((st) => asArray(st.Resource as unknown | unknown[]).map(text)).join('\n');
    expect(resources).toContain('inference-profile/au.anthropic.claude-haiku-4-5-20251001-v1:0');
    expect(resources).toContain('foundation-model/anthropic.claude-haiku-4-5-20251001-v1:0');
    expect(resources).toContain('foundation-model/amazon.titan-embed-text-v2:0');
    expect(resources).not.toContain('sonnet');
    expect(resources).not.toContain('gpt-oss');
    expect(resources).not.toContain('"*"');
    const embedding = bySid(ingest, 'InvokeEmbeddingModel');
    expect(actionsOf(embedding)).toEqual(['bedrock:InvokeModel']);
    expect(flat(embedding.Resource)).toBe('arn:aws:bedrock:ap-southeast-2::foundation-model/amazon.titan-embed-text-v2:0');
  });

  test('the API may also embed the question, with the same model', () => {
    expect(flat(bySid(api, 'InvokeEmbeddingModel').Resource)).toBe(
      'arn:aws:bedrock:ap-southeast-2::foundation-model/amazon.titan-embed-text-v2:0',
    );
  });

  test('no statement of either role has a wildcard resource', () => {
    for (const st of [...api, ...ingest]) {
      expect(text(st.Resource), String(st.Sid)).not.toMatch(/"\*"/);
    }
  });

  test('every IAM role of the stack carries the permissions boundary, the worker role too', () => {
    const roles = resourcesOf(prod, 'AWS::IAM::Role');
    expect(roles.map(([id]) => id).some((id) => id.startsWith('IngestFunctionServiceRole'))).toBe(true);
    expect(roles.length).toBeGreaterThanOrEqual(3);
    for (const [id, role] of roles) {
      expect(text(role.Properties?.PermissionsBoundary), id).toContain('OpenbookProdPermissionsBoundary');
    }
  });
});

describe('the transcribe and embedding model rules', () => {
  const entry = (over: Json): Json => ({
    id: 'm',
    label: 'M',
    vendor: 'anthropic',
    provider: 'bedrock',
    modelId: 'au.anthropic.claude-haiku-4-5-20251001-v1:0',
    residency: 'au',
    maxOutputTokens: 2000,
    vision: true,
    ...over,
  });
  const withModels = (list: Json[], extra: Json = {}): (() => Synth) =>
    () => synth({ models: list, defaultChatModel: 'm', fastModel: 'm', ...extra });

  test('the deployed catalog marks the Claude models as vision, and gpt-oss as not', () => {
    expect(models.map((m) => [m.id, m.vision])).toEqual([
      ['claude-sonnet', true],
      ['claude-haiku', true],
      ['gpt-oss-120b', false],
    ]);
  });

  test('the transcribe model defaults to the fast model', () => {
    expect(() => withModels([entry({})])()).not.toThrow();
  });

  test('refuses a transcribe model that is not vision', () => {
    expect(withModels([entry({ vision: false })])).toThrow(/must have vision: true/);
    expect(withModels([entry({ vision: undefined })])).toThrow(/must have vision: true/);
  });

  test('refuses a transcribe model that is not in the catalog', () => {
    expect(withModels([entry({})], { transcribeModel: 'other' })).toThrow(/is not in the catalog/);
  });

  test('refuses a transcribe model with residency global', () => {
    const list = [entry({}), entry({ id: 'g', modelId: 'global.anthropic.claude-sonnet-4-6', residency: 'global' })];
    expect(withModels(list, { transcribeModel: 'g' })).toThrow(/residency "au"/);
  });

  test('accepts a transcribe model that is not the fast model, when it is vision and au', () => {
    const list = [entry({}), entry({ id: 'v', modelId: 'au.anthropic.claude-sonnet-4-6' })];
    const s = withModels(list, { transcribeModel: 'v' })();
    const env = envOf(functionBy(s, 'IngestFunction'));
    expect(env.OPENBOOK_TRANSCRIBE_MODEL).toBe('v');
    expect(text(roleStatements(s, 'IngestFunctionServiceRoleDefaultPolicy'))).toContain('au.anthropic.claude-sonnet-4-6');
    expect(text(roleStatements(s, 'IngestFunctionServiceRoleDefaultPolicy'))).not.toContain('haiku');
  });

  test('refuses a vision flag that is not a boolean', () => {
    expect(withModels([entry({ vision: 'yes' })])).toThrow(/vision must be true or false/);
  });

  test('refuses an embedding model that is a profile or has an unknown vendor', () => {
    const models1 = [entry({})];
    expect(withModels(models1, { embeddingModelId: 'global.amazon.titan-embed-text-v2:0' })).toThrow(/on-demand model in Sydney/);
    expect(withModels(models1, { embeddingModelId: 'au.amazon.titan-embed-text-v2:0' })).toThrow(/on-demand model in Sydney/);
    expect(withModels(models1, { embeddingModelId: 'acme.embed-1' })).toThrow(/is not allowed/);
    expect(withModels(models1, { embeddingModelId: 'amazon.titan-embed-text-v3:0' })).not.toThrow();
  });
});

describe('the worker asset directory', () => {
  const copyIngest = (omit: string): string => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'openbook-ingest-'));
    for (const file of fs.readdirSync(fixtures.ingest)) {
      if (file !== omit) fs.copyFileSync(path.join(fixtures.ingest, file), path.join(dir, file));
    }
    return dir;
  };

  test('synth refuses a directory without libpdfium.so, or without bootstrap', () => {
    const noPdfium = copyIngest('libpdfium.so');
    const noBootstrap = copyIngest('bootstrap');
    expect(() => synth({ ingestAssetDir: noPdfium })).toThrow(/libpdfium\.so does not exist/);
    expect(() => synth({ ingestAssetDir: noBootstrap })).toThrow(/bootstrap does not exist/);
    fs.rmSync(noPdfium, { recursive: true, force: true });
    fs.rmSync(noBootstrap, { recursive: true, force: true });
  });
});

describe('the limits of phase 2', () => {
  test('context overrides the transcription caps, for the launch', () => {
    const s = synth({
      stage: 'prod',
      limitDailyTranscribedPagesGlobal: '1000',
      limitDailyTranscribedPagesPerUser: '50',
      limitPagesPerDocument: '100',
    });
    const env = envOf(functionBy(s, 'IngestFunction'));
    expect(env.OPENBOOK_LIMIT_DAILY_TRANSCRIBED_PAGES_GLOBAL).toBe('1000');
    expect(env.OPENBOOK_LIMIT_DAILY_TRANSCRIBED_PAGES_PER_USER).toBe('50');
    expect(env.OPENBOOK_LIMIT_PAGES_PER_DOCUMENT).toBe('100');
  });
});

describe('S3 Vectors resource types (Service Authorization Reference)', () => {
  type Statement = { Sid: string; Action: string | string[]; Resource: unknown };
  const bucketLevel = new Set(['s3vectors:CreateVectorBucket', 's3vectors:GetVectorBucket', 's3vectors:DeleteVectorBucket', 's3vectors:ListIndexes']);
  const indexLevel = new Set([
    's3vectors:CreateIndex',
    's3vectors:GetIndex',
    's3vectors:DeleteIndex',
    's3vectors:QueryVectors',
    's3vectors:PutVectors',
    's3vectors:GetVectors',
    's3vectors:DeleteVectors',
  ]);

  test('every index action, CreateIndex included, targets the course index ARN pattern and never the bucket ARN', () => {
    const roles = [
      ...roleStatements(prod, 'ApiFunctionServiceRoleDefaultPolicy'),
      ...roleStatements(prod, 'IngestFunctionServiceRoleDefaultPolicy'),
    ] as Statement[];
    const seen = new Set<string>();
    for (const st of roles) {
      for (const action of asArray(st.Action).filter((a) => a.startsWith('s3vectors:'))) {
        seen.add(action);
        expect(indexLevel.has(action), `${action} is an index action`).toBe(true);
        expect(flat(st.Resource), `${st.Sid} ${action}`).toBe(indexARN);
      }
    }
    expect(seen.has('s3vectors:CreateIndex')).toBe(true);
  });

  test('the boundary and the execution role follow the same resource types', () => {
    const values = { ...exampleValues(), ACCOUNT_ID: '111111111111' };
    const rendered = (name: string): { Statement: Statement[] } =>
      JSON.parse(renderIamDocument(readTemplate(name), values)) as { Statement: Statement[] };
    const boundary = rendered('permissions-boundary-policy');
    const vectors = boundary.Statement.filter((st) => asArray(st.Action).some((a) => a.startsWith('s3vectors:')));
    expect(vectors).toHaveLength(1);
    expect([...asArray(vectors[0].Action)].sort()).toEqual([...indexLevel].sort());
    expect(vectors[0].Resource).toBe(indexARN);
    const exec = rendered('cloudformation-execution-role-policy');
    const bucket = exec.Statement.find((st) => st.Sid === 'VectorBucket') as Statement;
    const bucketActions = asArray(bucket.Action).filter((a) => !/agResource|ListTagsForResource/.test(a));
    for (const action of bucketActions) expect(bucketLevel.has(action), action).toBe(true);
    expect(bucket.Resource).toBe('arn:aws:s3vectors:ap-southeast-2:111111111111:bucket/openbook-prod-*');
  });
});
