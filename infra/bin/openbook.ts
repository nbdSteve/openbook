#!/usr/bin/env node
import * as cdk from 'aws-cdk-lib';
import { CertificateStack } from '../lib/certificate-stack';
import { region, resolveConfig } from '../lib/config';
import { deploymentSynthesizer } from '../lib/deployment';
import { OpenbookStack } from '../lib/openbook-stack';

const app = new cdk.App();
const config = resolveConfig(app.node);
const account = process.env.CDK_DEFAULT_ACCOUNT;
const env = { ...(account ? { account } : {}), region };

let certificate;
if (config.domain) {
  if (!account) throw new Error('a custom domain needs CDK_DEFAULT_ACCOUNT (cross-Region references)');
  const certStack = new CertificateStack(app, `OpenbookCert-${config.stage}`, {
    domain: config.domain,
    env: { account, region: 'us-east-1' },
    synthesizer: deploymentSynthesizer(),
  });
  certificate = certStack.certificate;
}

new OpenbookStack(app, `Openbook-${config.stage}`, {
  config,
  env,
  description: `Openbook (${config.stage}): Ask web app, API, and data`,
  synthesizer: deploymentSynthesizer(),
  terminationProtection: config.stage === 'prod',
  ...(certificate ? { certificate, crossRegionReferences: true } : {}),
});
