#!/usr/bin/env python3
"""Read-only pending reply diagnosis; never print room text or credentials."""

from __future__ import annotations

import argparse
from collections import Counter
import json
from pathlib import Path
import sqlite3
import time


def audit(path: Path, identities: list[str], limit: int = 50) -> dict:
    reports = {}
    # Do not instantiate AttentionStore: its initialization writes/migrates.
    with sqlite3.connect(path.resolve().as_uri() + "?mode=ro", uri=True) as connection:
        connection.row_factory = sqlite3.Row
        connection.execute("PRAGMA query_only=ON")
        connection.execute("PRAGMA busy_timeout=10000")
        connection.execute("BEGIN")  # One consistent WAL snapshot, no writes.
        for identity in identities:
            rows = connection.execute(
                "SELECT attention_id, delivery_id, room, turn_id, stored_at_ms, "
                "accepted_at_ms, turn_completed_at_ms FROM attentions "
                "WHERE identity=? AND reply_required=1 AND final_response_at_ms IS NULL "
                "AND terminal_status IS NULL ORDER BY stored_at_ms, rowid", (identity,)
            )
            counts = Counter()
            details = []
            for row in rows:
                outputs = connection.execute(
                    "SELECT relay_kind, status, covered_attention_ids_json FROM relay_items "
                    "WHERE turn_id=?", (row["turn_id"],)
                ).fetchall()
                own = [(r["relay_kind"], r["status"]) for r in outputs
                       if row["attention_id"] in json.loads(r["covered_attention_ids_json"])]
                if ("final", "accepted") in own:
                    reason = "accepted_relay_milestone_mismatch"
                elif ("final", "pending") in own:
                    reason = "relay_retry_pending"
                elif ("final", "shadow") in own:
                    reason = "shadow_not_sent"
                elif ("final_suppressed", "suppressed") in own:
                    reason = "final_suppressed"
                elif ("no_reply", "suppressed") in own:
                    reason = "required_no_reply"
                elif row["accepted_at_ms"] is None:
                    reason = "not_accepted"
                elif row["turn_completed_at_ms"] is None:
                    reason = "in_flight"
                else:
                    reason = "completed_without_final"
                counts[reason] += 1
                if len(details) < limit:
                    details.append({
                        "delivery_id": row["delivery_id"], "room": row["room"],
                        "turn_id": row["turn_id"], "reason": reason,
                        "age_seconds": max(0, int(time.time() - row["stored_at_ms"] / 1000)),
                        "accepted": row["accepted_at_ms"] is not None,
                        "turn_completed": row["turn_completed_at_ms"] is not None,
                    })
            reports[identity] = {"pending": sum(counts.values()),
                                 "reasons": dict(counts), "records": details,
                                 "records_truncated": sum(counts.values()) > limit}
    return {"read_only": True, "healthy": all(r["pending"] == 0 for r in reports.values()),
            "identities": reports}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ledger", type=Path, required=True)
    parser.add_argument("--identity", action="append", required=True)
    parser.add_argument("--limit", type=int, default=50)
    args = parser.parse_args()
    if not 0 <= args.limit <= 1000:
        parser.error("--limit must be between 0 and 1000")
    print(json.dumps(audit(args.ledger, args.identity, args.limit), indent=2))


if __name__ == "__main__":
    main()
