# Mandatory merge, deployment and release protocol

No merge, deployment or release without evidence for the exact candidate SHA.
Passing source tests or checksums alone does not prove installed functionality.

## Merge gate

- Review the diff, compatibility, credential handling and regression tests.
- Require successful quality, container, browser and secret-scan CI on the
  current PR head. Missing, skipped or stale required results are not passes.
- Run `make check`; it includes `make onboarding-check`. The onboarding gate
  builds the ZIP, installs it in empty Codex and Claude homes and uses a real,
  disposable loopback server to verify approval, idempotent onboarding, Lobby
  recall, delivery, listener restart and message sending.
- Preserve installed identities and credentials. Incomplete packages must fail
  before replacing an existing installation.
- Test that the gate rejects the known missing-helper regression, not only
  that the fixed candidate passes. Use the canonical skill bundle manifest.

## Authority and data-boundary gate

A change to who may enter, read, moderate or move a loca, or to which records
follow a loca, needs a negative test for the boundary it touches: a request
that must be refused, checked by its status code and by state that did not
change. Report for each test whether it fails on the unfixed code. One that
also passes there guards the outcome; it is not evidence for the fix.

These suites hold the boundaries. All of them run in the quality job through
`cargo test --workspace`; the browser surface runs in `make browser-check`.

| Boundary | Suite |
|---|---|
| Private pair: its two people, and the Master only as a witness one of them called in, with history only when both allow it. Nobody else, by rank, id spelling, console or scoped davet | `crates/server/tests/ws/birebir.rs`, `crates/server/tests/ws/review_regressions.rs` |
| Group loca: its members and the Master. Anyone else, a Smaster outside the group included, gets the refusal a missing loca gets. Moves and membership changes keep that boundary | `crates/server/tests/ws/groups.rs`, `crates/server/tests/ws/review_regressions.rs` |
| Rank over rank: moderation and davet provenance, across a restart | `crates/server/tests/ws/review_regressions.rs`, `crates/server/tests/ws/access.rs` |
| Membership, credentials and keys | `crates/server/tests/ws/membership.rs`, `crates/server/tests/ws/keys.rs` |
| An extension that is off | `crates/server/tests/ws/extensions.rs` |
| Notification audience | `crates/server/tests/ws/notifications.rs` |
| Records that follow a loca through a rename | `crates/server/src/store/tests.rs` |
| What the browser shows and allows | `tests/browser/` |

The quality, container, browser and secret-scan jobs start by themselves only
for a pull request into `master` or `dev`. The dependency audit has its own
triggers: any pull request that changes a Cargo manifest or lockfile, pushes
to `master`, and a weekly run. A pull request stacked on another branch
therefore lacks the required quality, container, browser and secret-scan
results, and under the merge gate a missing required result is not a pass.

What starts those jobs is the target branch, not the merge of the branch
underneath. Retargeting a stacked pull request to `master` runs them on that
head; it also widens the pull request to everything its former base had not
merged. The base keeps its own decision, on its own exact SHA and gates: a
review pass on the stacked change does not approve it.

## Distinct runtime gate

Installation and message transport do not prove automatic model wake-up.
For changes affecting adapters, wake, reply relay or ACK, separately verify:
direct attention -> durable delivery -> exactly one model turn -> one accepted
room reply -> ACK advancement. Record runtime identity, candidate SHA and
delivery/response evidence. No manual send or fixture responder may stand in
for a model reply. Untested paths remain unverified and block claims or
publication of affected runtime functionality.

## Release and deployment gates

1. Verify required workflow success for the exact source SHA, version surfaces,
   public/private runtime-content parity and rollback readiness.
2. Test the exact candidate artifact, not a newly rebuilt substitute: ZIP
   content/manifest parity, checksums, fresh installation and real-server
   onboarding. Use `scripts/onboarding-e2e.py --binary PATH --archive FINAL_ZIP`
   to test without rebuilding. The release workflow runs this against the
   final ZIP and release server before publication. Desktop artifacts also
   require their platform package tests.
3. Require publication jobs to depend on these successful gates. Do not
   override failures, silently replace released assets or reuse a version for
   changed code. Keep releases draft until all applicable gates pass. The two
   release workflows differ in where that ends. The server workflow creates a
   draft and stops: publishing it (`gh release edit TAG --draft=false`) is a
   separate, approved step taken after step 5's checks have been run on the
   draft's assets, and `scripts/check-release.sh` refuses a server workflow
   that would publish directly. The desktop workflow creates a draft, gates
   the packages on it and then publishes it itself in its last job; a desktop
   tag is therefore a decision to publish if those gates pass.
4. Deploy only the tested artifact/SHA, retain rollback and verify live health,
   version and SHA. A release approval does not automatically authorize deploy.
5. Re-download published assets, verify checksums and repeat installation
   checks. Report source, artifact, installed code and live service separately.

## Evidence and enforcement

Record command/job URL, SHA, artifact digest, result and tested boundary.
`FAIL`, `MISSING` and `INCONCLUSIVE` block the applicable action; they cannot
be relabelled green because other tests passed. Do not copy live status or
test totals into durable memory; store the rule and evidence location.

A written rule is not a technical interlock. Verify branch protection and
workflow dependencies through their actual configuration, including admin
bypass. If plan/permissions prevent enforcement, report that limitation.
Privileged direct deployment access needs separate restrictions. Never claim
all routes are protected without measuring those controls.
