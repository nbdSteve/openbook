#!/usr/bin/env node
import * as cdk from 'aws-cdk-lib';
import { region, resolveConfig } from '../lib/config';
import { deploymentSynthesizer } from '../lib/deployment';
import { OpenbookStack } from '../lib/openbook-stack';

const app = new cdk.App();
const config = resolveConfig(app.node);
const account = process.env.CDK_DEFAULT_ACCOUNT;
const env = { ...(account ? { account } : {}), region };

new OpenbookStack(app, `Openbook-${config.stage}`, {
  config,
  env,
  description: `Openbook (${config.stage}): Ask web app, API, and data`,
  synthesizer: deploymentSynthesizer(config.stage),
  terminationProtection: config.stage === 'prod',
});
