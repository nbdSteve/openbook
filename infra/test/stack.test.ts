import * as fs from 'fs';
import * as path from 'path';
import { Match } from 'aws-cdk-lib/assertions';
import { describe, expect, test } from 'vitest';
import { cdkJsonContext, synth, type Json } from './helpers';

const dev = synth({ stage: 'dev' });
const prod = synth({ stage: 'prod' });

/** Resource types that cost money while idle (docs/architecture.md, hard rule 1). */
const baselineCostTypes = [
  /^AWS::OpenSearchServerless::/,
  /^AWS::OpenSearchService::Domain$/,
  /^AWS::EC2::NatGateway$/,
  /^AWS::EC2::VPC$/,
  /^AWS::RDS::/,
  /^AWS::ElastiCache::/,
  /^AWS::WAFv2::WebACL$/,
  /^AWS::SecretsManager::Secret$/,
  /^AWS::KMS::Key$/,
  /^AWS::ECS::Service$/,
  /^AWS::ElasticLoadBalancingV2::LoadBalancer$/,
];

describe.each([
  ['dev', dev],
  ['prod', prod],
])('the %s stack', (_stage, s) => {
  test('has no resource with a baseline monthly cost', () => {
    const types = Object.values(s.json.Resources).map((r) => r.Type);
    const offenders = types.filter((t) => baselineCostTypes.some((re) => re.test(t)));
    expect(offenders).toEqual([]);
  });

  test('the function URL uses AWS_IAM and response streaming', () => {
    s.template.hasResourceProperties('AWS::Lambda::Url', {
      AuthType: 'AWS_IAM',
      InvokeMode: 'RESPONSE_STREAM',
    });
    s.template.resourceCountIs('AWS::Lambda::Url', 1);
  });

  test('the table has the keys, index, and TTL of docs/architecture.md', () => {
    s.template.hasResourceProperties('AWS::DynamoDB::Table', {
      BillingMode: 'PAY_PER_REQUEST',
      KeySchema: [
        { AttributeName: 'pk', KeyType: 'HASH' },
        { AttributeName: 'sk', KeyType: 'RANGE' },
      ],
      AttributeDefinitions: Match.arrayEquals([
        { AttributeName: 'pk', AttributeType: 'S' },
        { AttributeName: 'sk', AttributeType: 'S' },
        { AttributeName: 'gsi1pk', AttributeType: 'S' },
        { AttributeName: 'gsi1sk', AttributeType: 'S' },
      ]),
      GlobalSecondaryIndexes: [
        {
          IndexName: 'gsi1',
          KeySchema: [
            { AttributeName: 'gsi1pk', KeyType: 'HASH' },
            { AttributeName: 'gsi1sk', KeyType: 'RANGE' },
          ],
          Projection: { ProjectionType: 'ALL' },
        },
      ],
      TimeToLiveSpecification: { AttributeName: 'ttl', Enabled: true },
    });
  });

  test('both buckets block public access, encrypt, and require TLS', () => {
    const buckets = Object.entries(s.json.Resources).filter(([, r]) => r.Type === 'AWS::S3::Bucket');
    expect(buckets).toHaveLength(2);
    for (const [, bucket] of buckets) {
      expect(bucket.Properties?.PublicAccessBlockConfiguration).toEqual({
        BlockPublicAcls: true,
        BlockPublicPolicy: true,
        IgnorePublicAcls: true,
        RestrictPublicBuckets: true,
      });
      expect(bucket.Properties?.BucketEncryption).toEqual({
        ServerSideEncryptionConfiguration: [{ ServerSideEncryptionByDefault: { SSEAlgorithm: 'AES256' } }],
      });
    }
    const policies = Object.values(s.json.Resources).filter((r) => r.Type === 'AWS::S3::BucketPolicy');
    expect(policies).toHaveLength(2);
    for (const policy of policies) {
      const statements = (policy.Properties?.PolicyDocument as { Statement: Json[] }).Statement;
      expect(statements).toContainEqual(
        expect.objectContaining({
          Effect: 'Deny',
          Principal: { AWS: '*' },
          Condition: { Bool: { 'aws:SecureTransport': 'false' } },
        }),
      );
    }
  });

  test('the docs bucket aborts incomplete multipart uploads after one day', () => {
    s.template.hasResourceProperties('AWS::S3::Bucket', {
      LifecycleConfiguration: {
        Rules: [{ AbortIncompleteMultipartUpload: { DaysAfterInitiation: 1 }, Status: 'Enabled' }],
      },
    });
  });

  test('the docs bucket CORS allows only PUT from the site origins', () => {
    const docs = Object.values(s.json.Resources).find(
      (r) => r.Type === 'AWS::S3::Bucket' && r.Properties?.CorsConfiguration,
    );
    const rules = (docs?.Properties?.CorsConfiguration as { CorsRules: Json[] }).CorsRules;
    expect(rules).toHaveLength(1);
    expect(rules[0]).toMatchObject({
      AllowedMethods: ['PUT'],
      AllowedHeaders: ['content-type', 'content-length', 'x-amz-*'],
      ExposedHeaders: ['ETag'],
    });
  });

  test('the API function is arm64 provided.al2023 with the documented sizing', () => {
    s.template.hasResourceProperties('AWS::Lambda::Function', {
      Runtime: 'provided.al2023',
      Architectures: ['arm64'],
      Handler: 'bootstrap',
      MemorySize: 512,
      Timeout: 120,
    });
  });

  test('the function environment has no local-only variable and uses Bedrock', () => {
    const fn = Object.values(s.json.Resources).find(
      (r) => r.Type === 'AWS::Lambda::Function' && r.Properties?.Handler === 'bootstrap',
    );
    const env = (fn?.Properties?.Environment as { Variables: Record<string, unknown> }).Variables;
    // No AWS_ENDPOINT_URL, and none of the per-service AWS_ENDPOINT_URL_<SERVICE> variants.
    expect(Object.keys(env).filter((name) => /^AWS_ENDPOINT_URL/.test(name))).toEqual([]);
    for (const name of [
      'AWS_ENDPOINT_URL',
      'OPENBOOK_COGNITO_JWKS_URL',
      'OPENBOOK_CA_BUNDLE',
      'OPENBOOK_PRESIGN_ENDPOINT',
      'OPENBOOK_BEDROCK_PROFILE',
    ]) {
      expect(env).not.toHaveProperty(name);
    }
    expect(env).not.toHaveProperty('OPENBOOK_MODEL_PROVIDER');
    expect(env).not.toHaveProperty('OPENBOOK_CHAT_MODEL_ID');
    expect(env).not.toHaveProperty('OPENBOOK_FAST_MODEL_ID');
    expect(env.OPENBOOK_STAGE).toBe(_stage);
    expect(env.OPENBOOK_DEFAULT_CHAT_MODEL).toBe('claude-sonnet');
    expect(env.OPENBOOK_FAST_MODEL).toBe('claude-haiku');
    // The catalog is the one in cdk.json, and every entry is a Bedrock entry (no fake provider).
    const catalog = JSON.parse(env.OPENBOOK_MODELS as string) as { id: string; provider: string }[];
    expect(catalog).toEqual(cdkJsonContext.models);
    expect(new Set(catalog.map((m) => m.provider))).toEqual(new Set(['bedrock']));
    // Lambda allows 4 KB of environment variables in total.
    expect(JSON.stringify(env).length).toBeLessThan(4096);
    expect(env.OPENBOOK_LIMIT_DAILY_MESSAGES_PER_USER).toBe('50');
    // The spend caps until the public launch (stageLimits in cdk.json), well below the API defaults.
    expect(env.OPENBOOK_LIMIT_DAILY_MESSAGES_GLOBAL).toBe(_stage === 'prod' ? '300' : '100');
    expect(env.OPENBOOK_LIMIT_DAILY_TOKENS_GLOBAL).toBe(_stage === 'prod' ? '300000' : '100000');
    expect(env.OPENBOOK_LIMIT_FAILED_JOINS_PER_DAY).toBe('20');
    expect(env.OPENBOOK_LIMIT_COURSES_OWNED).toBe('10');
    expect(env.OPENBOOK_LIMIT_DOCUMENTS_PER_COURSE).toBe('50');
    expect(env.OPENBOOK_LIMIT_UPLOAD_BYTES).toBe('26214400');
    expect(env.RUST_LOG).toBe('info');
    for (const name of [
      'OPENBOOK_TABLE_NAME',
      'OPENBOOK_DOCS_BUCKET',
      'OPENBOOK_COGNITO_USER_POOL_ID',
      'OPENBOOK_COGNITO_CLIENT_ID',
    ]) {
      expect(env).toHaveProperty(name);
    }
  });

  test('the /api/* behavior never caches and signs requests with OAC', () => {
    const dist = Object.values(s.json.Resources).find((r) => r.Type === 'AWS::CloudFront::Distribution');
    const config = dist?.Properties?.DistributionConfig as {
      CacheBehaviors: Json[];
      Origins: Json[];
      PriceClass: string;
      HttpVersion: string;
      DefaultCacheBehavior: Json;
    };
    expect(config.PriceClass).toBe('PriceClass_All');
    expect(config.HttpVersion).toBe('http2and3');
    expect(config.CacheBehaviors).toHaveLength(1);
    const api = config.CacheBehaviors[0];
    expect(api.PathPattern).toBe('/api/*');
    // Managed policy IDs: CachingDisabled, and AllViewerExceptHostHeader.
    expect(api.CachePolicyId).toBe('4135ea2d-6df8-44a3-9df3-4b5a84be39ad');
    expect(api.OriginRequestPolicyId).toBe('b689b0a8-53d0-40ab-baf2-68738e2966ac');
    expect(api.ViewerProtocolPolicy).toBe('https-only');
    expect(api.AllowedMethods).toEqual(['GET', 'HEAD', 'OPTIONS', 'PUT', 'PATCH', 'POST', 'DELETE']);

    const apiOrigin = config.Origins.find((o) => o.Id === api.TargetOriginId) as {
      OriginAccessControlId: unknown;
      CustomOriginConfig: { OriginProtocolPolicy: string; OriginReadTimeout: number };
    };
    expect(apiOrigin.OriginAccessControlId).toBeDefined();
    expect(apiOrigin.CustomOriginConfig.OriginProtocolPolicy).toBe('https-only');
    expect(apiOrigin.CustomOriginConfig.OriginReadTimeout).toBe(60);

    const oacs = Object.values(s.json.Resources)
      .filter((r) => r.Type === 'AWS::CloudFront::OriginAccessControl')
      .map((r) => (r.Properties?.OriginAccessControlConfig as Json).OriginAccessControlOriginType)
      .sort();
    expect(oacs).toEqual(['lambda', 's3']);
    for (const r of Object.values(s.json.Resources)) {
      if (r.Type !== 'AWS::CloudFront::OriginAccessControl') continue;
      expect(r.Properties?.OriginAccessControlConfig).toMatchObject({
        SigningBehavior: 'always',
        SigningProtocol: 'sigv4',
      });
    }
  });

  test('the default behavior redirects to HTTPS, compresses, and rewrites SPA routes', () => {
    const dist = Object.values(s.json.Resources).find((r) => r.Type === 'AWS::CloudFront::Distribution');
    const behavior = (dist?.Properties?.DistributionConfig as { DefaultCacheBehavior: Json }).DefaultCacheBehavior;
    expect(behavior.ViewerProtocolPolicy).toBe('redirect-to-https');
    expect(behavior.Compress).toBe(true);
    // Managed policy ID: CachingOptimized.
    expect(behavior.CachePolicyId).toBe('658327ea-f89d-4fab-a63d-7e88639e58f6');
    expect(behavior.FunctionAssociations).toEqual([
      { EventType: 'viewer-request', FunctionARN: expect.anything() },
    ]);
  });

  test('CloudFront may invoke the function URL with both permissions the docs require', () => {
    const permissions = Object.values(s.json.Resources)
      .filter((r) => r.Type === 'AWS::Lambda::Permission')
      .map((r) => r.Properties as Json);
    expect(permissions.map((p) => p.Action).sort()).toEqual(['lambda:InvokeFunction', 'lambda:InvokeFunctionUrl']);
    for (const p of permissions) {
      expect(p.Principal).toBe('cloudfront.amazonaws.com');
      expect(p.SourceArn).toBeDefined();
    }
  });

  test('security headers include a strict CSP', () => {
    const policy = Object.values(s.json.Resources).find((r) => r.Type === 'AWS::CloudFront::ResponseHeadersPolicy');
    const headers = (policy?.Properties?.ResponseHeadersPolicyConfig as { SecurityHeadersConfig: Json })
      .SecurityHeadersConfig as {
      ContentSecurityPolicy: { ContentSecurityPolicy: string };
      FrameOptions: Json;
      ContentTypeOptions: Json;
      StrictTransportSecurity: Json;
      ReferrerPolicy: Json;
    };
    const csp = headers.ContentSecurityPolicy.ContentSecurityPolicy;
    expect(csp).toContain(`default-src 'self'`);
    expect(csp).toContain(`script-src 'self';`);
    expect(csp).toContain(`style-src 'self' 'unsafe-inline'`);
    expect(csp).toContain(`img-src 'self' data: blob:`);
    expect(csp).toContain(`font-src 'self' data:`);
    expect(csp).toContain(
      `connect-src 'self' https://cognito-idp.ap-southeast-2.amazonaws.com https://openbook-${_stage}-docs-111111111111-ap-southeast-2.s3.ap-southeast-2.amazonaws.com`,
    );
    expect(csp).toContain(`object-src 'none'`);
    expect(csp).toContain(`frame-ancestors 'none'`);
    expect(csp).toContain(`base-uri 'self'`);
    expect(csp).toContain(`form-action 'self'`);
    expect(csp).not.toContain(`'unsafe-eval'`);
    expect(headers.FrameOptions).toEqual({ FrameOption: 'DENY', Override: true });
    expect(headers.ContentTypeOptions).toEqual({ Override: true });
    expect(headers.StrictTransportSecurity).toMatchObject({ Override: true, AccessControlMaxAgeSec: 31536000 });
    expect(headers.ReferrerPolicy).toMatchObject({ ReferrerPolicy: 'strict-origin-when-cross-origin' });
  });

  test('Cognito uses the Lite plan with the documented sign-in rules', () => {
    s.template.hasResourceProperties('AWS::Cognito::UserPool', {
      UserPoolTier: 'LITE',
      UsernameAttributes: ['email'],
      AutoVerifiedAttributes: ['email'],
      AdminCreateUserConfig: { AllowAdminCreateUserOnly: false },
      AccountRecoverySetting: { RecoveryMechanisms: [{ Name: 'verified_email', Priority: 1 }] },
      Policies: {
        PasswordPolicy: {
          MinimumLength: 8,
          RequireLowercase: true,
          RequireUppercase: true,
          RequireNumbers: true,
          RequireSymbols: false,
        },
      },
    });
    s.template.hasResourceProperties('AWS::Cognito::UserPoolClient', {
      GenerateSecret: false,
      PreventUserExistenceErrors: 'ENABLED',
      IdTokenValidity: 60,
      RefreshTokenValidity: 43200,
      TokenValidityUnits: Match.objectLike({ IdToken: 'minutes', RefreshToken: 'minutes' }),
    });
    // One sign-in flow in every stage (docs/api.md): USER_PASSWORD_AUTH and refresh, no SRP,
    // and no hosted-UI OAuth.
    const client = Object.values(s.json.Resources).find((r) => r.Type === 'AWS::Cognito::UserPoolClient');
    expect([...(client?.Properties?.ExplicitAuthFlows as string[])].sort()).toEqual([
      'ALLOW_REFRESH_TOKEN_AUTH',
      'ALLOW_USER_PASSWORD_AUTH',
    ]);
    expect(client?.Properties).not.toHaveProperty('AllowedOAuthFlows');
    // An unverified new email must not replace the verified one.
    s.template.hasResourceProperties('AWS::Cognito::UserPool', {
      UserAttributeUpdateSettings: { AttributesRequireVerificationBeforeUpdate: ['email'] },
    });
  });

  test('the web deployments never prune; only assets/ gets the immutable cache', () => {
    const deployments = Object.values(s.json.Resources).filter((r) => r.Type === 'Custom::CDKBucketDeployment');
    expect(deployments).toHaveLength(2);
    for (const d of deployments) expect(d.Properties?.Prune).toBe(false);

    const immutable = deployments.filter(
      (d) => (d.Properties?.SystemMetadata as Json)['cache-control'] === 'public, max-age=31536000, immutable',
    );
    const noCache = deployments.filter((d) => (d.Properties?.SystemMetadata as Json)['cache-control'] === 'no-cache');
    expect(immutable).toHaveLength(1);
    expect(noCache).toHaveLength(1);
    expect(immutable[0].Properties).not.toHaveProperty('DistributionId');
    expect(noCache[0].Properties?.DistributionPaths).toEqual(['/index.html', '/config.json']);

    // What each deployment uploads: the immutable one has the hashed assets and nothing else.
    const dir = s.app.synth().directory;
    const filesOf = (d: Json | undefined): string[] =>
      (d?.SourceObjectKeys as string[]).flatMap((key) => {
        const root = path.join(dir, `asset.${key.replace(/^.*\//, '').replace(/\.zip$/, '')}`);
        const walk = (p: string): string[] =>
          fs.readdirSync(p, { withFileTypes: true }).flatMap((e) =>
            e.isDirectory() ? walk(path.join(p, e.name)) : [path.relative(root, path.join(p, e.name))],
          );
        return walk(root);
      });
    expect(filesOf(immutable[0].Properties).sort()).toEqual(['assets/app-abc123.js']);
    // Everything else (the favicon too) is `no-cache`, and assets/ is not uploaded twice.
    expect(filesOf(noCache[0].Properties).sort()).toEqual(['config.json', 'favicon.ico', 'index.html']);
  });

  test('config.json has the shape of docs/api.md with a null Cognito endpoint', () => {
    const entry = Object.values(s.json.Resources).find(
      (r) => r.Type === 'Custom::CDKBucketDeployment' && r.Properties?.SourceMarkers,
    );
    const markers = (entry?.Properties?.SourceMarkers as Json[])[1];
    expect(Object.keys(markers)).toHaveLength(2);
    // The generated source is inline JSON in the asset zip; check the asset content.
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    const fs = require('fs') as typeof import('fs');
    const assemblyDir = s.app.synth().directory;
    const sources = fs
      .readdirSync(assemblyDir)
      .filter((d) => d.startsWith('asset.'))
      .map((d) => `${assemblyDir}/${d}/config.json`)
      .filter((f) => fs.existsSync(f))
      .map((f) => fs.readFileSync(f, 'utf8'));
    const generated = sources.find((t) => t.includes('cognito'));
    expect(generated).toBeDefined();
    const parsed = JSON.parse(
      (generated as string).replace(/<<marker:0xbaba:\d+>>/g, 'x'),
    ) as Json;
    expect(parsed).toEqual({
      stage: _stage,
      region: 'ap-southeast-2',
      cognito: { userPoolId: 'x', clientId: 'x', endpoint: null },
    });
  });

  test('the outputs are the documented set', () => {
    const outputs = Object.keys((s.json as unknown as { Outputs: Json }).Outputs).sort();
    expect(outputs).toEqual(
      ['ApiFunctionName', 'DistributionId', 'DocsBucketName', 'SiteUrl', 'TableName', 'UserPoolClientId', 'UserPoolId'].sort(),
    );
  });
});

describe('stage differences', () => {
  test('prod retains the table, buckets, and user pool, with PITR and deletion protection', () => {
    prod.template.hasResource('AWS::DynamoDB::Table', {
      DeletionPolicy: 'Retain',
      UpdateReplacePolicy: 'Retain',
      Properties: {
        DeletionProtectionEnabled: true,
        PointInTimeRecoverySpecification: { PointInTimeRecoveryEnabled: true },
      },
    });
    const buckets = Object.values(prod.json.Resources).filter((r) => r.Type === 'AWS::S3::Bucket');
    for (const b of buckets) expect(b.DeletionPolicy).toBe('Retain');
    prod.template.hasResource('AWS::Cognito::UserPool', {
      DeletionPolicy: 'Retain',
      Properties: { DeletionProtection: 'ACTIVE' },
    });
    expect(Object.values(prod.json.Resources).some((r) => r.Type === 'Custom::S3AutoDeleteObjects')).toBe(false);
  });

  test('prod enables termination protection', () => {
    expect(prod.stack.terminationProtection).toBe(true);
    expect(dev.stack.terminationProtection).toBe(false);
  });

  test('dev destroys the table and buckets, and auto-deletes bucket objects', () => {
    dev.template.hasResource('AWS::DynamoDB::Table', { DeletionPolicy: 'Delete' });
    const buckets = Object.values(dev.json.Resources).filter((r) => r.Type === 'AWS::S3::Bucket');
    for (const b of buckets) expect(b.DeletionPolicy).toBe('Delete');
    expect(
      Object.values(dev.json.Resources).filter((r) => r.Type === 'Custom::S3AutoDeleteObjects'),
    ).toHaveLength(2);
    dev.template.hasResourceProperties('AWS::DynamoDB::Table', {
      PointInTimeRecoverySpecification: { PointInTimeRecoveryEnabled: false },
    });
  });

  test('reserved concurrency and log retention follow the stage', () => {
    prod.template.hasResourceProperties('AWS::Lambda::Function', {
      Handler: 'bootstrap',
      ReservedConcurrentExecutions: 25,
    });
    dev.template.hasResourceProperties('AWS::Lambda::Function', {
      Handler: 'bootstrap',
      ReservedConcurrentExecutions: 5,
    });
    prod.template.hasResourceProperties('AWS::Logs::LogGroup', { RetentionInDays: 30 });
    dev.template.hasResourceProperties('AWS::Logs::LogGroup', { RetentionInDays: 7 });
  });

  test('the function logs to its explicit log group', () => {
    dev.template.hasResourceProperties('AWS::Lambda::Function', {
      Handler: 'bootstrap',
      LoggingConfig: { LogGroup: { Ref: Match.stringLikeRegexp('^ApiLogGroup') } },
    });
  });

  test('no stage lets a localhost origin upload to the docs bucket (local dev uses the Vite proxy)', () => {
    for (const stage of [dev, prod]) {
      const docs = Object.values(stage.json.Resources).find(
        (r) => r.Type === 'AWS::S3::Bucket' && r.Properties?.CorsConfiguration,
      );
      const origins = (docs?.Properties?.CorsConfiguration as { CorsRules: { AllowedOrigins: unknown[] }[] }).CorsRules[0]
        .AllowedOrigins;
      expect(JSON.stringify(origins)).not.toContain('localhost');
      expect(JSON.stringify(origins)).toContain('"DomainName"');
    }
  });
});

describe('configuration', () => {
  test('limits can be overridden by context', () => {
    const s = synth({ stage: 'dev', limitDailyMessagesPerUser: '7', limitUploadBytes: '1000' });
    const fn = Object.values(s.json.Resources).find(
      (r) => r.Type === 'AWS::Lambda::Function' && r.Properties?.Handler === 'bootstrap',
    );
    const env = (fn?.Properties?.Environment as { Variables: Record<string, string> }).Variables;
    expect(env.OPENBOOK_LIMIT_DAILY_MESSAGES_PER_USER).toBe('7');
    expect(env.OPENBOOK_LIMIT_UPLOAD_BYTES).toBe('1000');
  });

  test('refuses an unknown stage and bad limits', () => {
    expect(() => synth({ stage: 'staging' })).toThrow(/stage/);
    expect(() => synth({ limitCoursesOwned: '0' })).toThrow(/positive integer/);
  });
});

describe('custom domain', () => {
  const ctx = { stage: 'prod', domainName: 'ask.example.test', hostedZoneName: 'example.test', hostedZoneId: 'Z0000000000000000000' };
  const s = synth(ctx);

  test('the certificate lives in us-east-1 with DNS validation', () => {
    expect(s.certStack?.region).toBe('us-east-1');
    const certTemplate = JSON.stringify(s.app.synth().getStackByName('OpenbookCert-prod').template);
    expect(certTemplate).toContain('AWS::CertificateManager::Certificate');
    expect(certTemplate).toContain('ask.example.test');
  });

  test('the distribution serves the domain and Route 53 aliases it', () => {
    s.template.hasResourceProperties('AWS::CloudFront::Distribution', {
      DistributionConfig: Match.objectLike({ Aliases: ['ask.example.test'] }),
    });
    s.template.resourceCountIs('AWS::Route53::RecordSet', 2);
    s.template.hasResourceProperties('AWS::Route53::RecordSet', { Name: 'ask.example.test.', Type: 'A' });
    s.template.hasResourceProperties('AWS::Route53::RecordSet', { Name: 'ask.example.test.', Type: 'AAAA' });
    const outputs = (s.json as unknown as { Outputs: { SiteUrl: { Value: string } } }).Outputs;
    expect(outputs.SiteUrl.Value).toBe('https://ask.example.test');
  });

  test('the docs bucket CORS allows the custom domain', () => {
    expect(JSON.stringify(s.json)).toContain('https://ask.example.test');
  });

  test('a partial domain configuration is refused', () => {
    expect(() => synth({ domainName: 'ask.example.test' })).toThrow(/together/);
  });

  test('a domain outside the hosted zone is refused', () => {
    expect(() => synth({ ...ctx, domainName: 'ask.other.test' })).toThrow(/subdomain/);
  });
});
