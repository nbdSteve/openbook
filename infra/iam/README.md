# The deployment role documents

Four IAM documents belong to two roles: the GitHub deploy role that CI assumes, and the CloudFormation execution role that CloudFormation assumes. They live in an AWS account, not in the CDK stack, so nothing synthesizes them and nothing detects drift. This directory plus `../test/iam.test.ts` keep them honest. Nothing in this repository applies them, and no workflow here has the permission to.

| Template | Applied to |
|---|---|
| `github-deploy-role-trust.template.json` | The trust policy of the deploy role |
| `github-deploy-role-policy.template.json` | The inline policy of the deploy role |
| `cloudformation-execution-role-trust.template.json` | The trust policy of the execution role |
| `cloudformation-execution-role-policy.template.json` | The inline policy of the execution role |

## Why two roles

The default CDK deploy path assumes bootstrap roles, and the bootstrap execution role has `AdministratorAccess`. Anyone who can assume that path is an administrator. Openbook uses `CliCredentialsStackSynthesizer` (see `../lib/deployment.ts`), so CI uses its own role directly. That role can run CloudFormation on `Openbook-prod`, publish assets to the bootstrap bucket, and pass one role, to CloudFormation only. CloudFormation then creates the resources as the execution role, which can touch only `Openbook-prod` names.

## Placeholders

The applied documents name the account, the execution role, and GitHub's immutable repository identity. None of that belongs in a public repository, so each is an explicit `${PLACEHOLDER}`.

| Placeholder | What it is | Shape |
|---|---|---|
| `${ACCOUNT_ID}` | The twelve-digit AWS account | 12 digits |
| `${CLOUDFORMATION_EXECUTION_ROLE_NAME}` | The name of the execution role (the role-name part of the `AWS_CLOUDFORMATION_EXECUTION_ROLE_ARN` variable) | IAM role name |
| `${GITHUB_REPOSITORY_IDENTITY}` | GitHub's immutable `nbdSteve@<owner-id>/openbook@<repository-id>` segment | Two positive integers |

Everything else is a literal, because it is committed elsewhere and the tests tie the templates to that copy: the Region and stack name to `../bin/openbook.ts`, the qualifier to `../lib/deployment.ts`, and the resource name patterns to the synthesized `Openbook-prod` template.

## Render

```
npm run render:iam -- --example
npm run render:iam -- --account-id <12 digits> --repository-identity nbdSteve@<owner-id>/openbook@<repository-id> --execution-role-name <name>
```

Both write to `iam/rendered/`, which git ignores. Rendering is deterministic, and it refuses an unknown placeholder, a missing value, an unused value, a duplicate flag, and a badly shaped value. No message prints a supplied value.

`--example` uses `example-values.json`: the account `000000000000` and synthetic identifiers.

## Apply (out of band)

Use admin credentials for the target account in your own terminal. Replace `<deploy-role>` and `<execution-role>` with your role names, and use the same execution role name that you rendered.

1. Find the repository identity: `gh api repos/nbdSteve/openbook --jq '"nbdSteve@\(.owner.id)/openbook@\(.id)"'`. The repository must issue OIDC subjects in this immutable form. Check it with `gh api repos/nbdSteve/openbook/actions/oidc/customization/sub`.
2. Confirm the account has the GitHub OIDC provider `token.actions.githubusercontent.com` (`aws iam list-open-id-connect-providers`). Another repository in the account may already use it.
3. Render the documents with the real values.
4. Create the execution role: `aws iam create-role --role-name <execution-role> --assume-role-policy-document file://iam/rendered/cloudformation-execution-role-trust.json`, then `aws iam put-role-policy --role-name <execution-role> --policy-name OpenbookProdExecution --policy-document file://iam/rendered/cloudformation-execution-role-policy.json`.
5. Create the deploy role: `aws iam create-role --role-name <deploy-role> --assume-role-policy-document file://iam/rendered/github-deploy-role-trust.json`, then `aws iam put-role-policy --role-name <deploy-role> --policy-name OpenbookProdDeploy --policy-document file://iam/rendered/github-deploy-role-policy.json`.
6. Set the two variables on the GitHub `production` environment: `AWS_DEPLOY_ROLE_ARN` and `AWS_CLOUDFORMATION_EXECUTION_ROLE_ARN`.

To change a document later, edit the template, run the tests, render, and apply with `aws iam put-role-policy` or `aws iam update-assume-role-policy`.

## Limits

- The execution role action list comes from the committed stack, not from a deployment. The first prod deploy may find a missing action. That fails as a denied CloudFormation call and a rollback, not as a half-deployed stack. Add the action to the template, and apply it again.
- The execution role can create a role named `Openbook-prod-*`, put an inline policy on it, and pass it to Lambda. Reaching that needs a merge to `main`. A permissions boundary on that grant would close the path.
- A custom domain needs more than this: a Route 53 grant, an ACM certificate stack in `us-east-1`, and cross-Region reference permissions. Add them to the templates when you turn the domain on.
