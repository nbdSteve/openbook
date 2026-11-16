#!/usr/bin/env bash
# Create the local resources in AWS Emulator and write the local config files.
# See docs/local-dev.md. Safe to run twice.
#
#   scripts/bootstrap-local.sh [--emulator-port N] [--web-port N] [--api-port N]
#                              [--stack-dir DIR] [--env-file PATH] [--web-config PATH]
#                              [--daily-messages-per-user N]
#
# Defaults: OPENBOOK_EMULATOR_PORT (9297), OPENBOOK_WEB_PORT (5173), OPENBOOK_API_PORT (8787),
# and the stack folder .local/dev. The API environment goes to <stack-dir>/api.env and the web
# runtime config to <stack-dir>/web-config.json (the dev server serves it as /config.json
# when OPENBOOK_WEB_CONFIG names it). Each stack has its own folder, so `make dev` and
# `make e2e-local` do not share a file. If OPENBOOK_BEDROCK_PROFILE is set, the API
# environment gets the deployed model catalog from infra/cdk.json (real Bedrock) instead of
# the two fake models.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/pdfium.sh
. "${repo_root}/scripts/lib/pdfium.sh"

emulator_port="${OPENBOOK_EMULATOR_PORT:-9297}"
web_port="${OPENBOOK_WEB_PORT:-5173}"
api_port="${OPENBOOK_API_PORT:-8787}"
stack_dir="${repo_root}/.local/dev"
env_file=""
web_config=""
daily_messages_per_user=""
bedrock_profile="${OPENBOOK_BEDROCK_PROFILE:-}"

usage() {
  sed -n '2,18p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
  exit 2
}

while [ $# -gt 0 ]; do
  case "$1" in
    --emulator-port) emulator_port="${2:?--emulator-port needs a value}"; shift 2 ;;
    --web-port) web_port="${2:?--web-port needs a value}"; shift 2 ;;
    --api-port) api_port="${2:?--api-port needs a value}"; shift 2 ;;
    --stack-dir) stack_dir="${2:?--stack-dir needs a value}"; shift 2 ;;
    --daily-messages-per-user) daily_messages_per_user="${2:?--daily-messages-per-user needs a value}"; shift 2 ;;
    --env-file) env_file="${2:?--env-file needs a value}"; shift 2 ;;
    --web-config) web_config="${2:?--web-config needs a value}"; shift 2 ;;
    -h | --help) usage ;;
    *) echo "error: unknown argument: $1" >&2; usage ;;
  esac
done

env_file="${env_file:-${stack_dir}/api.env}"
web_config="${web_config:-${stack_dir}/web-config.json}"

for value in "$emulator_port" "$web_port" "$api_port" ${daily_messages_per_user:+"$daily_messages_per_user"}; do
  if [[ ! "$value" =~ ^[0-9]+$ ]]; then
    echo "error: ports and limits must be numbers, got '$value'" >&2
    exit 2
  fi
done

for tool in aws jq curl; do
  if ! command -v "$tool" > /dev/null 2>&1; then
    echo "error: '$tool' is required (AWS CLI v2, jq, and curl)" >&2
    exit 1
  fi
done

# ---- Never touch real AWS --------------------------------------------------------------
# Drop every AWS variable (credentials, profile, endpoint overrides, config files), then
# point the CLI at the emulator with test keys. Then prove it: the emulator account is
# 000000000000, and no real account has that ID.
while IFS= read -r name; do
  unset "$name"
done < <(env | sed -n 's/^\(AWS_[A-Za-z0-9_]*\)=.*/\1/p')

emulator_dir="${HOME}/.aws/emulator/${emulator_port}"
ca_pem="${emulator_dir}/ca.pem"
endpoint="https://127.0.0.1:${emulator_port}"

if [ ! -s "$ca_pem" ]; then
  echo "error: ${ca_pem} is missing. Start the emulator first: scripts/emulator.sh up ${emulator_port}" >&2
  exit 1
fi

export AWS_ENDPOINT_URL="$endpoint"
export AWS_CA_BUNDLE="$ca_pem"
export AWS_ACCESS_KEY_ID=test
export AWS_SECRET_ACCESS_KEY=test
export AWS_REGION=us-east-1
export AWS_DEFAULT_REGION=us-east-1
export AWS_PAGER=""
export AWS_EC2_METADATA_DISABLED=true
export AWS_CONFIG_FILE=/dev/null
export AWS_SHARED_CREDENTIALS_FILE=/dev/null

account="$(aws sts get-caller-identity --query Account --output text 2> /dev/null || true)"
if [ "$account" != "000000000000" ]; then
  echo "error: ${endpoint} did not answer as AWS Emulator (account '${account:-none}', expected 000000000000)." >&2
  echo "Refusing to continue. Is the emulator running? scripts/emulator.sh up ${emulator_port}" >&2
  exit 1
fi

# ---- Names (docs/local-dev.md) ---------------------------------------------------------
table_name="openbook-local"
docs_bucket="openbook-local-docs"
pool_name="openbook-local"
client_name="openbook-local"
vector_bucket="openbook-local-vectors"
queue_name="openbook-local-ingest.fifo"
dlq_name="openbook-local-ingest-dlq.fifo"
seed_password="Openbook-local-1"
seed_users=("teacher@openbook.test" "student@openbook.test")

log() { echo "bootstrap: $*"; }

# ---- DynamoDB --------------------------------------------------------------------------
if aws dynamodb describe-table --table-name "$table_name" > /dev/null 2>&1; then
  log "table ${table_name} exists"
else
  log "creating table ${table_name}"
  aws dynamodb create-table \
    --table-name "$table_name" \
    --attribute-definitions \
    AttributeName=pk,AttributeType=S AttributeName=sk,AttributeType=S \
    AttributeName=gsi1pk,AttributeType=S AttributeName=gsi1sk,AttributeType=S \
    --key-schema AttributeName=pk,KeyType=HASH AttributeName=sk,KeyType=RANGE \
    --billing-mode PAY_PER_REQUEST \
    --global-secondary-indexes \
    'IndexName=gsi1,KeySchema=[{AttributeName=gsi1pk,KeyType=HASH},{AttributeName=gsi1sk,KeyType=RANGE}],Projection={ProjectionType=ALL}' \
    > /dev/null
fi

waited=0
until [ "$(aws dynamodb describe-table --table-name "$table_name" --query 'Table.TableStatus' --output text)" = "ACTIVE" ]; do
  waited=$((waited + 1))
  if [ "$waited" -gt 60 ]; then
    echo "error: table ${table_name} is not ACTIVE after 60s" >&2
    exit 1
  fi
  sleep 1
done

ttl_status="$(aws dynamodb describe-time-to-live --table-name "$table_name" --query 'TimeToLiveDescription.TimeToLiveStatus' --output text)"
if [ "$ttl_status" != "ENABLED" ] && [ "$ttl_status" != "ENABLING" ]; then
  log "enabling TTL on attribute ttl"
  aws dynamodb update-time-to-live --table-name "$table_name" \
    --time-to-live-specification Enabled=true,AttributeName=ttl > /dev/null
fi

# ---- S3 docs bucket --------------------------------------------------------------------
if aws s3api head-bucket --bucket "$docs_bucket" > /dev/null 2>&1; then
  log "bucket ${docs_bucket} exists"
else
  log "creating bucket ${docs_bucket}"
  aws s3api create-bucket --bucket "$docs_bucket" > /dev/null
fi
cors="$(jq -nc --arg origin "http://localhost:${web_port}" '{CORSRules: [{
  AllowedMethods: ["PUT"],
  AllowedOrigins: [$origin],
  AllowedHeaders: ["content-type", "content-length", "x-amz-*"],
  ExposeHeaders: ["ETag"],
  MaxAgeSeconds: 3000}]}')"
aws s3api put-bucket-cors --bucket "$docs_bucket" --cors-configuration "$cors"

# ---- Cognito ---------------------------------------------------------------------------
pool_id="$(aws cognito-idp list-user-pools --max-results 60 --output json \
  | jq -r --arg name "$pool_name" '[.UserPools[] | select(.Name == $name)][0].Id // empty')"
if [ -z "$pool_id" ]; then
  log "creating user pool ${pool_name}"
  pool_id="$(aws cognito-idp create-user-pool \
    --pool-name "$pool_name" \
    --username-attributes email \
    --auto-verified-attributes email \
    --policies 'PasswordPolicy={MinimumLength=8,RequireUppercase=true,RequireLowercase=true,RequireNumbers=true,RequireSymbols=false}' \
    --account-recovery-setting 'RecoveryMechanisms=[{Priority=1,Name=verified_email}]' \
    --admin-create-user-config AllowAdminCreateUserOnly=false \
    --query 'UserPool.Id' --output text)"
else
  log "user pool ${pool_name} exists (${pool_id})"
fi

client_id="$(aws cognito-idp list-user-pool-clients --user-pool-id "$pool_id" --max-results 60 --output json \
  | jq -r --arg name "$client_name" '[.UserPoolClients[] | select(.ClientName == $name)][0].ClientId // empty')"
if [ -z "$client_id" ]; then
  log "creating app client ${client_name}"
  client_id="$(aws cognito-idp create-user-pool-client \
    --user-pool-id "$pool_id" \
    --client-name "$client_name" \
    --no-generate-secret \
    --explicit-auth-flows ALLOW_USER_PASSWORD_AUTH ALLOW_REFRESH_TOKEN_AUTH \
    --prevent-user-existence-errors ENABLED \
    --query 'UserPoolClient.ClientId' --output text)"
else
  log "app client ${client_name} exists (${client_id})"
fi

for user in "${seed_users[@]}"; do
  if aws cognito-idp admin-get-user --user-pool-id "$pool_id" --username "$user" > /dev/null 2>&1; then
    log "user ${user} exists"
  else
    log "creating user ${user}"
    aws cognito-idp admin-create-user \
      --user-pool-id "$pool_id" \
      --username "$user" \
      --user-attributes "Name=email,Value=${user}" Name=email_verified,Value=true \
      --message-action SUPPRESS > /dev/null
  fi
  # Always set the password, so a second run repairs a user who changed it.
  aws cognito-idp admin-set-user-password \
    --user-pool-id "$pool_id" \
    --username "$user" \
    --password "$seed_password" \
    --permanent
done

# ---- Ingest queue and vector bucket ----------------------------------------------------
# The same settings as the deployed stack (docs/retrieval.md): FIFO, the sender names the
# deduplication ID, visibility 960 s, retention 4 days, and a FIFO dead-letter queue after 5
# receives. The script sets them on a queue that already exists too.
if aws s3vectors get-vector-bucket --vector-bucket-name "$vector_bucket" > /dev/null 2>&1; then
  log "vector bucket ${vector_bucket} exists"
else
  log "creating vector bucket ${vector_bucket}"
  aws s3vectors create-vector-bucket --vector-bucket-name "$vector_bucket" > /dev/null
fi

if ! aws sqs get-queue-url --queue-name "$dlq_name" > /dev/null 2>&1; then
  log "creating queue ${dlq_name}"
  aws sqs create-queue --queue-name "$dlq_name" \
    --attributes FifoQueue=true,MessageRetentionPeriod=1209600 > /dev/null
fi
dlq_url="$(aws sqs get-queue-url --queue-name "$dlq_name" --query QueueUrl --output text)"
dlq_arn="$(aws sqs get-queue-attributes --queue-url "$dlq_url" --attribute-names QueueArn --query Attributes.QueueArn --output text)"
queue_settings="$(jq -nc --arg arn "$dlq_arn" '{
  ContentBasedDeduplication: "false",
  VisibilityTimeout: "960",
  MessageRetentionPeriod: "345600",
  RedrivePolicy: ({deadLetterTargetArn: $arn, maxReceiveCount: 5} | tojson)}')"
if aws sqs get-queue-url --queue-name "$queue_name" > /dev/null 2>&1; then
  log "queue ${queue_name} exists"
else
  log "creating queue ${queue_name}"
  aws sqs create-queue --queue-name "$queue_name" --attributes '{"FifoQueue":"true"}' > /dev/null
fi
queue_url="$(aws sqs get-queue-url --queue-name "$queue_name" --query QueueUrl --output text)"
aws sqs set-queue-attributes --queue-url "$queue_url" --attributes "$queue_settings"

# ---- pdfium for the local worker ---------------------------------------------------------
# The worker opens PDFs with pdfium. The fetch script downloads the pinned build for this
# machine into .local/pdfium/<platform>/ and prints where the library is.
pdfium_path=""
if [ -x "${OPENBOOK_PDFIUM_FETCH:-${repo_root}/scripts/fetch-pdfium.sh}" ]; then
  pdfium_path="$(pdfium_fetch "$(pdfium_platform)")"
  log "pdfium: ${pdfium_path}"
else
  log "warning: scripts/fetch-pdfium.sh does not exist yet, so OPENBOOK_PDFIUM_PATH is not set"
fi

# ---- Files -----------------------------------------------------------------------------
write_atomic() { # write_atomic <path> <mode>; the content comes from stdin
  local target="$1" mode="$2" tmp
  mkdir -p "$(dirname "$target")"
  tmp="$(mktemp "${target}.XXXXXX")"
  cat > "$tmp"
  chmod "$mode" "$tmp"
  mv "$tmp" "$target"
}

# Quote a value for a file that Bash sources: single quotes, with each inner quote written
# as '\''. The catalog is JSON, which has double quotes and may have anything else.
sq() { printf "'%s'" "${1//\'/\'\\\'\'}"; }

if [ -n "$bedrock_profile" ]; then
  # Hybrid mode: the deployed catalog on real Bedrock. Everything else stays on the emulator.
  models="$(jq -c '.context.models' "${repo_root}/infra/cdk.json")"
  default_chat="$(jq -r '.context.defaultChatModel' "${repo_root}/infra/cdk.json")"
  fast_model="$(jq -r '.context.fastModel' "${repo_root}/infra/cdk.json")"
  embedding_model="$(jq -r '.context.embeddingModelId' "${repo_root}/infra/cdk.json")"
  transcribe_model="${fast_model}"
  log "model catalog: deployed (real Bedrock through AWS profile ${bedrock_profile})"
else
  # `fake` is the default chat and fast model (residency au). `fake-global` has residency
  # global, so the model picker and its global label can be tested.
  models='[{"id":"fake","label":"Local fake model","vendor":"openbook","provider":"fake","modelId":"fake","residency":"au","maxOutputTokens":2000,"vision":true},{"id":"fake-global","label":"Local fake global model","vendor":"openbook","provider":"fake","modelId":"fake-global","residency":"global","maxOutputTokens":2000,"vision":true}]'
  default_chat="fake"
  fast_model="fake"
  # The local worker embeds with a hashed bag of words and transcribes with the fake model.
  embedding_model="fake"
  transcribe_model="fake"
  log "model catalog: two fake models (fake, fake-global)"
fi

{
  echo "# Written by scripts/bootstrap-local.sh. Local only. Do not commit."
  echo "OPENBOOK_STAGE=local"
  echo "OPENBOOK_TABLE_NAME=${table_name}"
  echo "OPENBOOK_DOCS_BUCKET=${docs_bucket}"
  echo "OPENBOOK_COGNITO_USER_POOL_ID=${pool_id}"
  echo "OPENBOOK_COGNITO_CLIENT_ID=${client_id}"
  echo "OPENBOOK_COGNITO_REGION=us-east-1"
  echo "OPENBOOK_COGNITO_JWKS_URL=${endpoint}/${pool_id}/.well-known/jwks.json"
  echo "OPENBOOK_MODELS=$(sq "$models")"
  echo "OPENBOOK_DEFAULT_CHAT_MODEL=${default_chat}"
  echo "OPENBOOK_FAST_MODEL=${fast_model}"
  echo "OPENBOOK_INGEST_QUEUE_URL=$(sq "$queue_url")"
  echo "OPENBOOK_VECTOR_BUCKET=${vector_bucket}"
  echo "OPENBOOK_EMBEDDING_MODEL_ID=${embedding_model}"
  echo "OPENBOOK_TRANSCRIBE_MODEL=${transcribe_model}"
  if [ -n "$pdfium_path" ]; then
    echo "OPENBOOK_PDFIUM_PATH=$(sq "$pdfium_path")"
  fi
  if [ -n "$daily_messages_per_user" ]; then
    echo "OPENBOOK_LIMIT_DAILY_MESSAGES_PER_USER=${daily_messages_per_user}"
  fi
  if [ -n "$bedrock_profile" ]; then
    echo "OPENBOOK_BEDROCK_PROFILE=$(sq "$bedrock_profile")"
  else
    # No AWS profile or credentials file can leak into the local API.
    echo "AWS_CONFIG_FILE=/dev/null"
    echo "AWS_SHARED_CREDENTIALS_FILE=/dev/null"
  fi
  echo "OPENBOOK_LISTEN_ADDR=127.0.0.1:${api_port}"
  echo "OPENBOOK_CA_BUNDLE=$(sq "$ca_pem")"
  echo "OPENBOOK_PRESIGN_ENDPOINT=http://localhost:${web_port}/__aws"
  echo "AWS_ENDPOINT_URL=${endpoint}"
  echo "AWS_REGION=us-east-1"
  echo "AWS_ACCESS_KEY_ID=test"
  echo "AWS_SECRET_ACCESS_KEY=test"
  echo "RUST_LOG=${RUST_LOG:-info}"
} | write_atomic "$env_file" 600

jq -n --arg pool "$pool_id" --arg client "$client_id" --arg endpoint "/__aws/" \
  '{stage: "local", region: "us-east-1", cognito: {userPoolId: $pool, clientId: $client, endpoint: $endpoint}}' \
  | write_atomic "$web_config" 644

log "wrote ${env_file}"
log "wrote ${web_config}"
log "done. Sign in with ${seed_users[0]} or ${seed_users[1]} (password ${seed_password})"
