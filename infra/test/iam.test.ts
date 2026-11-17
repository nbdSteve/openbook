import * as fs from 'fs';
import * as path from 'path';
import { describe, expect, test } from 'vitest';
import { assetBucketName, assetPrefix, bootstrapQualifier, permissionsBoundaryName } from '../lib/deployment';
import { exampleValues, iamDir, placeholderRules, placeholdersIn, readTemplate, renderIamDocument, templateNames, type Values } from '../scripts/render-iam';
import { asArray, cdkJsonContext, fakeAccount, synth, type Json } from './helpers';

/**
 * The IAM documents live in an AWS account, outside CDK, so nothing synthesizes or
 * drift-checks them. These tests tie the committed templates to the prod stack.
 */

const values: Values = { ...exampleValues(), ACCOUNT_ID: fakeAccount };
const render = (name: string): { Version: string; Statement: Statement[] } =>
  JSON.parse(renderIamDocument(readTemplate(name), values)) as { Version: string; Statement: Statement[] };

interface Statement {
  Sid: string;
  Effect: string;
  Principal?: Json;
  Action: string | string[];
  Resource?: string | string[];
  Condition?: Record<string, Record<string, string>>;
}

const actions = (st: Statement): string[] => asArray(st.Action);
const resources = (st: Statement): string[] => asArray(st.Resource);
const allActions = (doc: { Statement: Statement[] }): string[] => doc.Statement.flatMap(actions);

const trust = render('github-deploy-role-trust');
const deployPolicy = render('github-deploy-role-policy');
const execTrust = render('cloudformation-execution-role-trust');
const execPolicy = render('cloudformation-execution-role-policy');

const prod = synth({ stage: 'prod' });
const region = 'ap-southeast-2';
const boundaryArn = `arn:aws:iam::${fakeAccount}:policy/${permissionsBoundaryName}`;

describe('the GitHub deployment role trust policy', () => {
  const [st] = trust.Statement;

  test('is one Allow statement that federates through the account GitHub OIDC provider', () => {
    expect(trust.Statement).toHaveLength(1);
    expect(st.Effect).toBe('Allow');
    expect(st.Action).toBe('sts:AssumeRoleWithWebIdentity');
    expect(st.Principal).toEqual({
      Federated: `arn:aws:iam::${fakeAccount}:oidc-provider/token.actions.githubusercontent.com`,
    });
  });

  test('admits exactly the production environment of the immutable repository identity', () => {
    expect(st.Condition).toEqual({
      StringEquals: {
        'token.actions.githubusercontent.com:aud': 'sts.amazonaws.com',
        'token.actions.githubusercontent.com:sub': 'repo:nbdSteve@1/openbook@2:environment:production',
      },
    });
  });

  test('matches by equality, never by pattern', () => {
    expect(JSON.stringify(st.Condition)).not.toMatch(/Like|\*/);
  });
});

describe('the GitHub deployment role policy', () => {
  const byAction = (action: string): Statement[] => deployPolicy.Statement.filter((s) => actions(s).includes(action));

  test('deploys Openbook-prod and no other stack', () => {
    const [st] = byAction('cloudformation:ExecuteChangeSet');
    expect(resources(st)).toEqual([`arn:aws:cloudformation:${region}:${fakeAccount}:stack/Openbook-prod/*`]);
    expect(prod.stack.stackName).toBe('Openbook-prod');
    for (const s of deployPolicy.Statement) {
      for (const r of resources(s)) expect(r).not.toContain('Openbook-dev');
    }
  });

  test('publishes assets only under the Openbook prefix of the bootstrap bucket', () => {
    const bucket = `arn:aws:s3:::${assetBucketName(fakeAccount, region)}`;
    const [put] = byAction('s3:PutObject*');
    expect(resources(put)).toEqual([`${bucket}/${assetPrefix('prod')}*`]);
    expect(put.Condition).toEqual({ StringEquals: { 'aws:ResourceAccount': fakeAccount } });
    expect(allActions(deployPolicy)).not.toContain('s3:DeleteObject');
    // Listing is limited to the same prefix, and no other statement reaches the bucket itself
    // with an object-level action.
    const [list] = byAction('s3:ListBucket');
    expect(resources(list)).toEqual([bucket]);
    expect(list.Condition).toEqual({
      StringEquals: { 'aws:ResourceAccount': fakeAccount },
      StringLike: { 's3:prefix': [`${assetPrefix('prod')}*`] },
    });
    for (const st of deployPolicy.Statement.filter((x) => resources(x).includes(bucket))) {
      expect(actions(st).filter((a) => /Object|Abort/.test(a)), st.Sid).toEqual([]);
    }
  });

  test('the synthesizer writes under the prefix the grants name', () => {
    const manifest = JSON.parse(
      fs.readFileSync(path.join(prod.app.synth().directory, 'Openbook-prod.assets.json'), 'utf8'),
    ) as { files: Record<string, { destinations: Record<string, { objectKey: string }> }> };
    const keys = Object.values(manifest.files).flatMap((f) => Object.values(f.destinations).map((d) => d.objectKey));
    expect(keys.length).toBeGreaterThan(0);
    for (const key of keys) expect(key).toMatch(new RegExp(`^${assetPrefix('prod')}`));
  });

  test('reads the bootstrap version parameter the qualifier names', () => {
    const [st] = byAction('ssm:GetParameter');
    expect(resources(st)).toEqual([
      `arn:aws:ssm:${region}:${fakeAccount}:parameter/cdk-bootstrap/${bootstrapQualifier}/version`,
    ]);
  });

  test('passes one role, only to CloudFormation, and holds no other IAM permission', () => {
    const [st] = byAction('iam:PassRole');
    expect(resources(st)).toEqual([`arn:aws:iam::${fakeAccount}:role/${values.CLOUDFORMATION_EXECUTION_ROLE_NAME}`]);
    expect(st.Condition).toEqual({ StringEquals: { 'iam:PassedToService': 'cloudformation.amazonaws.com' } });
    expect(allActions(deployPolicy).filter((a) => a.startsWith('iam:'))).toEqual(['iam:PassRole']);
  });

  test('grants on every resource only calls that take no resource', () => {
    const star = deployPolicy.Statement.filter((s) => resources(s).includes('*') && s.Effect === 'Allow');
    expect(star).toHaveLength(1);
    expect(actions(star[0]).sort()).toEqual([
      'cloudformation:GetTemplateSummary',
      'cloudformation:ListStacks',
      'sts:GetCallerIdentity',
    ]);
  });

  test('grants no service outside CloudFormation, STS, S3 assets, SSM, and PassRole', () => {
    const services = new Set(allActions(deployPolicy).map((a) => a.split(':')[0]));
    expect([...services].sort()).toEqual(['cloudformation', 'iam', 's3', 'ssm', 'sts']);
  });
});

describe('the CloudFormation execution role', () => {
  test('is assumable by CloudFormation and nothing else', () => {
    expect(execTrust.Statement).toHaveLength(1);
    expect(execTrust.Statement[0].Principal).toEqual({ Service: 'cloudformation.amazonaws.com' });
    expect(execTrust.Statement[0]).not.toHaveProperty('Condition');
  });

  test('grants no wildcard action', () => {
    for (const a of allActions(execPolicy)) expect(a).not.toContain('*');
  });

  test('has a unique Sid for every statement, and one Deny: it keeps the boundary on', () => {
    const sids = execPolicy.Statement.map((s) => s.Sid);
    expect(new Set(sids).size).toBe(sids.length);
    const denies = execPolicy.Statement.filter((s) => s.Effect === 'Deny');
    expect(denies.map((s) => s.Sid)).toEqual(['NeverRemoveTheBoundary']);
    expect(actions(denies[0])).toEqual(['iam:DeleteRolePermissionsBoundary']);
  });

  // Every resource type the prod stack creates, and the IAM service that deploys it.
  const serviceOf: Record<string, string | null> = {
    'AWS::Cognito::UserPool': 'cognito-idp',
    'AWS::Cognito::UserPoolClient': 'cognito-idp',
    'AWS::DynamoDB::Table': 'dynamodb',
    'AWS::Logs::LogGroup': 'logs',
    'AWS::IAM::Role': 'iam',
    'AWS::IAM::Policy': 'iam',
    'AWS::Lambda::Function': 'lambda',
    'AWS::Lambda::Url': 'lambda',
    'AWS::Lambda::Permission': 'lambda',
    'AWS::Lambda::EventSourceMapping': 'lambda',
    'AWS::SQS::Queue': 'sqs',
    'AWS::SQS::QueuePolicy': 'sqs',
    'AWS::S3Vectors::VectorBucket': 's3vectors',
    'AWS::Lambda::LayerVersion': 'lambda',
    'Custom::CDKBucketDeployment': 'lambda',
    'AWS::S3::Bucket': 's3',
    'AWS::S3::BucketPolicy': 's3',
    'AWS::CloudFront::Distribution': 'cloudfront',
    'AWS::CloudFront::Function': 'cloudfront',
    'AWS::CloudFront::OriginAccessControl': 'cloudfront',
    'AWS::CloudFront::OriginRequestPolicy': 'cloudfront',
    'AWS::CloudFront::ResponseHeadersPolicy': 'cloudfront',
    'AWS::Route53::RecordSet': 'route53',
    'AWS::CDK::Metadata': null,
  };
  // Services the policy uses without a stack resource of their own: reading assets and the
  // bootstrap version, and CloudFront attaching an ACM certificate (the certificate is a parameter).
  const supportServices = ['acm', 'ssm'];

  test('every resource type of the prod stack is one the policy was written for', () => {
    const types = new Set(Object.values(prod.json.Resources).map((r) => r.Type));
    for (const type of types) expect(serviceOf, `unmapped resource type ${type}`).toHaveProperty([type]);
  });

  test('the policy grants exactly the services the prod stack needs', () => {
    const needed = new Set(
      Object.values(prod.json.Resources)
        .map((r) => serviceOf[r.Type])
        .filter((s): s is string => Boolean(s)),
    );
    const granted = new Set(allActions(execPolicy).map((a) => a.split(':')[0]));
    expect([...granted].sort()).toEqual([...needed, ...supportServices].sort());
  });

  test('grants nothing on a service with a baseline cost or that the stack does not use', () => {
    const granted = allActions(execPolicy).map((a) => a.split(':')[0]);
    for (const service of ['kms', 'secretsmanager', 'ec2', 'rds', 'es', 'aoss', 'elasticache', 'wafv2', 'bedrock']) {
      expect(granted).not.toContain(service);
    }
  });

  test('scopes every resource-level statement to Openbook-prod names, except the listed ones', () => {
    const exceptions = new Set([
      'UserPoolCreateTakesNoResource',
      'UserPoolAndClient',
      'CloudFrontSite',
      'BucketDeploymentLayers',
      'DescribeLogGroupsTakesTheWholeRegion',
      'ReadTheStackAssets',
      'ReadTheBootstrapVersion',
      'AttachAnAcmCertificateToCloudFront',
      'ReadTheAcmCertificateMetadata',
      'DnsAliasRecordsOfTheSiteDomain',
      'ReadTheRecordsOfTheHostedZone',
      'WaitForRecordChanges',
      'CloudFrontSpaRewriteFunction',
      'IngestEventSourceMappingOfTheStackFunctions',
      'ReadAndDeleteEventSourceMappings',
      'NeverRemoveTheBoundary',
    ]);
    for (const s of execPolicy.Statement) {
      if (exceptions.has(s.Sid)) continue;
      for (const r of resources(s)) expect(r, s.Sid).toMatch(/Openbook-prod-|openbook-prod-/);
    }
  });

  test('the only statements on every resource are the ones that AWS cannot scope, and a Deny', () => {
    const star = execPolicy.Statement.filter((s) => resources(s).includes('*')).map((s) => s.Sid).sort();
    expect(star).toEqual([
      'AttachAnAcmCertificateToCloudFront',
      'CloudFrontSite',
      'NeverRemoveTheBoundary',
      'UserPoolCreateTakesNoResource',
    ]);
    const create = execPolicy.Statement.find((s) => s.Sid === 'UserPoolCreateTakesNoResource');
    expect(create && actions(create)).toEqual(['cognito-idp:CreateUserPool']);
  });

  test('passes the stack roles to Lambda alone and attaches only the basic execution policy', () => {
    const pass = execPolicy.Statement.find((s) => actions(s).includes('iam:PassRole'));
    expect(resources(pass as Statement)).toEqual([`arn:aws:iam::${fakeAccount}:role/Openbook-prod-*`]);
    expect((pass as Statement).Condition).toEqual({ StringEquals: { 'iam:PassedToService': 'lambda.amazonaws.com' } });
    const attach = execPolicy.Statement.find((s) => actions(s).includes('iam:AttachRolePolicy'));
    expect((attach as Statement).Condition).toEqual({
      ArnEquals: { 'iam:PolicyARN': 'arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole' },
      StringEquals: { 'iam:PermissionsBoundary': boundaryArn },
    });
    // The stack's roles use that one managed policy and no other.
    const managed = Object.values(prod.json.Resources)
      .filter((r) => r.Type === 'AWS::IAM::Role')
      .flatMap((r) => JSON.stringify(r.Properties?.ManagedPolicyArns ?? []));
    for (const m of managed) {
      expect(m).toContain('policy/service-role/AWSLambdaBasicExecutionRole');
      expect(m.match(/policy\//g)).toHaveLength(1);
    }
  });

  test('the layer grants match both synthesized bucket-deployment layers', () => {
    const layerIds = Object.entries(prod.json.Resources)
      .filter(([, r]) => r.Type === 'AWS::Lambda::LayerVersion')
      .map(([id]) => id);
    expect(layerIds).toHaveLength(2);
    const layer = execPolicy.Statement.find((s) => s.Sid === 'BucketDeploymentLayers') as Statement;
    for (const id of layerIds) {
      const covered = resources(layer).some((r) => {
        const pattern = r.split(':layer:')[1].replace(/:\*$/, '').replace(/\*$/, '');
        return id.startsWith(pattern);
      });
      expect(covered, id).toBe(true);
    }
  });

  test('the invoke grant matches the synthesized bucket-deployment provider function', () => {
    const fnId = Object.entries(prod.json.Resources).find(
      ([id, r]) => r.Type === 'AWS::Lambda::Function' && id.startsWith('CustomCDKBucketDeployment'),
    )?.[0];
    expect(fnId).toBeDefined();
    const invoke = execPolicy.Statement.find((s) => s.Sid === 'InvokeTheBucketDeploymentProvider') as Statement;
    expect(actions(invoke)).toEqual(['lambda:InvokeFunction']);
    const [pattern] = resources(invoke).map((r) => r.split(':function:Openbook-prod-')[1].replace('*', ''));
    expect(fnId?.startsWith(pattern)).toBe(true);
  });

  test('the bucket grants cover the web bucket pattern and the fixed docs bucket name', () => {
    const [st] = execPolicy.Statement.filter((s) => s.Sid === 'StackBuckets');
    expect(resources(st)).toEqual(['arn:aws:s3:::openbook-prod-*']);
    expect(prod.stack.stackName.toLowerCase()).toBe('openbook-prod');
    const docs = Object.values(prod.json.Resources).find(
      (r) => r.Type === 'AWS::S3::Bucket' && r.Properties?.BucketName,
    );
    expect(docs?.Properties?.BucketName).toBe(`openbook-prod-docs-${fakeAccount}-${region}`);
  });

  test('the table and log group grants use the CloudFormation naming of this stack', () => {
    const table = execPolicy.Statement.find((s) => s.Sid === 'ApplicationTable') as Statement;
    expect(resources(table)[0]).toBe(`arn:aws:dynamodb:${region}:${fakeAccount}:table/Openbook-prod-*`);
    const logs = execPolicy.Statement.find((s) => s.Sid === 'StackLogGroups') as Statement;
    expect(resources(logs)[0]).toBe(`arn:aws:logs:${region}:${fakeAccount}:log-group:Openbook-prod-*`);
    // None of these resources has an explicit physical name, so CloudFormation prefixes the
    // stack name. The docs bucket is the one exception and is tested above.
    for (const [type, key] of [
      ['AWS::DynamoDB::Table', 'TableName'],
      ['AWS::Logs::LogGroup', 'LogGroupName'],
      ['AWS::IAM::Role', 'RoleName'],
    ] as const) {
      for (const r of Object.values(prod.json.Resources).filter((x) => x.Type === type)) {
        expect(r.Properties).not.toHaveProperty(key);
      }
    }
    for (const r of Object.values(prod.json.Resources).filter((x) => x.Type === 'AWS::Lambda::Function')) {
      expect(r.Properties).not.toHaveProperty('FunctionName');
    }
  });

  test('reads the assets from the bootstrap bucket the synthesizer publishes to', () => {
    const st = execPolicy.Statement.find((s) => s.Sid === 'ReadTheStackAssets') as Statement;
    expect(resources(st)).toEqual([`arn:aws:s3:::${assetBucketName(fakeAccount, region)}/${assetPrefix('prod')}*`]);
    const assembly = prod.app.synth();
    const manifest = JSON.parse(fs.readFileSync(path.join(assembly.directory, 'Openbook-prod.assets.json'), 'utf8')) as {
      files: Record<string, { destinations: Record<string, { bucketName: string }> }>;
    };
    const buckets = new Set(
      Object.values(manifest.files).flatMap((f) => Object.values(f.destinations).map((d) => d.bucketName)),
    );
    expect([...buckets]).toEqual([assetBucketName(fakeAccount, region)]);
  });
});

describe('synthesis under the deployment synthesizer', () => {
  test('depends on none of the CDK bootstrap roles', () => {
    const assembly = prod.app.synth();
    const artifact = assembly.getStackByName('Openbook-prod');
    expect(artifact.assumeRoleArn).toBeUndefined();
    expect(artifact.cloudFormationExecutionRoleArn).toBeUndefined();
    expect(artifact.lookupRole).toBeUndefined();
    const text = fs.readFileSync(path.join(assembly.directory, 'manifest.json'), 'utf8');
    expect(text).not.toContain('deploy-role');
    expect(text).not.toContain('cfn-exec-role');
    expect(text).not.toContain('file-publishing-role');
  });
});

describe('the committed IAM templates', () => {
  test('are the five documents applied to the two roles and the boundary policy', () => {
    expect(templateNames()).toEqual([
      'cloudformation-execution-role-policy',
      'cloudformation-execution-role-trust',
      'github-deploy-role-policy',
      'github-deploy-role-trust',
      'permissions-boundary-policy',
    ]);
  });

  test('carry no twelve-digit account number', () => {
    for (const name of templateNames()) expect(readTemplate(name), name).not.toMatch(/\d{12}/);
    expect(fs.readFileSync(path.join(iamDir, 'example-values.json'), 'utf8')).toContain('"000000000000"');
  });

  test('use only placeholders the render script knows', () => {
    for (const name of templateNames()) {
      for (const p of placeholdersIn(readTemplate(name))) expect(Object.keys(placeholderRules)).toContain(p);
    }
  });

  test('name IAM roles only through a placeholder or the stack own prefix', () => {
    for (const name of templateNames()) {
      for (const m of readTemplate(name).matchAll(/:role\/([^"]+)"/g)) {
        expect(m[1], name).toMatch(/^(\$\{CLOUDFORMATION_EXECUTION_ROLE_NAME\}|Openbook-prod-\*)$/);
      }
    }
  });

  test('refer to the one stage the production role may deploy', () => {
    for (const name of templateNames()) {
      expect(readTemplate(name), name).not.toMatch(/Openbook-dev|openbook-dev/);
    }
  });
});

const byAction2 = (list: Statement[], action: string): Statement => list.find((s) => actions(s).includes(action)) as Statement;

describe('the custom domain permissions of the execution role', () => {
  const zoneArn = `arn:aws:route53:::hostedzone/${values.HOSTED_ZONE_ID}`;
  const domain = (cdkJsonContext.stageDomains as { prod: { domainName: string } }).prod.domainName;
  const route53 = execPolicy.Statement.filter((s) => actions(s).some((a) => a.startsWith('route53:')));
  const byAction = (action: string): Statement[] => route53.filter((s) => actions(s).includes(action));

  test('grant exactly four Route 53 actions', () => {
    expect(route53.flatMap(actions).sort()).toEqual([
      'route53:ChangeResourceRecordSets',
      'route53:GetChange',
      'route53:GetHostedZone',
      'route53:ListResourceRecordSets',
    ]);
    for (const s of route53) expect(s.Effect).toBe('Allow');
  });

  test('change records only in the one hosted zone, only the domain name, only A and AAAA', () => {
    const [change] = byAction('route53:ChangeResourceRecordSets');
    expect(resources(change)).toEqual([zoneArn]);
    expect(change.Condition).toEqual({
      'ForAllValues:StringEquals': {
        'route53:ChangeResourceRecordSetsNormalizedRecordNames': domain,
        'route53:ChangeResourceRecordSetsRecordTypes': ['A', 'AAAA'],
      },
      // A request without these keys must not match the ForAllValues conditions.
      Null: {
        'route53:ChangeResourceRecordSetsNormalizedRecordNames': 'false',
        'route53:ChangeResourceRecordSetsRecordTypes': 'false',
      },
    });
  });

  test('the record types and the name are the ones that the prod stack creates', () => {
    const records = Object.values(prod.json.Resources).filter((r) => r.Type === 'AWS::Route53::RecordSet');
    expect(records.map((r) => r.Properties?.Type).sort()).toEqual(['A', 'AAAA']);
    for (const r of records) expect(r.Properties?.Name).toBe(`${domain}.`);
  });

  test('read the zone and its records in the same zone, and a change by ID (AWS has no narrower resource)', () => {
    // CloudFormation probably reads the zone for a record set, and AWS does not document it.
    expect(resources(byAction('route53:ListResourceRecordSets')[0])).toEqual([zoneArn]);
    expect(resources(byAction('route53:GetHostedZone')[0])).toEqual([zoneArn]);
    expect(actions(byAction('route53:GetHostedZone')[0]).sort()).toEqual(['route53:GetHostedZone', 'route53:ListResourceRecordSets']);
    expect(resources(byAction('route53:GetChange')[0])).toEqual(['arn:aws:route53:::change/*']);
  });

  test('never name a zone or use a wildcard zone', () => {
    const text = readTemplate('cloudformation-execution-role-policy');
    expect(text).toContain('hostedzone/${HOSTED_ZONE_ID}');
    expect(text).not.toMatch(/hostedzone\/(\*|Z[A-Z0-9]+)/);
    expect(text).not.toContain('route53:*');
  });

  // AWS documents acm:ListCertificates for assigning a certificate in the console. It lists no ACM
  // action that CreateDistribution or UpdateDistribution needs. Both actions are read-only
  // guards for the first deploy, not a documented need.
  test('allow only two read-only ACM actions, as guards for attaching the certificate', () => {
    const acm = execPolicy.Statement.filter((s) => actions(s).some((a) => a.startsWith('acm:')));
    expect(acm.flatMap(actions).sort()).toEqual(['acm:DescribeCertificate', 'acm:ListCertificates']);
    for (const s of acm) expect(s.Effect).toBe('Allow');
    expect(resources(byAction2(acm, 'acm:ListCertificates'))).toEqual(['*']);
    expect(resources(byAction2(acm, 'acm:DescribeCertificate'))).toEqual([
      `arn:aws:acm:us-east-1:${fakeAccount}:certificate/*`,
    ]);
  });

  test('keep Route 53 and ACM out of the deploy role and the permissions boundary', () => {
    for (const name of ['github-deploy-role-policy', 'permissions-boundary-policy']) {
      expect(readTemplate(name), name).not.toMatch(/route53|acm:/);
    }
  });
});
