# The deployment role documents

Five IAM documents belong to three things: the GitHub deploy role that CI assumes, the CloudFormation execution role that CloudFormation assumes, and the permissions boundary policy that caps every role the stack creates. They live in an AWS account, not in the CDK stack, so nothing synthesizes them and nothing detects drift. This directory plus the tests in `../test/` keep them honest. Nothing in this repository applies them, and no workflow here has the permission to.

| Template | Applied to |
|---|---|
| `permissions-boundary-policy.template.json` | A customer-managed policy named `OpenbookProdPermissionsBoundary` |
| `cloudformation-execution-role-trust.template.json` | The trust policy of the execution role |
| `cloudformation-execution-role-policy.template.json` | The inline policy of the execution role |
| `github-deploy-role-trust.template.json` | The trust policy of the deploy role |
| `github-deploy-role-policy.template.json` | The inline policy of the deploy role |

## Why this shape

The default CDK deploy path assumes bootstrap roles, and the bootstrap execution role has `AdministratorAccess`. Anyone who can assume that path is an administrator. Openbook uses `CliCredentialsStackSynthesizer` (see `../lib/deployment.ts`), so CI uses its own role directly. That role can run CloudFormation on `Openbook-prod`, publish assets under one prefix of the bootstrap bucket, and pass one role, to CloudFormation only. CloudFormation then creates the resources as the execution role, which can touch only `Openbook-prod` names.

Two controls close the privilege escalation that remains. The execution role can create roles, and a role that has any policy could run code with it through the bucket-deployment Lambda.

1. The permissions boundary. The execution role may create a role, or change its policies, only when the role carries `OpenbookProdPermissionsBoundary`. It can never remove the boundary (`iam:DeleteRolePermissionsBoundary` is never allowed, and an explicit Deny backs that up). The boundary allows only what the stack's own roles need: the application table, the stack buckets, read of the Openbook assets, Bedrock invocation, CloudFront invalidation, and Lambda logs. It allows no IAM, STS, CloudFormation, Lambda, or Cognito action, so a role under it cannot make another role or reach beyond the application. The CDK app adds the boundary to every role of the prod stack (`PermissionsBoundaryAspect` in `../lib/openbook-stack.ts`), and a test checks every `AWS::IAM::Role` of the synthesized prod template.
2. The split deploy workflow. The job that has the OIDC token (`.github/workflows/deploy.yml`) runs no build, no test, and no install script. A job without the token builds and synthesizes. See `../README.md`.

## Placeholders

The applied documents name the account, the execution role, and GitHub's immutable repository identity. None of that belongs in a public repository, so each is an explicit `${PLACEHOLDER}`.

| Placeholder | What it is | Shape |
|---|---|---|
| `${ACCOUNT_ID}` | The twelve-digit AWS account | 12 digits |
| `${CLOUDFORMATION_EXECUTION_ROLE_NAME}` | The name of the execution role (the role-name part of the `AWS_CLOUDFORMATION_EXECUTION_ROLE_ARN` secret) | IAM role name |
| `${GITHUB_REPOSITORY_IDENTITY}` | GitHub's immutable `nbdSteve@<owner-id>/openbook@<repository-id>` segment | Two positive integers |

Everything else is a literal, because it is committed elsewhere and the tests tie the templates to that copy: the Region and stack name to `../bin/openbook.ts`, the qualifier, the asset prefix, the boundary name, and the CloudFront Function name to `../lib/deployment.ts`, and the resource name patterns to the synthesized `Openbook-prod` template.

## Render

```
npm run render:iam -- --example
npm run render:iam -- --account-id <12 digits> --repository-identity nbdSteve@<owner-id>/openbook@<repository-id> --execution-role-name <name>
```

Both write to `iam/rendered/`. The renderer writes only to a folder that git ignores (a folder outside any repository is refused too), so rendered documents cannot reach a commit. Rendering is deterministic. It refuses an unknown placeholder, a missing value, an unused value, a duplicate flag, a badly shaped value, and any `$` left in the output (a malformed placeholder such as `${ACCOUNT-ID}` or `$ACCOUNT_ID`). No message prints a supplied value.

`--example` uses `example-values.json`: the account `000000000000` and synthetic identifiers.

`../test/snapshots/iam-*.json` hold the example rendering of every template. A change to a template fails a test until you review the diff and run `npx vitest run -u`.

## Apply (out of band)

Use admin credentials for the target account in your own terminal. Use the same execution role name here that you render with. Order matters: the boundary policy must exist before the first deploy creates a role with it.

1. Find the repository identity: `gh api repos/nbdSteve/openbook --jq '"nbdSteve@\(.owner.id)/openbook@\(.id)"'`. The repository must issue OIDC subjects in this immutable form. Check it with `gh api repos/nbdSteve/openbook/actions/oidc/customization/sub`.
2. Confirm that the account has the GitHub OIDC provider `token.actions.githubusercontent.com` (`aws iam list-open-id-connect-providers`). Another repository in the account may already use it.
3. Render the documents with the real values.
4. Create the boundary policy: `aws iam create-policy --policy-name OpenbookProdPermissionsBoundary --policy-document file://iam/rendered/permissions-boundary-policy.json`. Do not change the name. The execution role and the CDK app both refer to it.
5. Create the execution role: `aws iam create-role --role-name <execution-role> --assume-role-policy-document file://iam/rendered/cloudformation-execution-role-trust.json`, then `aws iam put-role-policy --role-name <execution-role> --policy-name OpenbookProdExecution --policy-document file://iam/rendered/cloudformation-execution-role-policy.json`.
6. Create the deploy role: `aws iam create-role --role-name <deploy-role> --assume-role-policy-document file://iam/rendered/github-deploy-role-trust.json`, then `aws iam put-role-policy --role-name <deploy-role> --policy-name OpenbookProdDeploy --policy-document file://iam/rendered/github-deploy-role-policy.json`.
7. Set the three secrets on the GitHub `production` environment (see `../README.md`).

To change a document later, edit the template, run the tests, update the snapshots, render, and apply. A boundary change uses `aws iam create-policy-version --policy-arn <arn> --policy-document file://... --set-as-default`. The execution role cannot do that, and that is the point: only you, out of band, can widen the boundary.

## Limits

- The execution role action list comes from the committed stack, not from a deployment. The first prod deploy may find a missing action. That fails as a denied CloudFormation call and a rollback, not as a half-deployed stack. Add the action to the template, snapshot, and apply it again.
- `iam:UpdateAssumeRolePolicy` is not conditioned on the boundary, because AWS does not support that condition for it. A role can still only do what its boundary allows.
- If an Openbook change needs a new kind of permission for a stack role (a new AWS service, say), add it to the boundary template and apply the new policy version first.
- The deploy role can list only the `openbook-prod/` prefix of the bootstrap bucket. If cdk-assets treats the resulting `403` on a missing object as an error, the first publish shows it. The fix is a wider `ListBucket` grant, not a wider object grant.
- A custom domain needs more than this: a Route 53 grant, an ACM certificate stack in `us-east-1`, and cross-Region reference permissions. Add them to the templates when you turn the domain on.
