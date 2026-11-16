import * as fs from 'fs';
import * as path from 'path';
import * as cdk from 'aws-cdk-lib';
import { Template } from 'aws-cdk-lib/assertions';
import { CertificateStack } from '../lib/certificate-stack';
import { resolveConfig } from '../lib/config';
import { deploymentSynthesizer } from '../lib/deployment';
import { OpenbookStack } from '../lib/openbook-stack';

/** A fake account. Tests never need AWS credentials or lookups. */
export const fakeAccount = '111111111111';

export const fixtures = {
  api: path.resolve(__dirname, 'fixtures/api'),
  web: path.resolve(__dirname, 'fixtures/web'),
};

export type Json = Record<string, unknown>;

export interface Synth {
  app: cdk.App;
  stack: OpenbookStack;
  certStack?: CertificateStack;
  template: Template;
  json: { Resources: Record<string, { Type: string; Properties?: Json; DeletionPolicy?: string; UpdateReplacePolicy?: string }> };
}

/** The context of cdk.json, which the CDK CLI loads but `new App` does not. It holds the model catalog. */
export const cdkJsonContext = (
  JSON.parse(fs.readFileSync(path.resolve(__dirname, '../cdk.json'), 'utf8')) as { context: Record<string, unknown> }
).context;

export function synth(context: Record<string, unknown> = {}): Synth {
  const app = new cdk.App({
    context: { ...cdkJsonContext, apiAssetDir: fixtures.api, webAssetDir: fixtures.web, ...context },
  });
  const config = resolveConfig(app.node);
  let certStack: CertificateStack | undefined;
  if (config.domain) {
    certStack = new CertificateStack(app, `OpenbookCert-${config.stage}`, {
      domain: config.domain,
      env: { account: fakeAccount, region: 'us-east-1' },
      synthesizer: deploymentSynthesizer(),
    });
  }
  const stack = new OpenbookStack(app, `Openbook-${config.stage}`, {
    config,
    env: { account: fakeAccount, region: 'ap-southeast-2' },
    synthesizer: deploymentSynthesizer(),
    terminationProtection: config.stage === 'prod',
    ...(certStack ? { certificate: certStack.certificate, crossRegionReferences: true } : {}),
  });
  const template = Template.fromStack(stack);
  return { app, stack, certStack, template, json: template.toJSON() as unknown as Synth['json'] };
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
