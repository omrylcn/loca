# Professional review: fixes and acceptance evidence

## Scope and release status

This change set addresses R01–R15 in the operator's professional review dated
2026-10-03. That review tested v0.9.28; implementation and the first full local
verification used the public product tree at
`ea935a63e9c2517441c4b2e7c608cf5b0b3c708f` (v0.9.29).
For private CI/review, the change commit is based on private master
`fe5f8db5b146baf04558cab1ad3ef615fec660c0` (same server/agent/web product code).
Private operations documents are not candidates for public sync.
Branch: `fix/professional-review-20261003`.

The original development worktree, including its unresolved merge, was not
modified. The fixes were prepared and independently reviewed as the
**0.10.0 release candidate** before publication.
This is the implementer's evidence, not an independent review approval.

## Finding-by-finding disposition

| Finding | Change | Regression evidence |
| --- | --- | --- |
| R01: claimed Journal author / sessionless note deletion | A shared mutation actor resolver uses the valid session identity, rejects supplied invalid sessions even in optional-session sandboxes, and enforces required sessions. | Real HTTP missing/invalid/valid actor tests; expired and revoked session tests. |
| R02: same-name principal inherits memory ownership | Persist `owner_principal_id`; write authority checks the principal, not the label. Migration binds only an unambiguous, active historical identity. Ambiguous/unbound closed-Building owners require explicit reassignment. | Real HTTP revoke/re-admit same-name test (new identity denied until reassignment); migration tests for unique, missing, and reused identities. |
| R03: environment overflow | Full delivery travels through a private seekable file/stdin. Oversized compatibility environment fields are omitted together; metadata remains available. | A real child receives four large Unicode messages byte-for-byte as the original envelope. |
| R04: blocked stdin before timeout | No blocking pipe write; the bounded process deadline starts before payload preparation and process creation. | A non-reading child with a large payload times out without deadlock. |
| R05: checkpoint/ACK ahead of durable output | File sinks flush and fsync before checkpoint/ACK; cursor replace syncs its parent on POSIX. On restart, an incomplete final JSONL record is removed before replay/append. | Fault-injected fsync failure, file/replace/directory ordering, torn multibyte-tail repair, reconnect tests. |
| R06: database-only backup | Add offline SQLite + indexed blob snapshots, checksums, integrity checks, private permissions, and verified restore to a new directory. Package the tool with server artifacts. | Missing/corrupt blob, tampered manifest, symlink, overwrite, and malformed-manifest rejection; real HTTP upload → shutdown → snapshot → empty restore → fresh server → history/download/hash E2E. |
| R07: finite ledger has no rollover | Choose the review's documented finite-ledger option, not a new consolidation mechanism. Remove the nonexistent consolidate instruction from errors; explain the 64 KiB append-only cap and continued read access. | Budget rejection tests and visible UI/docs policy. No reset or generation API is claimed. |
| R08: recovered wake has stale memory | Add an authenticated bounded memory snapshot endpoint; fetch it before recovered wakes and use its observed revision. Older queued WS memory frames cannot roll it back. | Reconnect test changes memory while offline and asserts that the first recovered turn contains the new version and text; cross-room snapshot access denied. |
| R09: failed Notes save loses draft | Track drafts by room/key, bind editing to the rendered editor identity, guard room generations, keep failed mutations visible, and prevent duplicate saves. | Real browser/API room switch and archived-room 409; injected 401/413/503 preserve create drafts. |
| R10: read failure becomes empty/Free state | Propagate storage read errors; corrupt persisted policy fails startup closed. HTTP reads return errors, not empty success. Note revision + replacement is transactional. | Corrupt settings/mode and dropped-table probes; failed replacement rolls back its revision. |
| R11: Unicode/literal search mismatch | Use the same Unicode-lowercase normalization for message and note search; SQL substring matching treats percent and underscore literally. | Turkish uppercase/lowercase and literal wildcard tests. This is not a claim of full Unicode normalization or language-independent case folding. |
| R12: stale onboarding pins | Align installation pins with the canonical released version and add a version-drift documentation gate, including embedded docs. | Deliberately stale onboarding fails the gate; current onboarding passes. |
| R13: Memory never leaves Loading | Separate request loading from authoritative domain state; clear loading after the response without requiring another WS frame. | Real browser/server persisted-memory panel opens on its first GET. |
| R14: unfocused Memory draft overwritten | Preserve dirty drafts independently of focus, retain their base revision, and warn on concurrent changes. | Real browser + remote API mutation + real WS frame leaves the local draft intact. |
| R15: accepted save falsely verified | Treat save acceptance and readback verification separately. Match readback text/provenance; serialize panel writes against background reads, clear recovered read errors, and ignore obsolete-room responses. | Real owner short/decision writes; injected post-save GET 503 produces an honest unverified-save result; later successful refresh clears the error. |

## Verification

Run from this branch:

```bash
make check
make browser-check
make container-check
make delivery-check
```

Local results on Linux:

- Rust workspace: **253 passed**, none failed or ignored; format and Clippy
  with warnings denied passed.
- Agent Python suite: **286 passed**; caretaker suite: **4 passed**.
- Browser Chromium suite: **83 passed**. The new content reliability suite uses
  a real server, persistent SQLite, and real WS updates. Specified network-error
  cases use controlled browser fault injection rather than claiming a naturally
  occurring outage.
- Shell syntax/lint, documentation links/version pins, panic-surface budget,
  production Compose fail-closed configuration, local smoke, and remote-agent
  package checksums passed.
- Production Docker image built locally; this is a build result, not a deploy.
- Real supervised listeners passed `all` and `lead` reminder recipient checks
  before and after a listener restart. This proves those delivery paths, not
  model-generated replies or all runtime adapters.

The test count is not a line-coverage percentage. A physical power-loss test,
Windows/macOS/native desktop checks, fresh-machine real Codex/Claude model
conformance, and multi-hour model soak were **not** performed in this change set.
POSIX file ordering tests are not a claim that every filesystem/hardware cache
honors durability identically. Backup requires all writers to be stopped;
`--server-stopped` records operator confirmation rather than detecting processes.

## Publication evidence

Publication required independent review and exact-commit CI/security gates.
Subsequent releases must continue to follow the repository contract in
AGENTS.md: development and verification in the private repository; public
allowlist sync and leak/tree checks; and user-facing tags/artifacts only in the
approved public repository. Verify downloaded artifacts/checksums and a
clean-machine install. The older v0.9.29 artifacts do not contain these fixes.
