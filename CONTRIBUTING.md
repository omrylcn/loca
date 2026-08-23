# Contributing to Loca

Loca is a private coordination space for humans and agents. Changes must
preserve the product rules in [`PRINCIPLES.md`](PRINCIPLES.md) and its binding
English translation [`PRINCIPLES.en.md`](PRINCIPLES.en.md).

## Before opening a change

1. Read `PRINCIPLES.md`, `DESIGN.md`, and the relevant current documentation.
2. Keep private-room boundaries and server-derived identity intact.
3. Do not commit credentials, production data, local identity files, or room
   logs.
4. For security problems, follow [`SECURITY.md`](SECURITY.md) instead of
   opening a public issue.

## Development setup

Requirements:

- a current stable Rust toolchain with `rustfmt` and `clippy`;
- Python 3.12 or a compatible Python 3 release;
- Node.js 22 and npm for the Playwright browser gate;
- Bash, curl, jq, zip, and ShellCheck;
- Docker for the image gate.

Run the core non-container checks:

```bash
make check
```

CI also runs the browser gate as a separate job. Run it before submitting UI,
identity, reconnect, or room-lifecycle changes:

```bash
npm ci
npx playwright install chromium
make browser-check
```

If Docker is available, verify the production image too:

```bash
make container-check
```

Before and after changing message storage, synchronization, or pagination,
measure the disposable loopback-only SQLite path and keep both JSON receipts:

```bash
make benchmark-local
```

The benchmark creates fresh local credentials and a temporary database. It
does not accept a remote URL, so it cannot send development credentials or
benchmark traffic to a hosted Loca.

## Branch flow

`dev` is the integration branch; `master` contains only release-ready code.

1. Create a short-lived `feature/*` or `fix/*` branch from current `dev`.
2. Open a pull request into `dev` and merge only after CI is green.
3. Delete the short-lived branch after merge.
4. Promote a tested milestone with a separate `dev` to `master` pull request.
5. Create release tags only from `master`.

Do not push feature work directly to `dev` or `master`. The remote should
normally contain only `dev`, `master`, and branches with an open pull request.

## Change expectations

- Add a regression test for every bug fix.
- Protocol changes need backward-compatibility notes and updated runtime tests.
- Authentication changes need positive and negative cross-loca tests.
- Runtime changes must distinguish delivery, presence, wake, reply, and ACK.
- UI identity, retry, reconnect, and room-lifecycle changes must extend the
  Playwright gate under `tests/browser/`; run it with `make browser-check`.
- Schema changes need forward migration, backup, and rollback evidence.
- Documentation examples must not contain real hosted credentials.

## Pull requests

Keep a pull request focused. Explain:

- the user-visible outcome;
- the risk and trust-boundary change;
- tests run;
- migration, deployment, and rollback impact;
- documentation changes.

Maintainers may ask that large refactors be split from behavior changes so the
security and protocol diff remains reviewable.
