#!/usr/bin/env python3
"""`connect.sh brief`: one loca's state in a few lines, for its Lead or its
operator to report from. What is done, what waits for a decision, who is
blocked on whom, who works on what, and the Goal.

It reads what connect.sh already fetched (tasks, waits, goals, journal) as
JSON files; each one may be missing because its extension is off. Every line
names its source (a task number, a wait, a journal entry), so a report built
from it can be checked. Stdlib only.
"""
import json
import sys
import time

DAY_MS = 24 * 60 * 60 * 1000


def _load(path):
    """A fetched document, or None where it could not be read (extension
    off, no seat, a network error): that section then says so."""
    if not path or path == "-":
        return None
    try:
        with open(path, encoding="utf-8") as handle:
            text = handle.read().strip()
        return json.loads(text) if text else None
    except (OSError, ValueError):
        return None


def _ago(ms, now):
    minutes = max(0, int((now - int(ms or 0)) / 60000))
    if minutes < 60:
        return f"{minutes} min ago"
    if minutes < 48 * 60:
        return f"{minutes // 60} h ago"
    return f"{minutes // (24 * 60)} d ago"


def brief(room, tasks, waits, goals, journal, now=None):
    """The lines of one loca's brief. `None` for a section means its source
    was not readable."""
    now = int(time.time() * 1000) if now is None else now
    out = [f"Brief of {room}"]

    if isinstance(goals, list):
        active = [g for g in goals if g.get("status") == "active"]
        if active:
            goal = active[0]
            line = f"Goal: {goal.get('outcome')}"
            if goal.get("checkpoint"):
                line += f" (next checkpoint: {goal['checkpoint']})"
            out.append(line)
        else:
            out.append("Goal: none active")
    else:
        out.append("Goal: Focus is not on here")

    if isinstance(tasks, list):
        here = [t for t in tasks if t.get("room", room) == room]
        review = [t for t in here if t.get("status") == "review"]
        taken = [t for t in here if t.get("status") == "taken"]
        open_ = [t for t in here if t.get("status") == "open"]
        done = [t for t in here if t.get("status") == "done"
                and now - int(t.get("closed_at") or t.get("progress_at") or 0) <= DAY_MS]
        out.append("")
        out.append(f"Waiting for a decision ({len(review)}):")
        for t in review:
            note = f" - note: {t['review_note']}" if t.get("review_note") else ""
            out.append(f"  #{t['id']} {t.get('title')} - {t.get('assigned_to') or 'nobody'} asks "
                       f"{t.get('assigned_by') or t.get('created_by')} to review{note}")
        out.append(f"In progress ({len(taken)}):")
        for t in taken:
            back = f" - sent back: {t['review_reason']}" if t.get("review_reason") else ""
            out.append(f"  #{t['id']} {t.get('title')} - {t.get('assigned_to') or 'nobody'}, "
                       f"moved {_ago(t.get('progress_at'), now)}{back}")
        out.append(f"To do ({len(open_)}):")
        for t in open_:
            out.append(f"  #{t['id']} {t.get('title')} - {t.get('assigned_to') or 'nobody yet'}")
        out.append(f"Done in the last 24 h ({len(done)}):")
        for t in done:
            out.append(f"  #{t['id']} {t.get('title')} - {t.get('assigned_to') or ''}".rstrip(" -"))
    else:
        out.append("")
        out.append("Tasks: the Loca Board is not on here")

    out.append("")
    if isinstance(waits, list):
        here = [w for w in waits if w.get("room", room) == room]
        out.append(f"Blocked ({len(here)}):")
        for w in here:
            out.append(f"  {w.get('waiter')} waits for {w.get('waiting_for')} since "
                       f"{_ago(w.get('since'), now)}: {w.get('reason')}")
    else:
        out.append("Blocked: waits could not be read")

    if isinstance(journal, list):
        recent = sorted((e for e in journal if now - int(e.get("at") or 0) <= DAY_MS),
                        key=lambda e: int(e.get("at") or 0), reverse=True)[:5]
        out.append(f"Landed in the last 24 h ({len(recent)} shown):")
        for e in recent:
            out.append(f"  {e.get('by')}: {e.get('text')} ({_ago(e.get('at'), now)})")
    else:
        out.append("Landed: the Journal is not on here")
    return out


def main(argv):
    # brief.py ROOM TASKS WAITS GOALS JOURNAL   (paths, or - where unreadable)
    if len(argv) != 6:
        sys.stderr.write("usage: brief.py ROOM TASKS WAITS GOALS JOURNAL\n")
        return 2
    room = argv[1]
    print("\n".join(brief(room, *(_load(path) for path in argv[2:]))))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
