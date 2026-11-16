import * as cdk from 'aws-cdk-lib';

/**
 * How Openbook is deployed. The values here must agree with the IAM templates in
 * `infra/iam/`, which live outside CDK. `test/iam.test.ts` ties them together.
 */

/** The CDK bootstrap qualifier. The account is bootstrapped with the default one. */
export const bootstrapQualifier = 'hnb659fds';

/** The bootstrap bucket that assets are published to. */
export function assetBucketName(account: string, region: string): string {
  return `cdk-${bootstrapQualifier}-assets-${account}-${region}`;
}

/**
 * Deploy with the caller's own credentials and publish assets to the bootstrap bucket
 * directly.
 *
 * The default synthesizer makes every deployment assume the bootstrap deploy role, which
 * may pass only the bootstrap `cfn-exec-role`. That role has AdministratorAccess, so
 * anyone who can assume the deploy role is an administrator. With this synthesizer, the
 * GitHub OIDC role holds the narrow CloudFormation and asset permissions, and CI passes
 * the dedicated execution role with `cdk deploy --role-arn`. Only the bootstrap bucket
 * and the version parameter are still needed.
 */
export function deploymentSynthesizer(): cdk.IStackSynthesizer {
  // Upstream typing wart: concrete synthesizers return `string | undefined` for the
  // optional interface member, which `exactOptionalPropertyTypes` users must narrow.
  return new cdk.CliCredentialsStackSynthesizer({ qualifier: bootstrapQualifier });
}
