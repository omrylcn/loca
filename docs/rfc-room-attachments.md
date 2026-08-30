# RFC: Room attachments (images, PDF, text)

Status: proposed (loca-care owns implementation; loca-dev owns Web/Desktop render
+ final acceptance). Target: a follow-on minor after 0.8.6.

## Goal

Humans and agents in the same room can share files: images shown inline, PDF /
text / Markdown offered to open/download. Agents send via one `connect.sh`
command that is identical for Claude Code and Codex. No binary is ever embedded
in a chat-message JSON or a WebSocket frame.

## Design decisions (locked)

1. **Content-addressed blob store, not binary-in-message.** A blob is stored
   under `<data>/attachments/<sha256[:2]>/<sha256>` (sha of the bytes → automatic
   dedup, traversal-safe path derived only from the hash). The chat message
   carries only a ref list:
   `attachments: [{ id, sha256, name, mime, size }]` (`id` == sha256). Messages
   stay small; the durable JSONL and every client parse unchanged except for the
   optional `attachments` field.

2. **Two room-scoped REST endpoints** (mirroring the existing membership/session
   auth the room already enforces — only a member/valid session may touch a
   room's blobs):
   - `POST /rooms/:room/attachments` — body is the raw bytes; `x-filename` and
     `content-type` headers carry name+mime. Server validates **type** (allowlist)
     and **size** (<= limit), computes sha256, writes the blob, returns
     `{ id, sha256, name, mime, size }`. Idempotent: re-uploading identical bytes
     returns the same id.
   - `GET /rooms/:room/attachments/:id` — streams the blob with its stored mime
     and `content-disposition`. 404 if the id is not referenced by any message in
     that room (a blob is reachable only through a room it was posted to — no
     cross-room read even with the hash).
   Sending a message: the existing send path gains an optional `attachments`
   array of already-uploaded ids; the server verifies each id exists before
   accepting the message.

3. **Skill / agent API — one path for BOTH runtimes.**
   `connect.sh send <server> <room> <name> <target> <text> --attach <file> [--attach <file>]`
   uploads each file (POST above, name+mime inferred), then sends the message with
   the returned refs. No runtime-specific code: the same `connect.sh` invocation
   works from a Claude Code Monitor and a Codex adapter. `listen.py` already
   delivers the whole message object, so the `attachments` field flows to every
   consumer with no wake-path change.

4. **Web = Desktop, one UI (loca-dev's slice).** A composer attach button
   (drag/drop + file picker) uploads then sends. Render: images inline (lazy,
   `max-width` bounded, click to open full), PDF/TXT/MD as a labeled chip that
   opens the GET url. The Desktop Host loads the same web UI → no desktop fork.

## Limits & security

- Types (allowlist): `image/png image/jpeg image/webp application/pdf
  text/plain text/markdown`. Anything else → 415.
- Size: 10 MB per file (config `attachments.max_bytes`), 4 attachments per message.
- Path safety: blob path is `sha256` only — never a client name; the client
  `name` is display metadata, sanitized on render, never used as a filesystem path.
- No credential scan needed (opaque bytes), but type+size are enforced server-side,
  and a room member is the only reader (reuse `require_membership`).
- Retention: blobs live with the room; deleting a room deletes its blobs (dedup
  refcount by room, not global, to keep deletion simple).

## Acceptance matrix (loca-dev verifies independently)

- Claude Code agent AND Codex agent: `send --attach` an image and a PDF; the
  other side receives the refs and can GET the bytes (sha matches).
- Web AND Desktop: image renders inline; PDF/TXT/MD chip opens.
- A non-member cannot GET a room's blob (403/404); an oversize/wrong-type upload
  is rejected; a message referencing a non-existent id is rejected.
- Deterministic: same bytes → same id (dedup).

## Implementation slices (loca-care), each pushed for review before the next

1. Server: blob store module + the two endpoints + message `attachments` field +
   validation + unit/ws tests. **(this slice first)**
2. Skill: `connect.sh send --attach` + a test in the onboarding/skill suite,
   exercised for both runtime invocation shapes.
3. Handoff to loca-dev: Web/Desktop composer + render.
4. End-to-end acceptance (loca-dev), then release.
