"""Bounded per-page context packets. No transport ACK is model injection.

Transport supplies authorized snapshots; adapters must acknowledge a packet
only after accepting it into the matching model context. This module never
starts a model turn, fetches credentials or grants room access.
"""
from copy import deepcopy
from difflib import SequenceMatcher
import hashlib
import json


def _digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, ensure_ascii=False).encode()).hexdigest()


class ContextBudgetExceeded(ValueError):
    pass


class WikiPageContext:
    def __init__(self, room, context_id, max_pages=64):
        if type(max_pages) is not int or not 1 <= max_pages <= 1024:
            raise ValueError("Invalid Wiki context cache budget")
        self.room = room
        self.context_id = context_id
        self.max_pages = max_pages
        self.injected_pages = {}
        self.pending = {}
        self.latest = {}
        self.generation = 0

    def reset(self, context_id):
        # Compaction can preserve the thread ID while discarding its baseline.
        # A new generation also fences acknowledgements from the old context.
        self.context_id = context_id
        self.generation += 1
        self.injected_pages.clear()
        self.pending.clear()
        self.latest.clear()

    def prepare(self, snapshot, slugs, max_chars=8000):
        if snapshot.get("room") != self.room:
            raise ValueError("Wiki belongs to another room")
        if not 1 <= max_chars <= 128000:
            raise ValueError("Invalid context budget")
        indexed = {page["slug"]: page for page in snapshot["pages"]}
        requested = list(dict.fromkeys(slugs))
        if len(requested) > self.max_pages:
            raise ContextBudgetExceeded("Too many selected pages; nothing was injected")
        if any(slug not in indexed for slug in requested):
            raise KeyError("Requested Wiki page is absent")
        offered, changes = {}, []
        for slug in requested:
            page = deepcopy(indexed[slug])
            if not isinstance(page.get("revision"), int) or page["revision"] < 0:
                raise ValueError("Invalid page revision")
            previous = self.injected_pages.get(slug)
            if previous:
                if page["revision"] < previous["revision"]:
                    raise ValueError("Stale Wiki page")
                if page["revision"] == previous["revision"]:
                    if _digest(page) != _digest(previous):
                        raise ValueError("Content changed without a new revision")
                    continue
            change = {"slug": slug, "title": page["title"], "revision": page["revision"],
                      "sources": deepcopy(page["sources"]), "content_hash": _digest(page)}
            if previous is None:
                change.update(kind="bootstrap", body=page["body"])
            else:
                old, new = previous["body"].splitlines(keepends=True), page["body"].splitlines(keepends=True)
                sections = []
                for tag, start, end, next_start, next_end in SequenceMatcher(None, old, new, autojunk=False).get_opcodes():
                    if tag != "equal":
                        sections.append({"operation": tag, "start_line": start, "end_line": end,
                                         "removed": "".join(old[start:end]),
                                         "inserted": "".join(new[next_start:next_end])})
                change.update(kind="delta", base_revision=previous["revision"],
                              base_hash=_digest(previous), sections=sections)
            changes.append(change)
            offered[slug] = page
        packet = {"room": self.room, "context_id": self.context_id,
                  "context_generation": self.generation,
                  "snapshot_revision": snapshot["revision"], "pages": changes}
        if not changes:
            return None
        packet_id = _digest(packet)
        packet["packet_id"] = packet_id
        encoded = json.dumps(packet, ensure_ascii=False, sort_keys=True)
        if len(encoded) > max_chars:
            raise ContextBudgetExceeded("Context requires a smaller page selection; nothing was injected")
        if len(set(self.latest).union(offered)) > self.max_pages:
            raise ContextBudgetExceeded("Pending context cache full; acknowledge or reset before another offer")
        self.pending[packet_id] = offered
        for slug in offered:
            self.latest[slug] = packet_id
        # A superseded offer must not accumulate forever on a long-lived thread.
        for old_id in list(self.pending):
            if old_id not in self.latest.values():
                del self.pending[old_id]
        return packet

    def acknowledge_injection(self, packet_id, room, context_id):
        if room != self.room or context_id != self.context_id:
            return False
        pages = self.pending.get(packet_id)
        if pages is None or any(self.latest.get(slug) != packet_id for slug in pages):
            return False
        for slug,page in deepcopy(pages).items():
            self.injected_pages.pop(slug,None)
            self.injected_pages[slug] = page
        while len(self.injected_pages) > self.max_pages:
            del self.injected_pages[next(iter(self.injected_pages))]
        del self.pending[packet_id]
        for slug in pages:
            del self.latest[slug]
        return True
