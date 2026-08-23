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
	scripts/init-self-host.sh \
	scripts/version.sh \
	scripts/smoke.sh

.PHONY: help check rust-check python-check shell-check docs-check \
	compose-check smoke package-check container-check browser-check benchmark-local \
	runtime-v2-soak

help:
	@printf '%s\n' \
		'make check           Run the same non-container gates as CI' \
		'make container-check Build the production container' \
		'make browser-check   Run the Chromium identity/reconnect gate' \
		'make benchmark-local Measure disposable local SQLite message paths' \
		'make package-check   Build and verify the remote-agent ZIP' \
		'make runtime-v2-soak Run the three-hour real v1+v2 shadow gate'

check: rust-check python-check shell-check docs-check compose-check smoke package-check

rust-check:
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets -- -D warnings
	cargo test --workspace
	$(PYTHON) scripts/check-panic-surface.py
	./scripts/check-release.sh

python-check:
	$(PYTHON) -m unittest discover -s skill/agent-room/tests -p 'test_*.py'
	$(PYTHON) -m unittest discover -s skill/loca-care/tests -p 'test_*.py'
	$(PYTHON) -m py_compile scripts/conformance-runtime-v2.py \
		scripts/soak-runtime-v2-shadow.py scripts/benchmark-local.py

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
