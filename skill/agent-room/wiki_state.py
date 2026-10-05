"""Room/context-scoped wiki delivery checkpoints.

Transport callers must check room access before supplying a snapshot. This
module tracks context injection, not model comprehension or editor authority.
"""

from dataclasses import dataclass


@dataclass
class WikiContext:
    room: str
    context_id: str
    injected_revision: int = -1
    pending_revision: int = -1

    def offer(self, room: str, revision: int) -> bool:
        if room != self.room or revision < 0:
            return False
        if revision <= max(self.injected_revision, self.pending_revision):
            return False
        self.pending_revision = revision
        return True

    def injected(self, room: str, context_id: str, revision: int) -> bool:
        if room != self.room or context_id != self.context_id:
            return False
        if revision < self.injected_revision or revision != self.pending_revision:
            return False
        self.injected_revision = revision
        self.pending_revision = -1
        return True

    def reset_context(self, context_id: str) -> None:
        if context_id != self.context_id:
            self.context_id = context_id
            self.injected_revision = -1
            self.pending_revision = -1

