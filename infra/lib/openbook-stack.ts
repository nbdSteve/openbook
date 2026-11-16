import * as cdk from 'aws-cdk-lib';
import * as acm from 'aws-cdk-lib/aws-certificatemanager';
import * as cloudfront from 'aws-cdk-lib/aws-cloudfront';
import * as origins from 'aws-cdk-lib/aws-cloudfront-origins';
import * as cognito from 'aws-cdk-lib/aws-cognito';
import * as dynamodb from 'aws-cdk-lib/aws-dynamodb';
import * as iam from 'aws-cdk-lib/aws-iam';
import * as lambda from 'aws-cdk-lib/aws-lambda';
import * as logs from 'aws-cdk-lib/aws-logs';
import * as route53 from 'aws-cdk-lib/aws-route53';
import * as targets from 'aws-cdk-lib/aws-route53-targets';
import * as s3 from 'aws-cdk-lib/aws-s3';
import * as s3deploy from 'aws-cdk-lib/aws-s3-deployment';
import type { Construct } from 'constructs';
import { region, type OpenbookConfig } from './config';
import { spaRewriteCode } from './spa-rewrite';

export interface OpenbookStackProps extends cdk.StackProps {
  config: OpenbookConfig;
  /** The us-east-1 certificate, when a custom domain is configured. */
  certificate?: acm.ICertificate;
}

/** The local dev origin that the docs bucket CORS allows in dev only. */
export const devWebOrigin = 'http://localhost:5173';

/** The one-year cache for hashed, immutable build assets. */
export const immutableCacheControl = 'public, max-age=31536000, immutable';

/**
 * The Openbook stack: Cognito, DynamoDB, S3, the API Lambda, and CloudFront.
 * Every resource is pay per use (docs/architecture.md, hard rule 1).
 */
export class OpenbookStack extends cdk.Stack {
  constructor(scope: Construct, id: string, props: OpenbookStackProps) {
    super(scope, id, props);
    const { config } = props;
    const isProd = config.stage === 'prod';
    const removalPolicy = isProd ? cdk.RemovalPolicy.RETAIN : cdk.RemovalPolicy.DESTROY;

    if (config.domain && !props.certificate) {
      throw new Error('a custom domain needs a certificate');
    }

    // ---- Cognito -------------------------------------------------------------------
    const userPool = new cognito.UserPool(this, 'UserPool', {
      featurePlan: cognito.FeaturePlan.LITE,
      selfSignUpEnabled: true,
      signInAliases: { email: true },
      signInCaseSensitive: false,
      autoVerify: { email: true },
      standardAttributes: { email: { required: true, mutable: true } },
      accountRecovery: cognito.AccountRecovery.EMAIL_ONLY,
      passwordPolicy: {
        minLength: 8,
        requireLowercase: true,
        requireUppercase: true,
        requireDigits: true,
        requireSymbols: false,
      },
      mfa: cognito.Mfa.OFF,
      deletionProtection: isProd,
      removalPolicy,
    });
    const userPoolClient = userPool.addClient('WebClient', {
      generateSecret: false,
      disableOAuth: true,
      authFlows: { userSrp: true, userPassword: true },
      preventUserExistenceErrors: true,
      idTokenValidity: cdk.Duration.hours(1),
      accessTokenValidity: cdk.Duration.hours(1),
      refreshTokenValidity: cdk.Duration.days(30),
    });

    // ---- DynamoDB ------------------------------------------------------------------
    const table = new dynamodb.Table(this, 'Table', {
      partitionKey: { name: 'pk', type: dynamodb.AttributeType.STRING },
      sortKey: { name: 'sk', type: dynamodb.AttributeType.STRING },
      billingMode: dynamodb.BillingMode.PAY_PER_REQUEST,
      timeToLiveAttribute: 'ttl',
      pointInTimeRecoverySpecification: { pointInTimeRecoveryEnabled: isProd },
      deletionProtection: isProd,
      removalPolicy,
    });
    table.addGlobalSecondaryIndex({
      indexName: 'gsi1',
      partitionKey: { name: 'gsi1pk', type: dynamodb.AttributeType.STRING },
      sortKey: { name: 'gsi1sk', type: dynamodb.AttributeType.STRING },
      projectionType: dynamodb.ProjectionType.ALL,
    });

    // ---- Docs bucket name ----------------------------------------------------------
    // The name is fixed up front so that nothing needs a CloudFormation reference to the
    // bucket. The bucket CORS rule refers to the distribution domain, and the
    // distribution refers to the function and the CSP, so a reference the other way
    // would be a dependency cycle.
    const docsBucketName = `openbook-${config.stage}-docs-${this.account}-${this.region}`;
    const docsBucketDomain = `${docsBucketName}.s3.${region}.amazonaws.com`;

    // ---- API Lambda ----------------------------------------------------------------
    const apiLogGroup = new logs.LogGroup(this, 'ApiLogGroup', {
      retention: isProd ? logs.RetentionDays.ONE_MONTH : logs.RetentionDays.ONE_WEEK,
      removalPolicy: cdk.RemovalPolicy.DESTROY,
    });
    const apiFunction = new lambda.Function(this, 'ApiFunction', {
      runtime: lambda.Runtime.PROVIDED_AL2023,
      architecture: lambda.Architecture.ARM_64,
      handler: 'bootstrap',
      code: lambda.Code.fromAsset(config.apiAssetDir),
      memorySize: 512,
      timeout: cdk.Duration.seconds(120),
      reservedConcurrentExecutions: isProd ? 25 : 5,
      logGroup: apiLogGroup,
      environment: {
        OPENBOOK_STAGE: config.stage,
        OPENBOOK_TABLE_NAME: table.tableName,
        OPENBOOK_DOCS_BUCKET: docsBucketName,
        OPENBOOK_COGNITO_USER_POOL_ID: userPool.userPoolId,
        OPENBOOK_COGNITO_CLIENT_ID: userPoolClient.userPoolClientId,
        OPENBOOK_MODEL_PROVIDER: 'bedrock',
        OPENBOOK_CHAT_MODEL_ID: config.chatModelId,
        OPENBOOK_FAST_MODEL_ID: config.fastModelId,
        OPENBOOK_LIMIT_DAILY_MESSAGES_PER_USER: String(config.limits.dailyMessagesPerUser),
        OPENBOOK_LIMIT_DAILY_MESSAGES_GLOBAL: String(config.limits.dailyMessagesGlobal),
        OPENBOOK_LIMIT_COURSES_OWNED: String(config.limits.coursesOwned),
        OPENBOOK_LIMIT_DOCUMENTS_PER_COURSE: String(config.limits.documentsPerCourse),
        OPENBOOK_LIMIT_UPLOAD_BYTES: String(config.limits.uploadBytes),
        RUST_LOG: 'info',
      },
    });
    table.grantReadWriteData(apiFunction);
    apiFunction.addToRolePolicy(
      new iam.PolicyStatement({
        sid: 'DocsBucketObjects',
        actions: ['s3:GetObject', 's3:PutObject', 's3:DeleteObject'],
        resources: [`arn:${this.partition}:s3:::${docsBucketName}/*`],
      }),
    );
    apiFunction.addToRolePolicy(
      new iam.PolicyStatement({
        sid: 'DocsBucketList',
        actions: ['s3:ListBucket'],
        resources: [`arn:${this.partition}:s3:::${docsBucketName}`],
      }),
    );
    // The `au.` profiles route to Sydney and Melbourne. The profile ARNs come from the
    // configured model IDs, so a model change updates both the environment and this policy.
    apiFunction.addToRolePolicy(
      new iam.PolicyStatement({
        sid: 'InvokeAuInferenceProfilesAndAnthropicModels',
        actions: ['bedrock:InvokeModel', 'bedrock:InvokeModelWithResponseStream'],
        resources: [
          ...[...new Set([config.chatModelId, config.fastModelId])].map(
            (profile) => `arn:${this.partition}:bedrock:${region}:${this.account}:inference-profile/${profile}`,
          ),
          `arn:${this.partition}:bedrock:ap-southeast-2::foundation-model/anthropic.*`,
          `arn:${this.partition}:bedrock:ap-southeast-4::foundation-model/anthropic.*`,
        ],
      }),
    );

    const functionUrl = apiFunction.addFunctionUrl({
      authType: lambda.FunctionUrlAuthType.AWS_IAM,
      invokeMode: lambda.InvokeMode.RESPONSE_STREAM,
    });

    // ---- Web bucket ----------------------------------------------------------------
    const webBucket = new s3.Bucket(this, 'WebBucket', {
      blockPublicAccess: s3.BlockPublicAccess.BLOCK_ALL,
      encryption: s3.BucketEncryption.S3_MANAGED,
      enforceSSL: true,
      removalPolicy,
      autoDeleteObjects: !isProd,
    });

    // ---- CloudFront ----------------------------------------------------------------
    const csp = [
      `default-src 'self'`,
      `script-src 'self'`,
      // KaTeX and framer-motion need inline styles.
      `style-src 'self' 'unsafe-inline'`,
      `img-src 'self' data: blob:`,
      `font-src 'self' data:`,
      `connect-src 'self' https://cognito-idp.${region}.amazonaws.com https://${docsBucketDomain}`,
      `frame-ancestors 'none'`,
      `base-uri 'self'`,
      `form-action 'self'`,
    ].join('; ');

    const responseHeadersPolicy = new cloudfront.ResponseHeadersPolicy(this, 'SecurityHeaders', {
      securityHeadersBehavior: {
        strictTransportSecurity: {
          accessControlMaxAge: cdk.Duration.days(365),
          includeSubdomains: false,
          preload: false,
          override: true,
        },
        contentTypeOptions: { override: true },
        frameOptions: { frameOption: cloudfront.HeadersFrameOption.DENY, override: true },
        referrerPolicy: {
          referrerPolicy: cloudfront.HeadersReferrerPolicy.STRICT_ORIGIN_WHEN_CROSS_ORIGIN,
          override: true,
        },
        contentSecurityPolicy: { contentSecurityPolicy: csp, override: true },
      },
    });

    const spaRewrite = new cloudfront.Function(this, 'SpaRewrite', {
      runtime: cloudfront.FunctionRuntime.JS_2_0,
      code: cloudfront.FunctionCode.fromInline(spaRewriteCode),
    });

    const distribution = new cloudfront.Distribution(this, 'Distribution', {
      priceClass: cloudfront.PriceClass.PRICE_CLASS_ALL,
      httpVersion: cloudfront.HttpVersion.HTTP2_AND_3,
      ...(config.domain && props.certificate
        ? {
            domainNames: [config.domain.domainName],
            certificate: props.certificate,
            minimumProtocolVersion: cloudfront.SecurityPolicyProtocol.TLS_V1_2_2021,
          }
        : {}),
      defaultBehavior: {
        origin: origins.S3BucketOrigin.withOriginAccessControl(webBucket),
        viewerProtocolPolicy: cloudfront.ViewerProtocolPolicy.REDIRECT_TO_HTTPS,
        cachePolicy: cloudfront.CachePolicy.CACHING_OPTIMIZED,
        compress: true,
        responseHeadersPolicy,
        functionAssociations: [
          { function: spaRewrite, eventType: cloudfront.FunctionEventType.VIEWER_REQUEST },
        ],
      },
      additionalBehaviors: {
        '/api/*': {
          origin: origins.FunctionUrlOrigin.withOriginAccessControl(functionUrl, {
            readTimeout: cdk.Duration.seconds(60),
          }),
          viewerProtocolPolicy: cloudfront.ViewerProtocolPolicy.HTTPS_ONLY,
          allowedMethods: cloudfront.AllowedMethods.ALLOW_ALL,
          cachePolicy: cloudfront.CachePolicy.CACHING_DISABLED,
          originRequestPolicy: cloudfront.OriginRequestPolicy.ALL_VIEWER_EXCEPT_HOST_HEADER,
          // Server-sent events must not be buffered for compression.
          compress: false,
          responseHeadersPolicy,
        },
      },
    });

    // CDK grants CloudFront both `lambda:InvokeFunctionUrl` and `lambda:InvokeFunction`
    // (with `InvokedViaFunctionUrl`) on the function URL, as the "Restrict access to an
    // AWS Lambda function URL origin" page requires. test/stack.test.ts asserts both.

    const siteHost = config.domain ? config.domain.domainName : distribution.distributionDomainName;
    const siteUrl = `https://${siteHost}`;

    // ---- Docs bucket ---------------------------------------------------------------
    const siteOrigins = [`https://${distribution.distributionDomainName}`];
    if (config.domain) siteOrigins.push(`https://${config.domain.domainName}`);
    if (!isProd) siteOrigins.push(devWebOrigin);

    const docsBucket = new s3.Bucket(this, 'DocsBucket', {
      bucketName: docsBucketName,
      blockPublicAccess: s3.BlockPublicAccess.BLOCK_ALL,
      encryption: s3.BucketEncryption.S3_MANAGED,
      enforceSSL: true,
      removalPolicy,
      autoDeleteObjects: !isProd,
      lifecycleRules: [{ abortIncompleteMultipartUploadAfter: cdk.Duration.days(1) }],
      cors: [
        {
          allowedMethods: [s3.HttpMethods.PUT],
          allowedOrigins: siteOrigins,
          allowedHeaders: ['content-type', 'content-length', 'x-amz-*'],
          exposedHeaders: ['ETag'],
          maxAge: 3000,
        },
      ],
    });

    // ---- DNS -----------------------------------------------------------------------
    if (config.domain) {
      const zone = route53.HostedZone.fromHostedZoneAttributes(this, 'Zone', {
        hostedZoneId: config.domain.hostedZoneId,
        zoneName: config.domain.hostedZoneName,
      });
      const target = route53.RecordTarget.fromAlias(new targets.CloudFrontTarget(distribution));
      new route53.ARecord(this, 'AliasA', { zone, recordName: config.domain.domainName, target });
      new route53.AaaaRecord(this, 'AliasAaaa', { zone, recordName: config.domain.domainName, target });
    }

    // ---- Web deployment ------------------------------------------------------------
    // Neither deployment prunes. Pruning in the second one would delete every hashed
    // asset, and old hashed assets are harmless (they keep old tabs working).
    const assetsDeployment = new s3deploy.BucketDeployment(this, 'DeployAssets', {
      sources: [s3deploy.Source.asset(config.webAssetDir)],
      destinationBucket: webBucket,
      exclude: ['index.html', 'config.json'],
      cacheControl: [s3deploy.CacheControl.fromString(immutableCacheControl)],
      prune: false,
    });
    const entryDeployment = new s3deploy.BucketDeployment(this, 'DeployEntry', {
      sources: [
        s3deploy.Source.asset(config.webAssetDir, { exclude: ['*', '!index.html'] }),
        s3deploy.Source.jsonData('config.json', {
          stage: config.stage,
          region,
          cognito: {
            userPoolId: userPool.userPoolId,
            clientId: userPoolClient.userPoolClientId,
            endpoint: null,
          },
        }),
      ],
      destinationBucket: webBucket,
      cacheControl: [s3deploy.CacheControl.noCache()],
      prune: false,
      distribution,
      distributionPaths: ['/index.html', '/config.json'],
    });
    // The entry point lands after the assets it names.
    entryDeployment.node.addDependency(assetsDeployment);

    // ---- Outputs -------------------------------------------------------------------
    new cdk.CfnOutput(this, 'SiteUrl', { value: siteUrl });
    new cdk.CfnOutput(this, 'DistributionId', { value: distribution.distributionId });
    new cdk.CfnOutput(this, 'UserPoolId', { value: userPool.userPoolId });
    new cdk.CfnOutput(this, 'UserPoolClientId', { value: userPoolClient.userPoolClientId });
    new cdk.CfnOutput(this, 'TableName', { value: table.tableName });
    new cdk.CfnOutput(this, 'DocsBucketName', { value: docsBucket.bucketName });
    new cdk.CfnOutput(this, 'ApiFunctionName', { value: apiFunction.functionName });
  }
}
