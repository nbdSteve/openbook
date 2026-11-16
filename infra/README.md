# Openbook infrastructure

The AWS CDK app (TypeScript) for Openbook. It creates the `Openbook-<stage>` stack in `ap-southeast-2` (`dev` or `prod`), and an optional certificate stack in `us-east-1` for a custom domain. Everything is pay per use (see `docs/architecture.md`).

## Commands

| Command | What it does |
|---|---|
| `npm run lint` | ESLint |
| `npm run typecheck` | `tsc --noEmit` |
| `npm run build` | `tsc` to `dist/` |
| `npm test` | Vitest: the security tests, the IAM template ties, the workflow checks |
| `npm run synth:placeholders` | `cdk synth` with the fixture assets in `test/fixtures` (no credentials) |
| `npm run render:iam -- --example` | Render the IAM templates (see `iam/README.md`) |

From the repository root, `make synth`, `make deploy STAGE=dev`, and `make check` wrap these.

## Context

The CDK app reads all settings from context (`cdk.json` or `-c key=value`). Nothing account-specific is committed. The account comes from `CDK_DEFAULT_ACCOUNT`.

| Key | Default | Meaning |
|---|---|---|
| `stage` | `dev` | `dev` or `prod`. Prod retains data, has point-in-time recovery and deletion protection, and 25 reserved concurrent executions. |
| `apiAssetDir` | `../target/lambda/openbook-api` | The directory with the Lambda `bootstrap` binary |
| `webAssetDir` | `../apps/ask/dist` | The built web app |
| `models`, `defaultChatModel`, `fastModel` | in `cdk.json` | The model catalog (`docs/architecture.md`, "Model catalog"). Synth fails if it breaks a rule. The Bedrock IAM policy comes from it. |
| `limitDailyMessagesPerUser`, `limitDailyMessagesGlobal`, `limitCoursesOwned`, `limitDocumentsPerCourse`, `limitUploadBytes` | the API defaults | The `OPENBOOK_LIMIT_*` values |
| `domainName`, `hostedZoneName`, `hostedZoneId` | unset | A custom domain. All three are needed. The hosted zone ID avoids a lookup at synth time. |

## Deploy

1. Bootstrap the account and Region once with `cdk bootstrap` (qualifier `hnb659fds`).
2. Dev: `AWS_PROFILE=<profile> make deploy STAGE=dev`.
3. Prod: push to `main`. `.github/workflows/deploy.yml` deploys it with the roles in `iam/`. `make deploy STAGE=prod ALLOW_PROD=1` exists for recovery.

CI deploys with `cdk deploy Openbook-prod --role-arn <execution role>`. The synthesizer in `lib/deployment.ts` skips the bootstrap deploy roles, so the narrow roles in `iam/` are the only path.

## GitHub settings

- Environment `production`, with a deployment branch policy that allows `main` only.
- Environment variables: `AWS_DEPLOY_ROLE_ARN` and `AWS_CLOUDFORMATION_EXECUTION_ROLE_ARN`.
- Branch protection on `main` with the `checks` jobs as required status checks, so that only checked commits reach the deploy workflow.
- No AWS secret anywhere. The OIDC token is the only credential.

## Notes

- Cognito uses the default email sender. It allows about 50 emails a day. Add Amazon SES before public sign-up.
- Reserved concurrency (5 and 25) needs an account concurrency quota above 10 plus the reserved amount.
- The docs bucket has a fixed name, `openbook-<stage>-docs-<account>-<region>`. A fixed name avoids a dependency cycle between its CORS rule (which names the CloudFront domain) and the distribution (which names the bucket in the CSP and the function).
