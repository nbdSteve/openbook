import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { describe, expect, test } from 'vitest';
import { bedrockResourceArns } from '../lib/catalog';
import { resolveConfig } from '../lib/config';
import { deploymentSynthesizer, permissionsBoundaryName, spaRewriteFunctionName } from '../lib/deployment';
import { OpenbookStack } from '../lib/openbook-stack';
import { assertNoInlineScripts } from '../lib/web-assets';
import { exampleValues, readTemplate, renderIamDocument, templateNames, type Values } from '../scripts/render-iam';
import { asArray, cdkJsonContext, fakeAccount, fixtures, newApp, synth, type Json } from './helpers';

const prod = synth({ stage: 'prod' });
const dev = synth({ stage: 'dev' });
const values: Values = { ...exampleValues(), ACCOUNT_ID: fakeAccount };
const rendered = (name: string): { Statement: Json[] } =>
  JSON.parse(renderIamDocument(readTemplate(name), values)) as { Statement: Json[] };

// ---- Normalizing CloudFormation values so that policies can be compared exactly ----------

/**
 * Resolves a CloudFormation value to a string. A reference to a resource becomes the shape
 * of the physical name CloudFormation gives it: the stack name plus the logical ID for most
 * resources, and `openbook-prod-<logical>` for buckets.
 */
function flatten(value: unknown, json: Synth['json']): string {
  if (typeof value === 'string') return value;
  const v = value as Record<string, unknown>;
  if ('Fn::Join' in v) {
    const [sep, parts] = v['Fn::Join'] as [string, unknown[]];
    return parts.map((p) => flatten(p, json)).join(sep);
  }
  const getAtt = v['Fn::GetAtt'] as string[] | undefined;
  const reference = (v.Ref ?? getAtt?.[0]) as string | undefined;
  if (reference === 'AWS::Partition') return 'aws';
  if (reference === 'AWS::AccountId') return fakeAccount;
  if (reference === 'AWS::Region') return 'ap-southeast-2';
  if (reference) {
    const resource = json.Resources[reference];
    const logical = reference.replace(/[0-9A-F]{8}$/, '');
    const isArn = getAtt?.[1] === 'Arn';
    const acct = `ap-southeast-2:${fakeAccount}`;
    if (resource?.Type === 'AWS::S3::Bucket') {
      return `${isArn ? 'arn:aws:s3:::' : ''}openbook-prod-${logical.toLowerCase()}`;
    }
    if (resource?.Type === 'AWS::SQS::Queue') {
      const name = (resource.Properties as { QueueName: string }).QueueName;
      return isArn ? `arn:aws:sqs:${acct}:${name}` : `https://sqs/${name}`;
    }
    if (resource?.Type === 'AWS::Lambda::Function') {
      return `${isArn ? `arn:aws:lambda:${acct}:function:` : ''}Openbook-prod-${logical}`;
    }
    if (resource?.Type === 'AWS::DynamoDB::Table') {
      return `${isArn ? `arn:aws:dynamodb:${acct}:table/` : ''}Openbook-prod-${logical}`;
    }
    if (resource?.Type === 'AWS::Logs::LogGroup') {
      return `${isArn ? `arn:aws:logs:${acct}:log-group:` : ''}Openbook-prod-${logical}${isArn ? ':*' : ''}`;
    }
    return `<${logical}>`;
  }
  throw new Error(`cannot flatten ${JSON.stringify(value)}`);
}

type Synth = ReturnType<typeof synth>;

interface NormalizedStatement {
  Effect: string;
  Action: string[];
  Resource: string[];
  Condition?: unknown;
}

function stackStatements(s: Synth): { policy: string; statement: NormalizedStatement }[] {
  const out: { policy: string; statement: NormalizedStatement }[] = [];
  for (const [id, resource] of Object.entries(s.json.Resources)) {
    if (resource.Type !== 'AWS::IAM::Policy') continue;
    const doc = resource.Properties?.PolicyDocument as { Statement: Json[] };
    for (const st of doc.Statement) {
      out.push({
        policy: id.replace(/[0-9A-F]{8}$/, ''),
        statement: {
          Effect: st.Effect as string,
          Action: asArray(st.Action as string | string[]).sort(),
          Resource: asArray(st.Resource as unknown | unknown[]).map((r) => flatten(r, s.json)).sort(),
          ...(st.Condition ? { Condition: st.Condition } : {}),
        },
      });
    }
  }
  return out;
}

/** Glob match for IAM actions and ARNs: `*` matches anything. */
const glob = (pattern: string, text: string): boolean =>
  new RegExp(`^${pattern.replace(/[.+?^${}()|[\]\\]/g, '\\$&').replace(/\*/g, '.*')}$`).test(text);

describe('the permissions boundary', () => {
  const roles = (s: Synth): [string, { Properties?: Json }][] =>
    Object.entries(s.json.Resources).filter(([, r]) => r.Type === 'AWS::IAM::Role');

  test('every IAM role of the prod stack carries it', () => {
    expect(roles(prod).length).toBeGreaterThanOrEqual(2);
    for (const [id, role] of roles(prod)) {
      expect(JSON.stringify(role.Properties?.PermissionsBoundary), id).toContain(
        `:policy/${permissionsBoundaryName}`,
      );
    }
  });

  test('the dev stack has no boundary, because the dev account has no such policy', () => {
    for (const [, role] of roles(dev)) expect(role.Properties).not.toHaveProperty('PermissionsBoundary');
  });

  test('the boundary policy exists as a template with the same name in its grants', () => {
    const doc = rendered('permissions-boundary-policy');
    expect(doc.Statement.length).toBeGreaterThan(0);
    for (const st of doc.Statement) {
      expect(st.Effect).toBe('Allow');
      expect(JSON.stringify(st.Resource)).not.toMatch(/"\*"/);
    }
    // No IAM, STS, Lambda, CloudFormation, or Cognito permission: a role under the boundary
    // can never create or change another role.
    const services = new Set(doc.Statement.flatMap((st) => asArray(st.Action as string[]).map((a) => a.split(':')[0])));
    expect([...services].sort()).toEqual(['bedrock', 'cloudfront', 'dynamodb', 'logs', 's3', 's3vectors', 'sqs']);
  });

  test('every grant of the stack roles fits inside the boundary', () => {
    const boundary = rendered('permissions-boundary-policy').Statement.map((st) => ({
      actions: asArray(st.Action as string[]),
      resources: asArray(st.Resource as string[]),
    }));
    const statements = stackStatements(prod);
    expect(statements.length).toBeGreaterThan(0);
    for (const { policy, statement } of statements) {
      for (const action of statement.Action) {
        for (const raw of statement.Resource) {
          // CDK grants the bucket-deployment role read on every object of the bootstrap bucket.
          // The boundary narrows that to the Openbook prefix, so test the narrowed ARN.
          const resource = raw.replace(/(cdk-hnb659fds-assets-[0-9]+-ap-southeast-2)\/\*$/, '$1/openbook-prod/*');
          // A grant on `*` is narrowed to the boundary resources, so the action must be in the
          // boundary at all. Only CDK's own CloudFront invalidation grant is allowed to do that.
          if (resource === '*') expect(action, policy).toMatch(/^cloudfront:(Create|Get)Invalidation$/);
          const allowed = boundary.some(
            (b) =>
              b.actions.some((p) => glob(p, action) || glob(action, p)) &&
              (resource === '*' || b.resources.some((p) => glob(p, resource))),
          );
          expect(allowed, `${policy}: ${action} on ${resource} is outside the boundary`).toBe(true);
        }
      }
    }
    // The managed policy that CDK attaches to both Lambda roles: it grants logging on
    // every resource, so the boundary alone must allow the log actions on the stack's groups.
    for (const action of ['logs:CreateLogGroup', 'logs:CreateLogStream', 'logs:PutLogEvents']) {
      expect(boundary.some((b) => b.actions.includes(action))).toBe(true);
    }
  });

  test('the execution role creates or changes a role only with that boundary, and never removes it', () => {
    const exec = rendered('cloudformation-execution-role-policy').Statement as {
      Sid: string;
      Effect: string;
      Action: string | string[];
      Condition?: { StringEquals?: Record<string, string> };
    }[];
    const arn = `arn:aws:iam::${fakeAccount}:policy/${permissionsBoundaryName}`;
    const writes = new Set([
      'iam:CreateRole',
      'iam:PutRolePolicy',
      'iam:AttachRolePolicy',
      'iam:DetachRolePolicy',
      'iam:DeleteRolePolicy',
      'iam:PutRolePermissionsBoundary',
    ]);
    const seen = new Set<string>();
    for (const st of exec.filter((x) => x.Effect === 'Allow')) {
      const hit = asArray(st.Action).filter((a) => writes.has(a));
      if (hit.length === 0) continue;
      for (const a of hit) seen.add(a);
      expect(st.Condition?.StringEquals?.['iam:PermissionsBoundary'], st.Sid).toBe(arn);
    }
    expect([...seen].sort()).toEqual([...writes].sort());
    const allowed = exec.filter((x) => x.Effect === 'Allow').flatMap((x) => asArray(x.Action));
    expect(allowed).not.toContain('iam:DeleteRolePermissionsBoundary');
    expect(allowed.filter((a) => /iam:(Create|Delete|Set|Put|Update).*Polic(y|ies)(Version)?$/.test(a)).sort()).toEqual([
      'iam:DeleteRolePolicy',
      'iam:PutRolePolicy',
      'iam:UpdateAssumeRolePolicy',
    ]);
    expect(allowed).not.toContain('iam:CreatePolicyVersion');
    expect(allowed).not.toContain('iam:SetDefaultPolicyVersion');
    // The role may be assumed by CloudFormation only, and the boundary name matches the CDK app.
    expect(exec.find((x) => x.Effect === 'Deny')?.Action).toBe('iam:DeleteRolePermissionsBoundary');
  });
});

describe('exact policy lists', () => {
  test('the stack roles hold exactly the committed statements', async () => {
    await expect(JSON.stringify(stackStatements(prod), null, 2) + '\n').toMatchFileSnapshot(
      'snapshots/stack-policies.prod.json',
    );
  });

  test.each(templateNames())('the rendered %s is exactly the committed document', async (name) => {
    await expect(renderIamDocument(readTemplate(name), values)).toMatchFileSnapshot(`snapshots/iam-${name}.json`);
  });

  test('no statement of the stack roles grants a wildcard action, or a wildcard resource other than the list', () => {
    for (const { policy, statement } of stackStatements(prod)) {
      for (const action of statement.Action) {
        // CDK writes these from `grantRead` style calls; each is a read or object-scoped family.
        if (action.includes('*')) expect(action, policy).toMatch(/^s3:(Abort|DeleteObject|GetBucket|GetObject|List|PutObject)[A-Za-z]*\*$/);
      }
      // The one grant on every resource is CDK's own CloudFront invalidation, which the
      // boundary narrows to the account's distributions.
      if (statement.Resource.includes('*')) {
        expect(statement.Action, policy).toEqual(['cloudfront:CreateInvalidation', 'cloudfront:GetInvalidation']);
      }
    }
  });
});

describe('the CloudFront Function', () => {
  test('has the fixed name that the execution role is scoped to', () => {
    const fn = Object.values(prod.json.Resources).find((r) => r.Type === 'AWS::CloudFront::Function');
    expect(fn?.Properties?.Name).toBe(spaRewriteFunctionName('prod'));
    const exec = rendered('cloudformation-execution-role-policy').Statement;
    const st = exec.find((x) => x.Sid === 'CloudFrontSpaRewriteFunction');
    expect(st?.Resource).toBe(`arn:aws:cloudfront::${fakeAccount}:function/${spaRewriteFunctionName('prod')}`);
  });
});

describe('inline scripts', () => {
  const dirWith = (html: string): string => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'openbook-web-'));
    fs.writeFileSync(path.join(dir, 'index.html'), html);
    return dir;
  };

  test('the fixture page passes', () => {
    expect(() => assertNoInlineScripts(fixtures.web)).not.toThrow();
  });

  test.each([
    '<script>window.x = 1</script>',
    '<script type="module">import "/a.js"</script>',
    '<SCRIPT>1</SCRIPT>',
    '<script\n>1</script>',
    '<script async>1</script>',
  ])('refuses %j', (html) => {
    expect(() => assertNoInlineScripts(dirWith(`<html><head>${html}</head></html>`))).toThrow(/inline <script>/);
  });

  test.each([
    '<script type="module" src="/assets/app.js"></script>',
    '<script src="/theme-init.js"></script>',
    "<script src='/a.js' defer></script>",
    '<script type="application/json">{"a":1}</script>',
    '<script type="application/ld+json">{"a":1}</script>',
  ])('accepts %j', (html) => {
    expect(() => assertNoInlineScripts(dirWith(`<html><head>${html}</head></html>`))).not.toThrow();
  });

  test('synth refuses a web build with an inline script, and a missing build', () => {
    const dir = dirWith('<script>1</script>');
    expect(() => synth({ webAssetDir: dir })).toThrow(/inline <script>/);
    expect(() => synth({ webAssetDir: path.join(dir, 'nope') })).toThrow(/does not exist/);
  });
});

describe('deployment logs and limits', () => {
  test('both bucket-deployment functions write to one log group with the stage retention', () => {
    for (const [s, days] of [
      [prod, 30],
      [dev, 7],
    ] as const) {
      const handler = Object.entries(s.json.Resources).find(
        ([id, r]) => r.Type === 'AWS::Lambda::Function' && id.startsWith('CustomCDKBucketDeployment'),
      )?.[1];
      expect(JSON.stringify(handler?.Properties?.LoggingConfig)).toContain('DeploymentLogGroup');
      const groups = Object.entries(s.json.Resources).filter(
        ([id, r]) => r.Type === 'AWS::Logs::LogGroup' && id.startsWith('DeploymentLogGroup'),
      );
      expect(groups).toHaveLength(1);
      expect(groups[0][1].Properties?.RetentionInDays).toBe(days);
    }
  });

  test('the bucket-deployment constructs have an Openbook prefix', () => {
    const ids = Object.keys(prod.json.Resources).filter((id) => id.includes('AwsCliLayer'));
    expect(ids.length).toBe(2);
    for (const id of ids) expect(id).toMatch(/^OpenbookDeploy/);
  });

  test('the new limits default as documented and context overrides them', () => {
    const env = (s: Synth): Record<string, string> =>
      (
        Object.values(s.json.Resources).find(
          (r) => r.Type === 'AWS::Lambda::Function' && r.Properties?.Handler === 'bootstrap',
        )?.Properties?.Environment as { Variables: Record<string, string> }
      ).Variables;
    expect(env(synth({ limitDailyTokensGlobal: '1000', limitFailedJoinsPerDay: 3 })).OPENBOOK_LIMIT_DAILY_TOKENS_GLOBAL).toBe('1000');
    expect(env(synth({ limitDailyTokensGlobal: '1000', limitFailedJoinsPerDay: 3 })).OPENBOOK_LIMIT_FAILED_JOINS_PER_DAY).toBe('3');
    expect(() => synth({ limitFailedJoinsPerDay: '0' })).toThrow(/positive integer/);
  });
});

describe('an account-agnostic synth, as the CI build job runs it', () => {
  test('the prod template has no account number and still names every resource through pseudo parameters', () => {
    const app = newApp({ ...cdkJsonContext, stage: 'prod', apiAssetDir: fixtures.api, ingestAssetDir: fixtures.ingest, webAssetDir: fixtures.web });
    const config = resolveConfig(app.node);
    const stack = new OpenbookStack(app, 'Openbook-prod', {
      config,
      env: { region: 'ap-southeast-2' },
      synthesizer: deploymentSynthesizer('prod'),
    });
    const text = JSON.stringify(app.synth().getStackArtifact(stack.artifactId).template);
    expect(text).not.toMatch(/\b\d{12}\b/);
    expect(text).toContain('AWS::AccountId');
    expect(text).toContain(':policy/OpenbookProdPermissionsBoundary');
  });
});

describe('the least-privilege grants of the execution and deploy roles', () => {
  type Statement = {
    Sid: string;
    Effect: string;
    Action: string | string[];
    Resource: string | string[];
    Condition?: Record<string, Record<string, unknown>>;
  };
  const exec = rendered('cloudformation-execution-role-policy').Statement as Statement[];
  const deployDoc = rendered('github-deploy-role-policy').Statement as Statement[];
  const withAction = (list: Statement[], action: string): Statement[] =>
    list.filter((s) => asArray(s.Action).includes(action));

  test('AddPermission only with the CloudFront principal', () => {
    const grants = withAction(exec, 'lambda:AddPermission');
    expect(grants).toHaveLength(1);
    expect(grants[0].Condition).toEqual({ StringEquals: { 'lambda:Principal': 'cloudfront.amazonaws.com' } });
  });

  test('the event source mapping: create and update only for the stack functions, get and delete without the condition', () => {
    const conditioned = withAction(exec, 'lambda:CreateEventSourceMapping');
    expect(conditioned).toHaveLength(1);
    expect(asArray(conditioned[0].Action).sort()).toEqual(['lambda:CreateEventSourceMapping', 'lambda:UpdateEventSourceMapping']);
    expect(conditioned[0].Condition).toEqual({
      ArnLike: { 'lambda:FunctionArn': `arn:aws:lambda:ap-southeast-2:${fakeAccount}:function:Openbook-prod-*` },
    });
    // AWS does not support the FunctionArn condition on these two. Denying them would stop
    // CloudFormation from reading the mapping after it creates it, and roll the stack back.
    const plain = withAction(exec, 'lambda:GetEventSourceMapping');
    expect(plain).toHaveLength(1);
    expect(asArray(plain[0].Action).sort()).toEqual(['lambda:DeleteEventSourceMapping', 'lambda:GetEventSourceMapping']);
    expect(plain[0]).not.toHaveProperty('Condition');
    expect(plain[0].Resource).toBe(`arn:aws:lambda:ap-southeast-2:${fakeAccount}:event-source-mapping:*`);
  });

  test('function URLs are created and updated only with AWS_IAM auth, so none can be public', () => {
    for (const action of ['lambda:CreateFunctionUrlConfig', 'lambda:UpdateFunctionUrlConfig']) {
      const grants = withAction(exec, action);
      expect(grants, action).toHaveLength(1);
      expect(grants[0].Condition).toEqual({ StringEquals: { 'lambda:FunctionUrlAuthType': 'AWS_IAM' } });
    }
  });

  test('the user pool grants are limited to resources that CloudFormation tagged for this stack', () => {
    const grant = exec.find((s) => s.Sid === 'UserPoolAndClient');
    expect(grant?.Condition).toEqual({
      StringEquals: { 'aws:ResourceTag/aws:cloudformation:stack-name': 'Openbook-prod' },
    });
    // CreateUserPool takes no resource, so it cannot carry that condition. It is alone in its statement.
    expect(asArray(exec.find((s) => s.Sid === 'UserPoolCreateTakesNoResource')?.Action)).toEqual(['cognito-idp:CreateUserPool']);
  });

  test('the deploy role cannot import resources into the stack, delete it, or lose its protection by accident', () => {
    const deny = deployDoc.find((s) => s.Effect === 'Deny');
    expect(deny).toMatchObject({
      Action: 'cloudformation:CreateChangeSet',
      Condition: { StringEquals: { 'cloudformation:ChangeSetType': 'IMPORT' } },
    });
    expect(deployDoc.filter((s) => s.Effect === 'Deny')).toHaveLength(1);
    const allowed = deployDoc.filter((s) => s.Effect === 'Allow').flatMap((s) => asArray(s.Action));
    expect(allowed).not.toContain('cloudformation:DeleteStack');
    // The CDK CLI sets termination protection on the first deploy of a stack that has it.
    expect(allowed).toContain('cloudformation:UpdateTerminationProtection');
  });
});

describe('the boundary follows the catalog and the real API calls', () => {
  const boundary = rendered('permissions-boundary-policy').Statement as {
    Sid: string;
    Action: string[];
    Resource: string[];
  }[];
  const models = cdkJsonContext.models as Parameters<typeof bedrockResourceArns>[0];

  test('the boundary Bedrock resources are exactly the catalog ARNs, and equal to the stack policy', () => {
    const resources = boundary.find((s) => s.Sid === 'InvokeBedrockModels')?.Resource ?? [];
    expect([...resources].sort()).toEqual(
      [...bedrockResourceArns(models, fakeAccount, 'aws', cdkJsonContext.embeddingModelId as string)].sort(),
    );
    const stackBedrock = stackStatements(prod)
      .filter((x) => x.statement.Action.some((a) => a.startsWith('bedrock:')))
      .flatMap((x) => x.statement.Resource);
    expect([...resources].sort()).toEqual([...new Set(stackBedrock)].sort());
    for (const r of resources) expect(r).not.toContain('*');
  });

  test('the boundary has exactly the DynamoDB actions that the stack role has, and none that the API never calls', () => {
    const boundaryActions = boundary.find((s) => s.Sid === 'UseTheApplicationTable')?.Action ?? [];
    const stackActions = stackStatements(prod)
      .filter((x) => x.statement.Action.some((a) => a.startsWith('dynamodb:')))
      .flatMap((x) => x.statement.Action);
    expect([...boundaryActions].sort()).toEqual([...new Set(stackActions)].sort());
    for (const never of ['dynamodb:Scan', 'dynamodb:DescribeTable', 'dynamodb:ConditionCheckItem']) {
      expect(boundaryActions).not.toContain(never);
    }
    // The calls that crates/core/src/store.rs makes. A transaction needs Put, Update, and Delete.
    expect([...new Set(stackActions)].sort()).toEqual([
      'dynamodb:BatchGetItem',
      'dynamodb:BatchWriteItem',
      'dynamodb:DeleteItem',
      'dynamodb:GetItem',
      'dynamodb:PutItem',
      'dynamodb:Query',
      'dynamodb:UpdateItem',
    ]);
  });

  test('the renderer refuses ${BEDROCK_RESOURCES} anywhere but as a whole JSON string', () => {
    expect(() => renderIamDocument('{"Resource": ["x${BEDROCK_RESOURCES}"]}', values)).toThrow(/whole JSON string/);
  });
});

describe('the spend caps of the stages', () => {
  const env = (s: Synth): Record<string, string> =>
    (
      Object.values(s.json.Resources).find(
        (r) => r.Type === 'AWS::Lambda::Function' && r.Properties?.Handler === 'bootstrap',
      )?.Properties?.Environment as { Variables: Record<string, string> }
    ).Variables;

  test('prod and dev have the pre-launch caps from cdk.json', () => {
    expect(env(prod).OPENBOOK_LIMIT_DAILY_MESSAGES_GLOBAL).toBe('300');
    expect(env(prod).OPENBOOK_LIMIT_DAILY_TOKENS_GLOBAL).toBe('1500000');
    expect(env(dev).OPENBOOK_LIMIT_DAILY_MESSAGES_GLOBAL).toBe('100');
    expect(env(dev).OPENBOOK_LIMIT_DAILY_TOKENS_GLOBAL).toBe('500000');
  });

  test('a context key overrides the stage default, for the launch', () => {
    const launched = synth({ stage: 'prod', limitDailyMessagesGlobal: '500', limitDailyTokensGlobal: '3000000' });
    expect(env(launched).OPENBOOK_LIMIT_DAILY_MESSAGES_GLOBAL).toBe('500');
    expect(env(launched).OPENBOOK_LIMIT_DAILY_TOKENS_GLOBAL).toBe('3000000');
  });
});
