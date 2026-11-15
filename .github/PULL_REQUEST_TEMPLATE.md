## Outcome

<!-- In plain words: what was wrong, and what changes for a user, operator,
or agent. -->

## What it rests on

<!-- The PRINCIPLES, DESIGN or ADR paragraph this change relies on (file and
line or section). If it touches a decision in docs/adr/0003-recorded-decisions.md,
name the entry and say why its cost has changed. -->

## Spirit test

<!-- Answer each question from PRINCIPLES "The spirit test for every PR and
commit". "If an answer falls on the wrong side, the change is discussed and
rejected." -->

1. Does it make conversation more natural? *(it should)*
2. Does it reduce the distance between human and agent? *(it should)*
3. Does it make people cautious about speaking? *(it should not)*
4. Does it turn the room into a dashboard, queue, or Jira? *(it should not)*
5. Is the system inferring intent? *(no—the participant decides explicitly)*
6. Does it solve pain experienced today, or did a document merely ask for it?
   *(pain should lead)*

## Trust boundary and risk

<!-- Auth, private-room isolation, credentials, prompt ingress, storage, deploy. -->

## Tests

<!-- For each new or changed test: does it fail on the previous code (say how
you saw it fail), or is it a guard that passes on both? -->

## Not verified

<!-- What you did not run or could not check. -->

## Verification

- [ ] `cargo fmt --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] Python skill tests
- [ ] `scripts/smoke.sh`
- [ ] New regression test added where applicable

## Compatibility and operations

<!-- What a user or operator notices on upgrade: a migration, a one-way schema
change, a new setting, being signed out, a changed default. -->

- [ ] No protocol/config/schema change
- [ ] Protocol/config/schema change is documented
- [ ] Migration and rollback are documented
- [ ] Agent runtime install/update behavior is covered

## Documentation

<!-- List updated docs, or explain why none are required. -->
