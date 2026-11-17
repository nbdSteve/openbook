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

1. The permissions boundary. The execution role may create a role, or change its policies, only when the role carries `OpenbookProdPermissionsBoundary`. It can never remove the boundary (`iam:DeleteRolePermissionsBoundary` is never allowed, and an explicit Deny backs that up). The boundary allows only what the stack's own roles need: the application table, the stack buckets, read of the Openbook assets, the ingest queue (send and consume), the stage's S3 Vectors indexes, Bedrock invocation of the catalog models and the embedding model, CloudFront invalidation, and Lambda logs. It allows no IAM, STS, CloudFormation, Lambda, or Cognito action, so a role under it cannot make another role or reach beyond the application. The CDK app adds the boundary to every role of the prod stack (`PermissionsBoundaryAspect` in `../lib/openbook-stack.ts`), and a test checks every `AWS::IAM::Role` of the synthesized prod template.
2. The split deploy workflow. The job that has the OIDC token (`.github/workflows/deploy.yml`) runs no build, no test, and no install script. A job without the token builds and synthesizes. See `../README.md`.

## Placeholders

The applied documents name the account, the execution role, and GitHub's immutable repository identity. None of that belongs in a public repository, so each is an explicit `${PLACEHOLDER}`.

| Placeholder | What it is | Shape |
|---|---|---|
| `${ACCOUNT_ID}` | The twelve-digit AWS account | 12 digits |
| `${CLOUDFORMATION_EXECUTION_ROLE_NAME}` | The name of the execution role (the role-name part of the `AWS_CLOUDFORMATION_EXECUTION_ROLE_ARN` secret) | IAM role name |
| `${GITHUB_REPOSITORY_IDENTITY}` | GitHub's immutable `nbdSteve@<owner-id>/openbook@<repository-id>` segment | Two positive integers |
| `${HOSTED_ZONE_ID}` | The Route 53 hosted zone of the site domain (the `OPENBOOK_HOSTED_ZONE_ID` secret) | `Z` and up to 32 capital letters or digits |

Everything else is a literal, because it is committed elsewhere and the tests tie the templates to that copy: the Region and stack name to `../bin/openbook.ts`, the qualifier, the asset prefix, the boundary name, and the CloudFront Function name to `../lib/deployment.ts`, and the resource name patterns to the synthesized `Openbook-prod` template.

## Render

```
npm run render:iam -- --example
npm run render:iam -- --account-id <12 digits> --repository-identity nbdSteve@<owner-id>/openbook@<repository-id> --execution-role-name <name> --hosted-zone-id <Z...>
```

Both write to `iam/rendered/`. The renderer writes only to a folder that git ignores (a folder outside any repository is refused too), so rendered documents cannot reach a commit. Rendering is deterministic. It refuses an unknown placeholder, a missing value, an unused value, a duplicate flag, a badly shaped value, and any `$` left in the output (a malformed placeholder such as `${ACCOUNT-ID}` or `$ACCOUNT_ID`). No message prints a supplied value.

`--example` uses `example-values.json`: the account `000000000000` and synthetic identifiers.

`../test/snapshots/iam-*.json` hold the rendering of every template with the account `111111111111` (the fake account that the stack tests use) and the other values of `example-values.json`. The Bedrock resources of the boundary come from the catalog in `cdk.json`. A change to a template or to the catalog fails a test until you review the diff and run `npx vitest run -u`. `stack-policies.prod.json` holds every statement of the roles that the stack creates.

## Apply (out of band)

The render script writes the Bedrock resources of the boundary from the model catalog in `cdk.json`, so a catalog change needs a new boundary version (see below) before the deploy that uses the new models.

Use admin credentials for the target account in your own terminal. Use the same execution role name here that you render with. Order matters: the boundary policy must exist before the first deploy creates a role with it.

1. Find the repository identity: `gh api repos/nbdSteve/openbook --jq '"nbdSteve@\(.owner.id)/openbook@\(.id)"'`. The repository must issue OIDC subjects in this immutable form. Check it with `gh api repos/nbdSteve/openbook/actions/oidc/customization/sub`.
2. Confirm that the account has the GitHub OIDC provider `token.actions.githubusercontent.com` (`aws iam list-open-id-connect-providers`). Another repository in the account may already use it.
3. Render the documents with the real values.
4. Create the boundary policy: `aws iam create-policy --policy-name OpenbookProdPermissionsBoundary --policy-document file://iam/rendered/permissions-boundary-policy.json`. Do not change the name. The execution role and the CDK app both refer to it.
5. Create the execution role: `aws iam create-role --role-name <execution-role> --assume-role-policy-document file://iam/rendered/cloudformation-execution-role-trust.json`, then `aws iam put-role-policy --role-name <execution-role> --policy-name OpenbookProdExecution --policy-document file://iam/rendered/cloudformation-execution-role-policy.json`.
6. Create the deploy role: `aws iam create-role --role-name <deploy-role> --assume-role-policy-document file://iam/rendered/github-deploy-role-trust.json`, then `aws iam put-role-policy --role-name <deploy-role> --policy-name OpenbookProdDeploy --policy-document file://iam/rendered/github-deploy-role-policy.json`.
7. Set the five secrets on the GitHub `production` environment (see `../README.md`).

### Update an existing setup (the custom domain added Route 53 and ACM to the execution role)

Only the execution role policy changes (the boundary, the deploy role, and both trust policies are the same), so apply only that. Earlier updates (phase 2: queues, a worker, and a vector bucket) also changed the boundary, and the steps below keep that case.

For the custom domain, render with the same values as the first time plus `--hosted-zone-id`, then run only step 3.

For a boundary change, apply the boundary first, then the execution role. Both are new versions of documents that exist.

1. Render with the same values as the first time.
2. `aws iam create-policy-version --policy-arn arn:aws:iam::<account>:policy/OpenbookProdPermissionsBoundary --policy-document file://iam/rendered/permissions-boundary-policy.json --set-as-default`. A managed policy keeps five versions. If it has five, delete the oldest non-default one first with `aws iam delete-policy-version`.
3. `aws iam put-role-policy --role-name <execution-role> --policy-name OpenbookProdExecution --policy-document file://iam/rendered/cloudformation-execution-role-policy.json`.
4. Compare the rendered documents with what is applied (`aws iam get-policy-version`, `aws iam get-role-policy`) before you merge the stack change.

To change a document later, edit the template, run the tests, update the snapshots, render, and apply. A boundary change uses `aws iam create-policy-version --policy-arn <arn> --policy-document file://... --set-as-default`. The execution role cannot do that, and that is the point: only you, out of band, can widen the boundary.

## Limits

- The execution role action list comes from the committed stack, not from a deployment. The first prod deploy may find a missing action. That fails as a denied CloudFormation call and a rollback, not as a half-deployed stack. Add the action to the template, snapshot, and apply it again.
- The event source mapping actions (create, update, delete, get) cannot be limited by name, because a mapping ID is a random UUID. Create and update carry the condition `lambda:FunctionArn` = `Openbook-prod-*`, so a mapping can be made or changed only for the stack's functions. Get and delete carry no condition, because AWS does not support the key on them (CloudFormation reads the mapping after it creates it, and a denial there rolls the first deploy back). So the execution role can read or delete any event source mapping of the account. That is a residual risk. The vector bucket and queue grants are name-scoped (`openbook-prod-*` and `Openbook-prod-ingest*`). `s3vectors:ListVectorBuckets` is not granted, because CloudFormation does not need it to create, update, or delete a bucket.
- `iam:UpdateAssumeRolePolicy` is not conditioned on the boundary, because AWS does not support that condition for it. A role can still only do what its boundary allows.
- If an Openbook change needs a new kind of permission for a stack role (a new AWS service, say) or a new model in the catalog, render the boundary again and apply the new policy version first.
- Residual risk: the execution role can `CreateRole` with any trust policy (for example one that trusts another account). AWS has no condition key for the content of a trust document, so IAM cannot prevent it. The boundary caps what such a role could do to the application table, the stack buckets, the Openbook assets, the catalog's Bedrock models, CloudFront invalidation, and logs, and it allows no IAM action. A role that trusts a stranger still cannot do more than that. Only a change to the stack (a merge to `main` that passes the checks) can create such a role.
- The Lambda grants have conditions: `lambda:AddPermission` only for the principal `cloudfront.amazonaws.com`, and function URLs only with `AWS_IAM` auth, so no template can make a function public. The `DeletionPolicy: Retain` resources of prod (table, buckets, user pool) outlive the stack, so also review changes to them as data changes.
- The user pool grants carry the condition `aws:ResourceTag/aws:cloudformation:stack-name` = `Openbook-prod`. CloudFormation tags the pool, but this was not tested against a real deploy. If the first deploy is denied on a Cognito action, check that tag first, and remove the condition as a last resort. `CreateUserPool` itself takes no resource and no usable condition.
- The deploy role cannot call `DeleteStack` and cannot create import change sets (an explicit Deny on `ChangeSetType` `IMPORT`). It keeps `UpdateTerminationProtection`, because the CDK CLI sets protection on the first deploy of a stack that has it. If a first deploy fails and leaves the stack in `ROLLBACK_COMPLETE`, delete that stack yourself with your admin credentials, then run the deploy again.
- The deploy role can list only the `openbook-prod/` prefix of the bootstrap bucket. If cdk-assets treats the resulting `403` on a missing object as an error, the first publish shows it. The fix is a wider `ListBucket` grant, not a wider object grant.
- The custom domain adds three Route 53 statements and one ACM statement to the execution role (not to the deploy role or the boundary). `route53:ChangeResourceRecordSets` is limited to the one hosted zone (`${HOSTED_ZONE_ID}`), to the record name `ask.steve.gg` (the domain in `../cdk.json`, which a test checks), and to the record types `A` and `AAAA`. `route53:ListResourceRecordSets` is limited to the same zone. `route53:GetChange` (CloudFormation waits for a record change to be `INSYNC`) takes only a change ID, so its resource is `change/*`. `acm:ListCertificates` has no resource-level scope (AWS lists it for `*`), and CloudFront needs it to attach an ACM certificate to a distribution. The execution role gets no other ACM action: the certificate is made and validated by hand, and it is passed to the stack as a parameter. The deploy role needs nothing new, because `--parameters` only changes the stack input.
