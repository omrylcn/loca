# Production panic-surface audit

This audit covers Rust code under `crates/server/src`, excluding test-only
files. It distinguishes request-triggerable failure from process bootstrap and
locally proven invariants; replacing `unwrap()` with `expect()` is not a fix.

## Enforced result

- Production `unwrap()` sites: **2** (baseline: 119; budget: 40).
- Raw `Mutex::lock().unwrap()` sites: **0**.
- Externally triggerable `unwrap()` sites: **0**.
- Every remaining `unwrap()` has a nearby `Invariant:` comment and is checked
  by `scripts/check-panic-surface.py` during `make rust-check`.

The two remaining sites update an Attention immediately after the same key was
cloned from the same room map. The mutable room guard remains held and no code
between the lookup and update can remove the key. Returning an HTTP-derived
value cannot invalidate that invariant.

## Poisoned mutex policy

The previous 117 lock unwraps let one earlier worker panic poison a standard
mutex and make every later request touching that state panic again. All
production state locks now use `RecoverMutex::lock_or_recover`:

1. a poison event is logged with the exact call site;
2. the memory-safe guarded value is recovered;
3. the poison flag is cleared so the incident does not cascade.

This is explicit degraded behavior, not hidden failure. SQLite transactions
roll back during unwinding and remain the persistence authority. A focused
unit test poisons a real mutex, proves recovery, and proves the flag is cleared.

## Other deliberate process exits

The remaining `expect()` calls are not request-data parsing:

- store open, room-rename validation/migration, socket bind, and server exit
  are bootstrap/operator configuration failures where serving a partial system
  would be false success;
- writing formatted bytes into a `String` is infallible by type contract;
- the work-policy goal id and pending-message pop are derived from a collection
  entry/length checked in the same guarded scope;
- reserved-loca cleanup is a startup persistence repair and fails closed if
  its audit mutation cannot be saved.

No handler unwraps untrusted JSON, headers, room names, query values, database
rows, or WebSocket payloads.

Run the gate directly:

```bash
python3 scripts/check-panic-surface.py
```
