# Openbook infrastructure

The AWS CDK app (TypeScript) for Openbook. It creates the `Openbook-<stage>` stack in `ap-southeast-2` (`dev` or `prod`), and an optional certificate stack in `us-east-1` for a custom domain. Everything is pay per use (see `docs/architecture.md`).

## Commands

| Command | What it does |
|---|---|
| `npm run lint` | ESLint |
| `npm run typecheck` | `tsc --noEmit` |
| `npm run build` | `tsc` to `dist/` |
| `npm test` | Vitest: the security tests, the IAM template ties, the exact policy snapshots, the workflow checks |
| `npm run synth:placeholders` | `cdk synth` with the fixture assets in `test/fixtures` and no AWS identity (the script drops every `AWS_*` and `CDK_DEFAULT_*` variable and points the SDK at nothing) |
| `npm run render:iam -- --example` | Render the IAM templates (see `iam/README.md`) |

From the repository root, `make synth`, `make deploy STAGE=dev`, and `make check` wrap these.

## Context

The CDK app reads all settings from context (`cdk.json` or `-c key=value`). Nothing account-specific is committed. The stack is account-agnostic unless `CDK_DEFAULT_ACCOUNT` is set, and it then names the account through pseudo parameters.

| Key | Default | Meaning |
|---|---|---|
| `stage` | `dev` | `dev` or `prod`. Prod retains data, has point-in-time recovery, deletion protection, termination protection, the permissions boundary, and 25 reserved concurrent executions. |
| `apiAssetDir` | `../target/lambda/openbook-api` | The directory with the Lambda `bootstrap` binary |
| `webAssetDir` | `../apps/ask/dist` | The built web app. Synth refuses an `index.html` with an inline `<script>`, because the CSP blocks it. |
| `models`, `defaultChatModel`, `fastModel` | in `cdk.json` | The model catalog (`docs/architecture.md`, "Model catalog"). Synth fails if it breaks a rule, including the model ID allowlist. The Bedrock IAM policy comes from it. |
| `limitDailyMessagesPerUser`, `limitDailyMessagesGlobal`, `limitDailyTokensGlobal`, `limitFailedJoinsPerDay`, `limitCoursesOwned`, `limitDocumentsPerCourse`, `limitUploadBytes` | the API defaults | The `OPENBOOK_LIMIT_*` values |
| `domainName`, `hostedZoneName`, `hostedZoneId` | unset | A custom domain. All three are needed. The hosted zone ID avoids a lookup at synth time. |

## Deploy

1. Bootstrap the account and Region once with `cdk bootstrap` (qualifier `hnb659fds`).
2. Apply the IAM documents in `iam/` once, out of band (`iam/README.md`).
3. Dev: `AWS_PROFILE=<profile> make deploy STAGE=dev`.
4. Prod: push to `main`. `.github/workflows/deploy.yml` deploys it.

The deploy workflow has three jobs. `checks` runs `checks.yml` on the exact commit. `build` has no OIDC token: it builds the Lambda and the web app, runs the tests, runs `cdk synth -c stage=prod` with no AWS identity, and uploads `infra/cdk.out`. `deploy` is the only job with `id-token: write` and the `production` environment. It runs no build and no test: it downloads the assembly, runs `npm ci --ignore-scripts` for the pinned CDK CLI, and runs `cdk deploy Openbook-prod --app cdk.out --role-arn <execution role>`. Third-party code (pip, npm, cargo, Vite) therefore never runs next to the token.

`make deploy STAGE=prod ALLOW_PROD=1` exists for recovery only. CDK writes assets under the key prefix `openbook-<stage>/` of the bootstrap bucket, and the deploy role may write only under `openbook-prod/`.

## GitHub settings

- Environment `production`, with a deployment branch policy that allows `main` only.
- Environment secrets (not variables, because this repository and its logs are public): `AWS_ACCOUNT_ID`, `AWS_DEPLOY_ROLE_ARN`, and `AWS_CLOUDFORMATION_EXECUTION_ROLE_ARN`. The workflow masks the account ID and sends the `cdk deploy` output to a file, and only a filtered summary reaches the log.
- Branch protection on `main` that requires a pull request and the `checks` jobs. The deploy workflow runs the checks again on the commit it deploys, so a direct push cannot skip them.
- No AWS access key anywhere. The OIDC token is the only credential.

## Notes

- Cognito uses the default email sender. It allows about 50 emails a day. Add Amazon SES before public sign-up.
- Reserved concurrency needs headroom: Lambda keeps 100 units of the account quota unreserved. The account needs a quota of at least 100 plus every reserved total (25 for prod, 5 for dev). A new account has a quota of 10, which is too low. This account has 400.
- The docs bucket has a fixed name, `openbook-<stage>-docs-<account>-<region>`. A fixed name avoids a dependency cycle between its CORS rule (which names the CloudFront domain) and the distribution (which names the bucket in the CSP and the function).
- Only the hashed files under `assets/` get the one-year immutable cache. Every other file, the favicon included, is sent with `no-cache`.
- The two bucket-deployment helper functions share one log group with the stage retention.
- `cdk.json` sets `cli-telemetry` to `false`.
