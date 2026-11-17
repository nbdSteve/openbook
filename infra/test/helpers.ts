import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import * as cdk from 'aws-cdk-lib';
import { Template } from 'aws-cdk-lib/assertions';
import { afterAll } from 'vitest';
import { resolveConfig } from '../lib/config';
import { deploymentSynthesizer } from '../lib/deployment';
import { OpenbookStack } from '../lib/openbook-stack';

/** A fake account. Tests never need AWS credentials or lookups. */
export const fakeAccount = '111111111111';

export const fixtures = {
  api: path.resolve(__dirname, 'fixtures/api'),
  ingest: path.resolve(__dirname, 'fixtures/ingest'),
  web: path.resolve(__dirname, 'fixtures/web'),
};

export type Json = Record<string, unknown>;

export interface Synth {
  app: cdk.App;
  stack: OpenbookStack;
  template: Template;
  json: { Resources: Record<string, { Type: string; Properties?: Json; DeletionPolicy?: string; UpdateReplacePolicy?: string }> };
}

/** The context of cdk.json, which the CDK CLI loads but `new App` does not. It holds the model catalog. */
export const cdkJsonContext = (
  JSON.parse(fs.readFileSync(path.resolve(__dirname, '../cdk.json'), 'utf8')) as { context: Record<string, unknown> }
).context;

/**
 * Every synth copies the 30 MB AWS CLI layer into a cloud assembly folder. Put the folders
 * in one place and remove them when the test file ends, or a full run fills the disk.
 */
const assemblyRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'openbook-cdk-'));
afterAll(() => fs.rmSync(assemblyRoot, { recursive: true, force: true }));

/** A new App that writes its cloud assembly under the folder that this test file cleans up. */
export function newApp(context: Record<string, unknown>): cdk.App {
  return new cdk.App({ context, outdir: fs.mkdtempSync(path.join(assemblyRoot, 'out-')) });
}

export function synth(context: Record<string, unknown> = {}): Synth {
  const app = newApp({ ...cdkJsonContext, apiAssetDir: fixtures.api, ingestAssetDir: fixtures.ingest, webAssetDir: fixtures.web, ...context });
  const config = resolveConfig(app.node);
  const stack = new OpenbookStack(app, `Openbook-${config.stage}`, {
    config,
    env: { account: fakeAccount, region: 'ap-southeast-2' },
    synthesizer: deploymentSynthesizer(config.stage),
    terminationProtection: config.stage === 'prod',
  });
  const template = Template.fromStack(stack);
  // Drop the big layer zip. No test reads a zip, and it adds up to gigabytes over a run.
  const outdir = app.outdir;
  for (const file of fs.readdirSync(outdir)) {
    if (file.endsWith('.zip')) fs.rmSync(path.join(outdir, file), { force: true });
  }
  return { app, stack, template, json: template.toJSON() as unknown as Synth['json'] };
}

/** All statements of every IAM policy in the template, with the role they attach to. */
export function policyStatements(json: Synth['json']): Json[] {
  const out: Json[] = [];
  for (const resource of Object.values(json.Resources)) {
    if (resource.Type !== 'AWS::IAM::Policy') continue;
    const doc = resource.Properties?.PolicyDocument as { Statement: Json[] };
    out.push(...doc.Statement);
  }
  return out;
}

export function asArray<T>(value: T | T[] | undefined): T[] {
  if (value === undefined) return [];
  return Array.isArray(value) ? value : [value];
}

/**
 * Synthesizes the prod stack the way the CI build job does (no account) and copies the cloud
 * assembly to `target`. The layer zip is emptied, because validators only check that it exists.
 */
export function copyAgnosticAssembly(target: string): void {
  const app = newApp({ ...cdkJsonContext, stage: 'prod', apiAssetDir: fixtures.api, ingestAssetDir: fixtures.ingest, webAssetDir: fixtures.web });
  const config = resolveConfig(app.node);
  new OpenbookStack(app, 'Openbook-prod', {
    config,
    env: { region: 'ap-southeast-2' },
    synthesizer: deploymentSynthesizer(config.stage),
  });
  fs.cpSync(app.synth().directory, target, { recursive: true });
  for (const file of fs.readdirSync(target)) {
    if (file.endsWith('.zip')) fs.writeFileSync(path.join(target, file), '');
  }
}
