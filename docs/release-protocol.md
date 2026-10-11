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
   changed code. Keep releases draft until all applicable gates pass.
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
