#!/usr/bin/env bash
# Synthesize with the fixture assets and no AWS identity: every AWS_* variable and
# CDK_DEFAULT_* variable is dropped, so the synth cannot pick up local credentials, a
# profile, or an account. The template then uses account pseudo parameters.
#
#   scripts/synth-placeholders.sh [extra cdk synth arguments, for example -c stage=prod]
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

while IFS= read -r name; do
  unset "$name"
done < <(env | sed -n 's/^\(AWS_[A-Za-z0-9_]*\|CDK_DEFAULT_[A-Za-z0-9_]*\)=.*/\1/p')

# The CDK CLI looks up the default account through the SDK credential chain. Point the
# chain at nothing, so that it makes no AWS call and finds no account.
export AWS_CONFIG_FILE=/dev/null
export AWS_SHARED_CREDENTIALS_FILE=/dev/null
export AWS_EC2_METADATA_DISABLED=true

exec npx cdk synth --quiet \
  -c apiAssetDir=test/fixtures/api -c ingestAssetDir=test/fixtures/ingest -c webAssetDir=test/fixtures/web "$@"
