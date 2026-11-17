#!/usr/bin/env bash
# Create or refresh the public demo course through the normal ingestion path.
# See docs/architecture.md ("Public demo") and docs/local-dev.md.
#
#   scripts/demo-provision.sh [local|dev|prod] [--force] [--wait-seconds N]
#
# local (the default): the stack of `make dev` (.local/dev/api.env, or OPENBOOK_STACK_DIR). The
#   stack must be running, because its ingest worker processes the PDFs.
# dev, prod: the deployed stack Openbook-<stage>, with your AWS credentials (AWS_PROFILE). The
#   command reads the table, bucket, and queue names from the CloudFormation outputs, uploads the
#   PDFs of demo/course.json, and waits for the deployed ingest worker. prod needs ALLOW_PROD=1.
#
# A second run with the same content says "up to date" and changes nothing. New content builds
# a second course, switches the demo to it, and deletes the old one.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

stage="local"
case "${1:-}" in
  local | dev | prod)
    stage="$1"
    shift
    ;;
esac

if [ "$stage" = "local" ]; then
  # shellcheck source=scripts/lib/stack.sh
  . scripts/lib/stack.sh
  stack_dir="${OPENBOOK_STACK_DIR:-.local/dev}"
  if [ ! -f "${stack_dir}/api.env" ]; then
    echo "error: ${stack_dir}/api.env does not exist. Run 'make dev' (or 'make bootstrap') first." >&2
    exit 1
  fi
  cargo build -p openbook-demo
  stack_run_demo "${stack_dir}/api.env" "$@"
  exit
fi

if [ "$stage" = "prod" ] && [ "${ALLOW_PROD:-}" != "1" ]; then
  echo "error: refusing to change the prod demo without ALLOW_PROD=1." >&2
  exit 1
fi
if [ -z "${AWS_PROFILE:-}" ] && [ -z "${AWS_ACCESS_KEY_ID:-}" ]; then
  echo "error: set AWS_PROFILE (or AWS credentials) for the account of the ${stage} stack." >&2
  exit 1
fi
for tool in aws jq cargo; do
  if ! command -v "$tool" > /dev/null 2>&1; then
    echo "error: '$tool' is required" >&2
    exit 1
  fi
done

region="ap-southeast-2"
stack="Openbook-${stage}"
echo "demo: reading the outputs of ${stack} (read only)"
outputs="$(aws cloudformation describe-stacks --stack-name "$stack" --region "$region" \
  --query 'Stacks[0].Outputs' --output json)"
output() {
  jq -r --arg key "$1" '[.[] | select(.OutputKey == $key)][0].OutputValue // empty' <<< "$outputs"
}
table="$(output TableName)"
bucket="$(output DocsBucketName)"
queue="$(output IngestQueueUrl)"
if [ -z "$table" ] || [ -z "$bucket" ] || [ -z "$queue" ]; then
  echo "error: ${stack} has no TableName, DocsBucketName, or IngestQueueUrl output. Deploy it first." >&2
  exit 1
fi

cargo build --release -p openbook-demo
target="$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)"
echo "demo: provisioning ${stack} (table ${table})"
# A deployed run never uses an emulator endpoint: drop every local-only variable first.
unset AWS_ENDPOINT_URL OPENBOOK_CA_BUNDLE
OPENBOOK_STAGE="$stage" OPENBOOK_TABLE_NAME="$table" OPENBOOK_DOCS_BUCKET="$bucket" \
  OPENBOOK_INGEST_QUEUE_URL="$queue" AWS_REGION="$region" \
  OPENBOOK_FAST_MODEL="$(jq -r '.context.fastModel' infra/cdk.json)" \
  "${target}/release/openbook-demo" provision "$@"
