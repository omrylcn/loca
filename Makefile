SHELL := /bin/bash
.DEFAULT_GOAL := help

PYTHON ?= python3

SHELL_SCRIPTS := \
	skill/agent-room/connect.sh \
	skill/agent-room/runtime.sh \
	packaging/remote-agent/install.sh \
	packaging/remote-agent/start-lobby.sh \
	packaging/remote-agent/stop-lobby.sh \
	packaging/remote-agent/status.sh \
	scripts/build-remote-agent-kit.sh \
	scripts/architecture-baseline.sh \
	scripts/check-release.sh \
	scripts/require-release-workflows.sh \
	scripts/init-self-host.sh \
	scripts/version.sh \
	scripts/smoke.sh

.PHONY: help check rust-check python-check shell-check docs-check \
	compose-check smoke package-check container-check browser-check benchmark-local \
	runtime-v2-soak delivery-check onboarding-check

help:
	@printf '%s\n' \
		'make check           Run the same non-container gates as CI' \
		'make container-check Build the production container' \
		'make browser-check   Run the Chromium identity/reconnect gate' \
		'make delivery-check  Run real all/lead reminder delivery gates' \
		'make benchmark-local Measure disposable local SQLite message paths' \
		'make package-check   Build and verify the remote-agent ZIP' \
		'make onboarding-check Test fresh ZIP onboarding against a real server' \
		'make wiki-protocol-check Test Wiki queue/transport/deltas (fixture worker, not native model)' \
		'make wiki-native-check WIKI_RUNTIME=codex|claude Opt-in real model Wiki worker/reader gate' \
		'make runtime-v2-soak Historical test-fixture comparison (not a supported v1 runtime)'

check: rust-check python-check shell-check docs-check compose-check smoke package-check onboarding-check wiki-protocol-check critical-integrity-check

.PHONY: critical-integrity-check
critical-integrity-check:
	cargo build -p server
	$(PYTHON) scripts/critical-integrity-e2e.py

rust-check:
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets -- -D warnings
	cargo test --workspace
	$(PYTHON) scripts/check-panic-surface.py
	./scripts/check-release.sh

python-check:
	$(PYTHON) -m unittest discover -s skill/agent-room/tests -p 'test_*.py'
	$(PYTHON) -m unittest discover -s skill/loca-care/tests -p 'test_*.py'
	$(PYTHON) -m py_compile scripts/audit-pending-replies.py scripts/conformance-runtime-v2.py scripts/conformance-late-commentary.py \
		scripts/soak-runtime-v2-shadow.py scripts/benchmark-local.py scripts/wiki-native-e2e.py scripts/wiki-runtime-e2e.py scripts/wiki-autonomy-e2e.py

shell-check:
	bash -n $(SHELL_SCRIPTS)
	shellcheck $(SHELL_SCRIPTS)

docs-check:
	$(PYTHON) scripts/check-doc-links.py
	git diff --check

compose-check:
	@if env -u ADMIN_TOKEN -u PUBLIC_SERVER_URL \
		docker compose -f docker-compose.yml config >/dev/null 2>&1; then \
		echo "production compose accepted missing secrets" >&2; \
		exit 1; \
	fi
	ADMIN_TOKEN=adm_ci_only \
		PUBLIC_SERVER_URL=https://loca.example.com \
		docker compose -f docker-compose.yml config >/dev/null
	docker compose -f compose.dev.yml config >/dev/null

smoke:
	./scripts/smoke.sh

onboarding-check:
	cargo build -p server
	$(PYTHON) scripts/onboarding-e2e.py

.PHONY: wiki-protocol-check
wiki-protocol-check:
	cargo build -p server
	$(PYTHON) scripts/wiki-protocol-e2e.py

.PHONY: wiki-native-check
# Explicit opt-in: ordinary make check must never spend model credits or require
# a developer's native login. A missing runtime/login is failure, not a skip/GO.
WIKI_RUNTIME ?= codex
WIKI_NATIVE_ARGS ?=
wiki-native-check:
	cargo build -p server
	$(PYTHON) scripts/wiki-native-e2e.py --runtime $(WIKI_RUNTIME) $(WIKI_NATIVE_ARGS)

.PHONY: wiki-runtime-check
# Shipped runner with a natural drafting prompt, not the component-test oracle.
# Native --binary/--node flags differ from the reader component gate's flags.
WIKI_RUNTIME_ARGS ?=
wiki-runtime-check:
	cargo build -p server
	$(PYTHON) scripts/wiki-runtime-e2e.py --runtime $(WIKI_RUNTIME) $(WIKI_RUNTIME_ARGS)

.PHONY: wiki-autonomy-check
# Native model credits/login are required. A short run is only preflight;
# --soak-seconds 10800 exercises periodic native replies and durable restart.
WIKI_AUTONOMY_ARGS ?=
wiki-autonomy-check:
	cargo build -p server
	$(PYTHON) scripts/wiki-autonomy-e2e.py --runtime $(WIKI_RUNTIME) $(WIKI_AUTONOMY_ARGS)

package-check:
	./scripts/build-remote-agent-kit.sh
	@stage=$$(mktemp -d); \
	trap 'rm -rf "$$stage"' EXIT; \
	unzip -q dist/loca-remote-agent.zip -d "$$stage"; \
	cd "$$stage/loca-remote-agent"; \
	sha256sum -c SHA256SUMS

container-check:
	docker build --tag loca:local-check .

browser-check:
	npm run test:browser

DELIVERY_BINARY ?= target/debug/room-server
DELIVERY_LISTENER ?= skill/agent-room/listen.py
DELIVERY_PORT ?= 18990

delivery-check:
	$(PYTHON) scripts/delivery-harness.py --binary $(DELIVERY_BINARY) \
		--listener $(DELIVERY_LISTENER) --agents alice,bob,carol \
		--lead alice --recipient all --supervised --restart --port $(DELIVERY_PORT)
	$(PYTHON) scripts/delivery-harness.py --binary $(DELIVERY_BINARY) \
		--listener $(DELIVERY_LISTENER) --agents alice,bob,carol \
		--lead alice --recipient lead --supervised --restart --port $$(( $(DELIVERY_PORT) + 1 ))

benchmark-local:
	$(PYTHON) scripts/benchmark-local.py

runtime-v2-soak:
	@state_dir=".runtime-v2-soak/$$(date -u +%Y%m%dT%H%M%SZ)"; \
	mkdir -p "$$state_dir"; \
	./scripts/soak-runtime-v2-shadow.py \
		--duration-seconds 10800 \
		--interval-seconds 600 \
		--state-dir "$$state_dir" \
		--output "$$state_dir/result.json"
