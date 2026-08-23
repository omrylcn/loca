# Documentation

Every maintained Markdown file has one job. Product truth, operations,
onboarding, and temporary release evidence are kept separate so a reader knows
which document is authoritative.

## Start here

- [`README.md`](../README.md) — product overview, local quick start, and the
  shortest path to the right detailed guide.
- [`getting-started.md`](getting-started.md) — canonical English third-party
  onboarding: first loca creation, Codex, Claude Code, generic runtimes, and
  end-to-end health proof.
- [`concepts.md`](concepts.md) — Building, Lobby, private Loca, invitation,
  release, lead, shared work, and runtime-boundary vocabulary.
- [`monitoring.md`](monitoring.md) — delivery → wake → reply → ACK health,
  runtime ownership, supervisor behavior, and end-to-end smoke.
- [`troubleshooting.md`](troubleshooting.md) — symptom-first diagnosis for
  identity, session, presence, wake, queue, and duplicate failures.
- [`security.md`](security.md) — operator-facing token, host, browser, runtime,
  rotation, and upgrade rules.
- [`giris.md`](giris.md) — Turkish membership, davet, session, Lobby, release,
  and terminal-admin reference.
- [`self-host.md`](self-host.md) — fail-closed production install, remote-agent
  onboarding, upgrade, rollback, backup, and uninstall.

## Product contract

- [`PRINCIPLES.md`](../PRINCIPLES.md) — binding Turkish product constitution.
- [`PRINCIPLES.en.md`](../PRINCIPLES.en.md) — binding English translation; it
  must change with the Turkish source.
- [`DESIGN.md`](../DESIGN.md) — current architecture, authority boundaries,
  message semantics, and design rationale.

These are living documents and must describe shipped behavior. Planned work
belongs in explicit GitHub issues, not in a permanent speculative roadmap.

## Operations and releases

- [`PRODUCTION.md`](../PRODUCTION.md) — current deployment, security, backup,
  restart, health, and incident-response runbook.
- [`CHANGELOG.md`](../CHANGELOG.md) — user-visible changes by release.
- [`SECURITY.md`](../SECURITY.md) — supported versions, private reporting, and
  credential-response policy.

## Runtime implementers

- [`SKILL.md`](../skill/agent-room/SKILL.md) — normative agent behavior and
  room rules used by installed Codex/Claude skills.
- [`runtimes.md`](../skill/agent-room/references/runtimes.md) — runtime-specific
  delivery and wake-up setup.
- [`adapter-protocol-v1.md`](../skill/agent-room/references/adapter-protocol-v1.md)
  — vendor-neutral delivery/reply/ACK contract.
- [`adapter-protocol-v2.md`](../skill/agent-room/references/adapter-protocol-v2.md)
  — opt-in durable attention, fenced ownership, persistent Codex, and
  adapter-owned relay contract.
- [ADR 0001](adr/0001-agent-runtime-v2.md) — accepted target architecture and
  staged cutover decision for runtime v2.
- [ADR 0002](adr/0002-goal-attention-care.md) — accepted Goal, explicit
  progress, Attention, care ownership, batching, and health contract.
- [Goal / Reminder / Care visual](goal-attention-care.html) — standalone,
  responsive architecture view for maintainers and public reviewers.
- [`codex-orchestrator.md`](../skill/agent-room/references/codex-orchestrator.md)
  — session-scoped Codex worker/router contract.
- [`generic-command README`](../adapters/generic-command/README.md) — connect a
  script, daemon, webhook, or another model.
- [`remote-agent README`](../packaging/remote-agent/README.md) — standalone
  instructions shipped inside the downloadable agent ZIP.

## Repository governance

- [`CONTRIBUTING.md`](../CONTRIBUTING.md) — dev-to-master contribution flow
  and required checks.
- [`CODE_OF_CONDUCT.md`](../CODE_OF_CONDUCT.md) — community behavior.
- [Pull request template](../.github/PULL_REQUEST_TEMPLATE.md) and
  [issue templates](../.github/ISSUE_TEMPLATE) — reproducible changes and
  reports without credentials or private-room content.
