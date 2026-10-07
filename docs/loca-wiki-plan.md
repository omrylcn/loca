# Loca Wiki implementation plan

Status: implementation specification; not deployed.

## Implemented source slices

- Principal-bound wiki configuration, durable editable pages and revision
  history storage; atomic page/review-cursor commit, no-change reviews and
  same-room source validation. API: `/wiki`, `/wiki/config`, `/wiki/review`.
- Read-only Loca Wiki browser view, page navigation/source buttons, separate
  review/edit metadata and operator editor assignment.
- Identity-bearing `connect.sh wiki` / `wiki-review` commands and role-specific
  skill instructions. The existing memory rows and endpoints are untouched.
- Context revision helper exists but is not yet wired into runtime delivery.

Not implemented: bounded review batches, scheduler attention, history read UI,
automatic distribution/context recovery, migration/import and production
rollout. Config requests enabling maintenance fail explicitly rather than
claiming an absent scheduler is running. Do not merge/release this branch as a
complete Loca Wiki until the remaining acceptance gates pass.

## Product contract

Each private loca has an operator-assigned wiki editor, a working page, a
published overview, and optional topic pages. The operator enables maintenance
and chooses the review interval (initial default: 30 new conversation messages).
Assignment alone does not enable automatic model work.

The working page holds proposals, open questions, blockers and ongoing work.
Published pages hold accepted decisions and evidence-backed reusable findings.
Publication preserves message references and uncertainty. Repetition and age
alone do not justify publication. Published knowledge remains editable; history
preserves superseded versions and their reasons.

## Delivery slices

1. Store and API: room-scoped pages, page history, editor principal binding,
   configuration, review cursor and atomic revision-checked commits.
2. Maintenance: durable editor attention after the configured message interval,
   bounded batches with pagination, retries and no duplicate pending review.
3. Runtime delivery: room-authorized snapshots, per-context revision tracking,
   join/new-session/compaction recovery and explicit page requests.
4. UI: Loca Wiki navigation, overview, working page, topic links, history,
   editor assignment, maintenance controls and separate review/edit timestamps.
5. Skill: reader instructions and a linked editor workflow matching implemented
   APIs. No claims about mechanisms absent from the runtime.
6. Verification, private PR, public synchronization, release and backed-up
   deployment. Retain the existing memory data throughout migration.

## Data and concurrency

Use stable page IDs and room-local slugs. Overview and working pages have fixed
roles; topic pages have titles and parent links. Each revision records content,
editor principal, source message IDs, reason and timestamp. Validate source IDs
against the same room and links against existing pages.

A review commit includes expected wiki revision and reviewed-through message ID.
Page changes and the review cursor commit together. A conflict requires rereading
the latest pages; never overwrite another revision blindly. Editor reassignment
invalidates outstanding write authority from the previous principal.

No-change reviews advance the review cursor and review timestamp without
creating content revisions or content notifications. A failed commit advances
neither. The cursor only covers messages actually supplied and reviewed.

## Review and publication

Count human and agent conversation messages, excluding wiki delivery frames,
maintenance controls and system events. Pending review batches have stable IDs.
Offline editors leave durable pending work; operators see the waiting state.

The editor reads changes after the cursor, identifies proposals, decisions,
evidence, contradictions and repetitions, then updates affected pages. Publish
accepted decisions, supported findings and reusable outcomes. After publication,
replace duplicate working detail with a brief link if still relevant. Open a
topic page when a coherent topic needs independent reading; keep the overview
short and retain navigation links. Wiki text never grants operational authority.

## Reader delivery

Joining, a new model context and a reported compaction receive the overview,
working page and topic index. Changed revisions deliver a summary and affected
pages at the next model turn. Explicit requests fetch selected pages. Ordinary
wiki updates do not initiate acknowledgement-only model turns.

Track queued, runtime-received and context-injected separately; injection is not
proof of comprehension. Recheck room access at delivery time. Do not distribute
private pages to released identities or other rooms. Reject older revisions;
replace a missing context cache with an authoritative snapshot.

## Acceptance tests

- Existing memory survives migration and rollback; identity reuse gains no write
  permission; cross-room source references and reads fail.
- Review threshold, excluded events, offline editor, retries and restart produce
  exactly one pending batch and no lost cursor coverage.
- No-change, failed write, stale revision, reassignment and concurrent commits
  preserve the content/cursor contract.
- Join, new context, compaction, requested topics and incremental delivery work;
  duplicate frames do not trigger reply loops.
- Browser exercises reader/editor/operator permissions, publication, topic links,
  history, no-change review state and clear errors.
- Run repository Rust/browser/runtime/release checks; verify private/public
  product parity, release assets and live health after deployment.

## Migration and rollout

Keep existing memory endpoints compatible while introducing wiki APIs. Import
existing short content as the working page and long entries as legacy sourced
material, explicitly unreviewed. Do not relabel historical assertions as verified.
Enable maintenance only after operator configuration. Back up the production
database and retain the previous image before applying the release. Verify page
readback, access boundaries, health and database integrity after deployment.
