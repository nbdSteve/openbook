# Openbook developer commands. Run `make help` for the list.
SHELL := /bin/bash
.SHELLFLAGS := -euo pipefail -c
.DEFAULT_GOAL := help

STAGE ?= dev
ALLOW_PROD ?=

OPENBOOK_EMULATOR_PORT ?= 9297
export OPENBOOK_EMULATOR_PORT

# Run a command in a directory only when the directory exists. The parts of the repository
# arrive in phases, so `make check` says "skipped" instead of failing on a missing part.
# usage: $(call part,<name>,<path that must exist>,<commands>)
define part
@if [ -e "$(2)" ]; then \
  echo "==> $(1)"; \
  $(3); \
else \
  echo "==> $(1): skipped ($(2) does not exist yet)"; \
fi
endef

# Run `npm ci` in a directory when node_modules is missing or package-lock.json changed since
# the last install. A stamp file in node_modules holds the checksum of the lockfile that was
# installed, so an unchanged lockfile prints nothing and costs nothing.
# usage: $(call npm_install,<dir>)
define npm_install
stamp="$(1)/node_modules/.openbook-lockfile"; \
want="$$(cksum < "$(1)/package-lock.json")"; \
if [ ! -d "$(1)/node_modules" ] || [ "$$(cat "$$stamp" 2>/dev/null)" != "$$want" ]; then \
  echo "--> npm ci ($(1)): package-lock.json changed or node_modules is missing"; \
  npm --prefix "$(1)" ci; \
  echo "$$want" > "$$stamp"; \
fi
endef

# Install npm dependencies when needed, then run the scripts.
define npm_check
@if [ -f "$(1)/package.json" ]; then \
  echo "==> $(2)"; \
  $(call npm_install,$(1)); \
  for script in $(3); do echo "--> npm run $$script ($(1))"; npm --prefix "$(1)" run "$$script" || exit 1; done; \
else \
  echo "==> $(2): skipped ($(1)/package.json does not exist yet)"; \
fi
endef

.PHONY: help
help: ## Show this list
	@echo "Openbook commands:"
	@awk 'BEGIN { FS = ":.*## " } /^[a-zA-Z0-9_-]+:.*## / { printf "  %-18s %s\n", $$1, $$2 }' $(MAKEFILE_LIST)
	@echo
	@echo "Variables: STAGE=dev|prod (deploy), ALLOW_PROD=1 (needed for STAGE=prod, with OPENBOOK_CERTIFICATE_ARN and OPENBOOK_HOSTED_ZONE_ID), OPENBOOK_EMULATOR_PORT (default 9297)."

.PHONY: check
check: ## Run every formatter check, linter, type check, and unit test
	$(call part,rust fmt,Cargo.toml,cargo fmt --all --check)
	$(call part,rust clippy,Cargo.toml,cargo clippy --workspace --all-targets -- -D warnings)
	$(call part,rust test,Cargo.toml,cargo test --workspace)
	$(call npm_check,apps/ask,web,lint typecheck test build)
	$(call npm_check,infra,infra,lint typecheck build test)
	@echo "==> shellcheck"
	@if command -v shellcheck > /dev/null; then \
	  shellcheck -x scripts/*.sh scripts/lib/stack.sh infra/scripts/*.sh; \
	elif [ -n "$${CI:-}" ]; then \
	  echo "error: shellcheck is not installed" >&2; exit 1; \
	else \
	  echo "shellcheck: skipped (not installed; CI runs it)"; \
	fi

.PHONY: fmt
fmt: ## Format the Rust code
	$(call part,rust fmt,Cargo.toml,cargo fmt --all)

.PHONY: eval
eval: ## Retrieval evaluation, offline, checked against evals/baseline.json and evals/benchmarks.json, and the README tables (no network)
	cargo run -q -p openbook-eval -- offline --check
	cargo run -q -p openbook-eval -- readme --check

.PHONY: benchmarks
benchmarks: ## Write the benchmark tables of evals/benchmarks.json into README.md (no network)
	cargo run -q -p openbook-eval -- readme

.PHONY: eval-dev
eval-dev: ## Retrieval evaluation on the development cache (shows that the harness works; never a baseline)
	cargo run -q -p openbook-eval -- offline --dev-cache

.PHONY: eval-live
eval-live: ## Refresh the caches of all corpora with REAL Bedrock calls: needs AWS_PROFILE. It prints the calls and the cost; add YES=1 to start
	@test -n "$${AWS_PROFILE:-}" || { echo "error: set AWS_PROFILE (this calls real AWS)" >&2; exit 1; }
	OPENBOOK_PDFIUM_PATH="$$(scripts/fetch-pdfium.sh | tail -1)" cargo run -q --release -p openbook-eval -- refresh $(if $(YES),--yes,)

.PHONY: eval-grid
eval-grid: ## Run the whole retrieval tuning grid offline; writes target/eval/grid.md and grid.json (no network)
	cargo run -q -p openbook-eval -- grid

.PHONY: eval-live-missing
eval-live-missing: ## Fill what the caches of all corpora lack with REAL Bedrock calls (a corpus with no cache gets the full refresh): needs AWS_PROFILE. It prints the calls and a cost bound; add YES=1 to start
	@test -n "$${AWS_PROFILE:-}" || { echo "error: set AWS_PROFILE (this calls real AWS)" >&2; exit 1; }
	OPENBOOK_PDFIUM_PATH="$$(scripts/fetch-pdfium.sh | tail -1)" cargo run -q --release -p openbook-eval -- refresh --missing-only $(if $(YES),--yes,)

.PHONY: test-integration
test-integration: ## Rust integration tests against AWS Emulator (port 9320)
	$(call part,rust integration tests,Cargo.toml,scripts/test-integration.sh)

.PHONY: emulator-up
emulator-up: ## Start the AWS Emulator container (build the image if needed)
	scripts/emulator.sh up $(OPENBOOK_EMULATOR_PORT)

.PHONY: emulator-down
emulator-down: ## Stop the AWS Emulator container
	scripts/emulator.sh down $(OPENBOOK_EMULATOR_PORT)

.PHONY: bootstrap
bootstrap: ## Create the local resources and write .local/dev/api.env and .local/dev/web-config.json
	scripts/bootstrap-local.sh --emulator-port $(OPENBOOK_EMULATOR_PORT)

.PHONY: dev
dev: ## Run the whole local stack (emulator, API, web); Ctrl-C stops it
	scripts/dev.sh

.PHONY: e2e-local
e2e-local: ## Run the Playwright flows against a fresh, isolated local stack
	scripts/e2e-local.sh

# The demo stage is local unless STAGE is set on the command line or in the environment.
DEMO_STAGE := $(if $(filter command line environment,$(origin STAGE)),$(STAGE),local)

.PHONY: demo-provision
demo-provision: ## Create or refresh the public demo course (STAGE=local by default; dev or prod need AWS_PROFILE; prod needs ALLOW_PROD=1; FORCE=1 rebuilds)
	ALLOW_PROD="$(ALLOW_PROD)" scripts/demo-provision.sh $(DEMO_STAGE) $(if $(FORCE),--force,)

.PHONY: build-lambdas
build-lambdas: ## Build the API and ingest Lambdas for arm64, with libpdfium.so next to the ingest bootstrap
	scripts/build-lambdas.sh

.PHONY: build-web
build-web: ## Build the web app (apps/ask/dist)
	@$(call npm_install,apps/ask)
	npm --prefix apps/ask run build

.PHONY: synth
synth: ## Synthesize the CDK app for STAGE (needs built Lambda and web assets)
	@$(call npm_install,infra)
	cd infra && npx cdk synth --quiet -c stage=$(STAGE)

# A stage with a custom domain (infra/cdk.json, stageDomains) takes its certificate and hosted
# zone as stack parameters. They are passed whenever both variables are set.
STAGE_DOMAIN = $(shell node -p "((require('./infra/cdk.json').context.stageDomains||{})['$(STAGE)']||{}).domainName||''" 2>/dev/null)
DEPLOY_PARAMETERS = $(if $(and $(OPENBOOK_CERTIFICATE_ARN),$(OPENBOOK_HOSTED_ZONE_ID)),--parameters "Openbook-$(STAGE):CertificateArn=$$OPENBOOK_CERTIFICATE_ARN" --parameters "Openbook-$(STAGE):HostedZoneId=$$OPENBOOK_HOSTED_ZONE_ID")

.PHONY: deploy
deploy: ## Build and deploy Openbook-<stage> with your AWS_PROFILE (STAGE=dev by default)
	@case "$(STAGE)" in \
	  dev) ;; \
	  prod) \
	    if [ "$(ALLOW_PROD)" != "1" ]; then \
	      echo "error: refusing to deploy prod from a laptop. CI deploys prod. Set ALLOW_PROD=1 to override." >&2; exit 1; \
	    fi ;; \
	  *) echo "error: STAGE must be dev or prod, got '$(STAGE)'" >&2; exit 1 ;; \
	esac
	@if [ -z "$${AWS_PROFILE:-}" ] && [ -z "$${AWS_ACCESS_KEY_ID:-}" ]; then \
	  echo "error: set AWS_PROFILE (or AWS credentials) for the account to deploy to" >&2; exit 1; \
	fi
	@if [ -n "$(STAGE_DOMAIN)" ] && { [ -z "$${OPENBOOK_CERTIFICATE_ARN:-}" ] || [ -z "$${OPENBOOK_HOSTED_ZONE_ID:-}" ]; }; then \
	  echo "error: $(STAGE) has the custom domain $(STAGE_DOMAIN) in infra/cdk.json. Set OPENBOOK_CERTIFICATE_ARN (an ACM certificate in us-east-1) and OPENBOOK_HOSTED_ZONE_ID (a Route 53 zone ID)." >&2; exit 1; \
	fi
	$(MAKE) build-lambdas build-web
	@$(call npm_install,infra)
	cd infra && npx cdk deploy Openbook-$(STAGE) -c stage=$(STAGE) $(DEPLOY_PARAMETERS)
