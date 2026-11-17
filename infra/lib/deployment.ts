import * as cdk from 'aws-cdk-lib';
import type { Stage } from './config';

/**
 * How Openbook is deployed. The values here must agree with the IAM templates in
 * `infra/iam/`, which live outside CDK. `test/iam.test.ts` ties them together.
 */

/** The CDK bootstrap qualifier. The account is bootstrapped with the default one. */
export const bootstrapQualifier = 'hnb659fds';

/**
 * The key prefix of this app's objects in the shared bootstrap bucket. The deploy role may
 * write only under the prod prefix, so it cannot overwrite the assets of other apps.
 */
export function assetPrefix(stage: Stage): string {
  return `openbook-${stage}/`;
}

/** The bootstrap bucket that assets are published to. */
export function assetBucketName(account: string, region: string): string {
  return `cdk-${bootstrapQualifier}-assets-${account}-${region}`;
}

/**
 * The customer-managed policy that bounds every role of the prod stack. The template is
 * `infra/iam/permissions-boundary-policy.template.json`. The execution role may create or
 * change a role only when it carries this boundary.
 */
export const permissionsBoundaryName = 'OpenbookProdPermissionsBoundary';

/**
 * The shapes of the two deploy-time parameters of a stage with a custom domain. The stack
 * checks them as `AllowedPattern`, `scripts/render-iam.ts` checks the zone ID, and the deploy
 * workflow checks both in bash. `test/workflows.test.ts` ties the workflow copy to these.
 */
export const hostedZoneIdPattern = '^Z[A-Z0-9]{1,32}$';
export const certificateIdPattern = '[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}';
/** An ACM certificate in us-east-1 (CloudFront reads certificates only there). */
export const certificateArnPattern = `^arn:aws:acm:us-east-1:[0-9]{12}:certificate/${certificateIdPattern}$`;

/** The name of the CloudFront Function. The execution role is scoped to it. */
export function spaRewriteFunctionName(stage: Stage): string {
  return `Openbook-${stage}-SpaRewrite`;
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
 * is still needed.
 */
export function deploymentSynthesizer(stage: Stage): cdk.IStackSynthesizer {
  return new cdk.CliCredentialsStackSynthesizer({
    qualifier: bootstrapQualifier,
    bucketPrefix: assetPrefix(stage),
  });
}
