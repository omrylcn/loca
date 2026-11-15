# Contributing to Loca

Loca is a private coordination space for humans and agents. Changes must
preserve the product rules in [`PRINCIPLES.md`](PRINCIPLES.md) and its Turkish
translation [`PRINCIPLES.tr.md`](PRINCIPLES.tr.md).

## Before opening a change

1. Read `PRINCIPLES.md`, `DESIGN.md`, and the relevant current documentation.
2. Keep private-room boundaries and server-derived identity intact.
3. Do not commit credentials, production data, local identity files, or room
   logs.
4. For security problems, follow [`SECURITY.md`](SECURITY.md) instead of
   opening a public issue.

A pull request or issue opened on the public repository is taken into the
private development repository rather than merged where it is; see
[Contributions to the public repository](docs/public-contributions.md).

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

For agent-kit or onboarding changes, the real-server gate can also be run alone:

```bash
make onboarding-check
```

It installs the shipped ZIP into isolated Codex and Claude homes, approves the
join requests in a disposable loopback Building, and checks Lobby recall,
message delivery, listener restart, and reply sending. It is included in
`make check`; it does not prove model inference or native runtime wake.

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

Follow the [mandatory merge, deployment and release protocol](docs/release-protocol.md).
Source checks, artifact checks and automatic runtime wake are separate evidence.

In the private repository everything merges into `master`. An older `dev`
branch still exists on the remote; it is not part of this flow and nothing
is merged into it.

1. Create a short-lived branch (`feat/*`, `fix/*`, `docs/*`, `test/*`, …)
   from current `master`, or from the branch it is stacked on.
2. Open a pull request into `master`. It merges only after the gate in
   [Pull request rules](#pull-request-rules) is green, either by itself or
   as part of an integration branch (`integrate/*`) that is itself a pull
   request into `master`.
3. Delete the short-lived branch after merge.
4. Releases are not cut here. A tested `master` commit is exported to the
   public repository and released from there, as the
   [release protocol](docs/release-protocol.md) says; release tags exist
   only in the public repository.

Do not push work directly to `master`. The remote should normally contain
only `master` and branches with an open pull request.

## Pull request rules

These hold for every pull request in the private repository, stacked or not.
Each rule is followed by its reason.

1. **One change, one branch, one pull request. A change of behaviour comes
   with its tests shown failing on the code before it.** A reviewer can judge
   one thing at a time, and a test that also passes on the old code proves
   nothing about the change. A change with no behaviour to test (a document,
   a comment, a workflow that cannot run locally) says so in the pull request
   and says what was checked instead; it is not held to a failing test it
   cannot have.
2. **A reviewer's GO is for one exact SHA.** Any new commit, a rebase or a
   merge of the base included, needs the GO carried to the new SHA: the
   reviewer reads what changed since the reviewed SHA and says GO for the new
   one. What merges must be what was reviewed.
3. **Several approved pull requests open at once are merged into one
   integration branch, each at exactly its reviewed SHA.** The reviewer
   reviews only the merge resolutions, the full gate set runs once on the
   integration head, and the integration branch is what merges. The
   resolutions are the only code nobody has reviewed, and the combination is
   what ships, so that is what the gate runs on. A merge without conflicts is
   checked too: the reviewer confirms that each approved SHA is an ancestor
   of the integration head and that its tree is exactly the union of the
   approved trees (and of any resolution, named), because a merge can change
   content without a conflict and nobody would have read the difference.
4. **The integration branch is opened as its own pull request that lists
   each merged head (pull request and SHA) and its review status.** Whoever
   merges it sees in one place what goes in and on what approval.
5. **A pull request stacked on another names its base, is reviewed as the
   diff against that base, and merges only after its base.** Its review then
   covers exactly its own change, and a GO for it never approves the base
   underneath.
6. **GitHub CI runs only in the public repository. In the private one, the
   gate is the full gate set run locally in Docker, and its result is posted
   as a comment on the pull request with the tested SHA for each gate.** A
   result that is not on the pull request for that SHA does not exist for the
   merge decision.
7. **A gate failure is recorded on the pull request even when a rerun is
   green, with the evidence for why it failed. A red result on a
   constituent's own head is recorded on the integration pull request even
   when the integration head is green.** A green rerun or a green
   combination can hide a defect or a flaky test that comes back; the record
   lets the next failure be recognised (see "Evidence and enforcement" in the
   [release protocol](docs/release-protocol.md)).
8. **A failure that turns out to be a defect already on `master` gets its own
   pull request, with the evidence before and after the fix; for a race,
   the failure rate over repeated runs before and after.** It is not folded
   into the change that happened to expose it, and a race is not shown fixed
   by one green run.
9. **A timing bound relaxed in a test is stated as relaxed in the pull
   request, with what the test still proves.** A looser bound weakens a test
   silently unless someone says what it no longer checks.

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
