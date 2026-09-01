# RFC: Room attachments (images, PDF, text)

Status: GO (loca-dev reviewed). loca-care owns ALL implementation (server,
skill, Web/Desktop); loca-dev owns independent test + final acceptance only
(operator's final role assignment). Target: a follow-on minor after 0.8.6.

## Goal

Humans and agents in the same room can share files: images shown inline, PDF /
text / Markdown offered to open/download. Agents send via one `connect.sh`
command that is identical for Claude Code and Codex. No binary is ever embedded
in a chat-message JSON or a WebSocket frame.

## Design decisions (locked)

1. **Content-addressed blob store, not binary-in-message.** A blob is stored
   under `<STORAGE_ROOT>/attachments/<sha256[:2]>/<sha256>` (sha of the bytes →
   automatic dedup, traversal-safe path derived only from the hash). The chat
   message carries only a ref list:
   `attachments: [{ id, sha256, name, mime, size }]` (`id` == sha256). Messages
   stay small; the durable JSONL and every client parse unchanged except for the
   optional `attachments` field.
   - **Atomic, safe write** (gap 4): write to `<root>/attachments/tmp/<uuid>`,
     `fsync`, then atomic `rename` into the final hash path. Before writing,
     `symlink_metadata` the target — if a blob path already exists and is NOT a
     regular file (symlink/special), refuse (500, never follow). The write path
     is never derived from any client-supplied name.
   - **Storage root & ops** (gap 8): `STORAGE_ROOT` defaults to the directory of
     `DB_PATH` (so blobs sit beside the SQLite file and are captured by the same
     Docker data volume + backup/restore). Memory-only mode (no DB_PATH) uses a
     temp dir cleared on restart, consistent with messages. Documented in the
     deploy notes: the attachments dir is part of the mounted volume.

2. **Two room-scoped REST endpoints** (mirroring the existing membership/session
   auth the room already enforces — only a member/valid session may touch a
   room's blobs):
   - `POST /rooms/:room/attachments` — body is the raw bytes; `x-filename` and
     `content-type` headers carry name+mime. Server:
     - **Validates content, not the header** (gap 1): sniff the leading bytes —
       PNG/JPEG/WebP/PDF magic-byte signatures; TXT/MD must be valid UTF-8 with no
       NUL. The claimed `content-type` must match the sniffed type, else `415`.
       The stored `mime` is the *sniffed* one, never the client's claim.
     - **Sanitizes `x-filename`** (gap 5): strip control chars and any
       `\r\n`/header-injection; cap at 255 UTF-8 bytes; it is display metadata
       only, never a path component.
     - Enforces **per-file size** and **quotas** (gap 3, see below), computes
       sha256, writes the blob atomically (decision 1), returns
       `{ id, sha256, name, mime, size }`. Idempotent: identical bytes → same id.
     - **Lifecycle** (gap 2): a fresh upload is `pending` with a TTL (config,
       default 1h). It becomes `referenced` when a message that cites it is
       accepted — the claim is AUTHORITATIVE, done inside the message-insert
       transaction (a concurrent sweep can never leave a sent message with a lost
       attachment; if the blob was swept the whole message is rejected). A
       background sweep deletes `pending` blobs past TTL (V1's only active
       reclamation), and — future-ready but untriggered in V1 (see Retention) —
       any blob whose refcount reaches 0. So an upload that never sends leaves no
       orphan.
   - `GET /rooms/:room/attachments/:id` (gap 6) — behind the SAME `RoomAccess` /
     `require_membership` gate as every room read; `404` if the id is not
     `referenced` by a message in *this* room (no cross-room read even with the
     hash). Serves with `X-Content-Type-Options: nosniff`, the stored mime, and a
     safe `Content-Disposition: attachment; filename="<sanitized>"` (PDF/TXT/MD
     always as attachment, never inline-executed; the client opens them
     deliberately).
   Sending a message: the send path gains an optional `attachments` array of
   already-uploaded ids; the server verifies each id exists, flips it
   `pending→referenced`, and records the message-ref + room-blob-reference **in
   one atomic step** (gap 7) so a concurrent delete/dedup can never race a
   half-referenced blob.
   - **Two refcount levels** (loca-dev invariant): each room keeps its own
     *logical* reference to a blob (used for quota + display), but the *physical*
     file is deleted ONLY when the **global** reference count across all rooms
     reaches 0. Deleting one room drops that room's logical refs and decrements
     the global count; a blob still referenced by another room is untouched — one
     room's deletion can never corrupt another room's shared file.

3. **Skill / agent API — one path for BOTH runtimes.**
   `connect.sh send <server> <room> <name> <target> <text> --attach <file> [--attach <file>]`
   uploads each file (POST above, name+mime inferred), then sends the message with
   the returned refs. No runtime-specific code: the same `connect.sh` invocation
   works from a Claude Code Monitor and a Codex adapter. `listen.py` already
   delivers the whole message object, so the `attachments` field flows to every
   consumer with no wake-path change.

4. **Web = Desktop, one UI.** A composer attach button
   (drag/drop + file picker) uploads then sends. Render: images inline (lazy,
   `max-width` bounded, click to open full), PDF/TXT/MD as a labeled chip that
   opens the GET url. The Desktop Host loads the same web UI → no desktop fork.

## Limits & security

- Types (allowlist): `image/png image/jpeg image/webp application/pdf
  text/plain text/markdown` — enforced by **magic-byte / UTF-8 sniff** (gap 1),
  not the header. Anything else → 415.
- Size & quotas (gap 3): 10 MB per file (`attachments.max_bytes`), 4 attachments
  per message; **per-room quota** (`attachments.room_max_bytes`) counts each
  room's *logical* referenced size (a blob shared by two rooms counts in both);
  **building quota** (`attachments.building_max_bytes`) counts *unique physical*
  blob size (a deduped blob counts once) — over quota → 413. A **concurrent-upload
  cap** per identity so a client can't fan out uploads. The request body is
  size-capped as it streams (reject early, never buffer >limit). Test:
  re-uploading the same file must not double-count against the building quota
  (dedup), and per-room accounting stays correct across shared blobs.
- Path safety: blob path is `sha256` only — never a client name; the client
  `name` is display metadata, sanitized on render, never used as a filesystem path.
- No-follow disk ops: reads/deletes never traverse a planted symlink/reparse
  point out of the store tree. **Unix** is fully handle-relative (read
  `O_NOFOLLOW`; delete `openat(O_NOFOLLOW|O_DIRECTORY)` + `unlinkat`). **Windows**
  uses `FILE_FLAG_OPEN_REPARSE_POINT` (read + a reparse-attribute check) and
  `ReplaceFileW` (atomic replace); the delete refuses a reparse-point shard and
  no-follows the leaf, leaving one narrow shard-swap window that a fully
  dirfd-relative delete would need `ntdll NtCreateFile(RootDirectory)` to close.
  **V1 assumption (locked):** the storage directory
  (`STORAGE_ROOT` = dir of `DB_PATH`) is writable ONLY by the server/desktop
  security principal — on the Windows Desktop that is the per-user
  `app_data_dir()` (`%APPDATA%\<app>`, ACL'd to that user). The residual delete
  window therefore requires the SAME principal's concurrent write access (who
  can already tamper with the store), so it is accepted for V1; the ntdll
  hardening is a deferred follow-up. If a deployment ever puts the store on a
  path other principals can write, this assumption is void and the ntdll delete
  is required.
- No credential scan needed (opaque bytes), but type+size are enforced server-side,
  and a room member is the only reader (reuse `require_membership`).
- Retention (V1, loca-dev decision): a **referenced** attachment lives exactly as
  long as its message. Messages are never deleted, and `delete/seal room` is an
  archive/tombstone that does NOT remove the message rows or their attachment
  references — so a referenced blob is never reclaimed in V1. The only active
  reclamation is the pending-TTL sweep: an upload that is never sent is collected
  after its TTL. The refcount machinery below (derived COUNT, `ON DELETE CASCADE`
  from `messages`) is correct and future-ready — a later message-delete/room-purge
  would drop refs and let the sweep collect a blob whose GLOBAL count reaches 0 —
  but that path has no trigger in V1 and is NOT presented as a working feature.

## Acceptance matrix (loca-dev verifies independently)

- Claude Code agent AND Codex agent: `send --attach` an image and a PDF; the
  other side receives the refs and can GET the bytes (sha matches).
- Web AND Desktop: image renders inline; PDF/TXT/MD chip opens.
- A non-member cannot GET a room's blob (403/404); a blob referenced only in
  room A is 404 from room B (no cross-room read).
- Rejections: oversize → 413; over room/building quota → 413; a file whose bytes
  don't match the claimed type (e.g. a script sent as image/png) → 415; a message
  referencing a non-existent id → rejected; a control-char/CRLF `x-filename` is
  sanitized.
- Lifecycle: an upload with no following message is gone after the TTL sweep (no
  orphan); a blob whose last referencing message is deleted is collected.
- Robustness: atomic write survives a mid-write crash (no partial blob is ever
  served); a pre-existing symlink at a blob path is refused, not followed.
- Deterministic: same bytes → same id (dedup); the message-ref + refcount update
  is atomic under concurrent send/delete.

## Implementation slices (loca-care), each pushed for review before the next

1. ✅ Server: blob store module + the two endpoints + message `attachments`
   field + validation + unit/ws/e2e tests. Refs land in the SAME transaction as
   the message insert (atomic); refcount is derived, not stored (no drift).
   `cd4f045` (blob store) → `a8314cd` (endpoints/lifecycle) → `8d69ebe`
   (transaction atomicity + `tests/attachments_e2e.rs`).
2. ✅ Skill: `connect.sh send --attach` (repeatable, ≤4) + `tests/
   test_send_attach.py`, one invocation shape for both runtimes. `f431200`.
3. ✅ Web/Desktop composer + render — same web UI, no desktop fork (CSP null,
   Desktop bundles `web/`). 📎 button + drag-drop upload, staged chips, inline
   image / openable chip render via header-authed blob fetch → object URL.
   Caption-less image allowed (empty text + attachments). `d1b2552`.
4. End-to-end acceptance — loca-dev independently verifies the matrix (Web +
   Desktop image-inline / pdf-chip, cross-room 404, rejections, lifecycle) and
   the Windows blob foundation on `windows-latest` CI, then release.
