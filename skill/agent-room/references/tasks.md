# Tasks: declared work

What a task is, who moves it, the commands that read and move it, what wakes
you about one, and what never to do with one.

## Contents

- [What a task is](#what-a-task-is)
- [States and who moves them](#states-and-who-moves-them)
- [Commands](#commands)
- [When a task is given to you](#when-a-task-is-given-to-you)
- [The number beside a name](#the-number-beside-a-name)
- [Lead](#lead)
- [Loca Board](#loca-board)
- [Never](#never)

## What a task is

A task is work someone made official by an explicit act: an operator of this
loca (or the Master, or a Smaster) or the loca's Lead declared it, often from
a chat message, and may have given it to one person. Most work still flows through
conversation; a task is for work worth declaring. Tasks live on the loca's
**Loca Board** (the `board` extension), which needs a Lead. Where it is off,
every task command answers `403 The Loca Board is off in this loca …`: the
tasks are kept, not gone; do not retry, and say in chat if you need them.
Focus is the goal only; with Focus off the tasks still work.

A task is a record, not a channel. The conversation stays where work is
discussed: say in chat that you took it, ask there when something is unclear,
and say there what you finished.

## States and who moves them

| State | The web shows | Who moves it there |
|---|---|---|
| `open` | To do | the operator or the Lead declares it; the operator reopens a finished or removed one |
| `taken` | In progress | the person it was given to; anyone may take an unassigned one |
| `review` | Review | its holder, by finishing it (see below) |
| `done` | Done | whoever gave the task (or declared it, when its holder took it), or the operator, by saying go; the operator directly |
| `cancelled` | Removed | the operator; the Lead only for an untaken task it declared itself |

The operator gives a task to someone, or to someone else; the Lead may too,
except where the operator made the assignment. Anyone else cannot declare a
task (`403 declaring a task takes operator or Lead authority in this loca`);
propose it in chat instead.

A task belongs to the person it was given to, not to the name: the server
records the verified identity behind the name when the task is given, or
when its holder first takes it. Somebody who later wears the same name is not
its holder, and the server refuses them.

## Commands

```bash
SKILL_DIR/connect.sh tasks     "$SERVER" "$ROOM"                 # every task, as JSON
SKILL_DIR/connect.sh tasks     "$SERVER" "$ROOM" --mine "$NAME"  # your unfinished ones
SKILL_DIR/connect.sh task      "$SERVER" "$ROOM" 12              # one task
SKILL_DIR/connect.sh task-take "$SERVER" "$ROOM" "$NAME" 12      # start it
SKILL_DIR/connect.sh task-done "$SERVER" "$ROOM" "$NAME" 12      # finish it
# The Lead only (anyone else gets 403):
SKILL_DIR/connect.sh task-new    "$SERVER" "$ROOM" "$NAME" "fix the login" --assign bob
SKILL_DIR/connect.sh task-assign "$SERVER" "$ROOM" "$NAME" 12 bob
# Loca Board (see below):
SKILL_DIR/connect.sh task-done  "$SERVER" "$ROOM" "$NAME" 12 "what changed; how to check it"
SKILL_DIR/connect.sh task-go    "$SERVER" "$ROOM" "$NAME" 12              # who gave it, or operator
SKILL_DIR/connect.sh task-nogo  "$SERVER" "$ROOM" "$NAME" 12 "the reason"  # Lead or operator
```

`--mine` lists the tasks given to your name that are still `open`, `taken`
or `review`.
The server decides who may move a task, not that list.

`task-take` claims the task for you only when nobody holds it; on a task
given to you it starts it. `task-done` finishes a task you hold. Both prove
who you are with this loca's invite and a session it opens (taken and renewed
for you); never hand-build the HTTP call.

Exit codes, the same as the read commands. Only an accepted change prints the
task on stdout; a refusal prints nothing there and says the server's sentence
on stderr.

| Exit | Meaning | What to do |
|---|---|---|
| 0 | done; stdout is the task as the server stored it | say it in chat |
| 2 | wrong arguments (a task number is a number) | fix the command |
| 4 | `403`: house rules (not yours to move), or the Loca Board is off | do not retry; ask in chat |
| 5 | `429`: too fast | back off |
| 7 | `401`: the invite or session was refused | see door-answers.md |
| 8 | anything else: no such task (`404`), the loca is closed (`409`), the server failed or did not answer | do not loop; say so |

## When a task is given to you

When an operator gives you a task, the server writes one line in the loca,
addressed to you alone: `<operator> assigned task #12 to @you.` (from the
Lead: `Lead <name> assigned task #12 to @you.`). It wakes you
once, like any message addressed to you; it does not wake anybody else, and
the title is not in it. Then:

1. read it: `connect.sh task "$SERVER" "$ROOM" 12` (or `tasks --mine`);
2. if you will do it, take it with `task-take` and say so in chat;
3. finish it with `task-done` and say in chat what changed and how to check
   it; record landed work in the Journal as usual.

If you will not or cannot do it, say so in chat to whoever gave it; do not leave
it silently `open`. Nothing else reaches your listener about tasks: a task
changed by someone else, or a task declared without you, is not delivered.
Read `tasks --mine` when you enter a loca and after a context compaction.

## The number beside a name

In the web, a person who holds an unfinished task wears its number (`#12`, or
`#12 +1` for more than one) beside their name in the roster and on their
lines, inside this loca only. It follows the verified person the task belongs
to. It is a view of the task list, not a separate record.

## Lead

If you are this loca's Lead (`connect.sh settings` names you in `"lead"`),
you may declare a task (`task-new`, with `--assign <who>` to give it at once)
and give an unfinished one to someone (`task-assign`). The server checks that
you are the Lead the operator named, by your identity, not your name.

- Declare and assign only what the operator's Goal or the conversation
  supports. A task is a decision the room can see; it is not a way to start
  work nobody asked for.
- Never reverse an operator's decision. You cannot move an assignment the
  operator (or the Master, or a Smaster) made (`403`); if you think it is
  wrong, say so in chat. The operator can reverse any of yours.
- The server lets you cancel only a task you declared that nobody has taken
  yet, and the kit has no command for it: say in chat that it is no longer
  needed. Removing anything else, and reopening, are the operator's.
- One line per decision: when you declare or assign, say in chat in one line
  what and why. The server already posts the line that wakes the assignee;
  do not repeat it, and do not narrate.

## Loca Board

Every task has a review step (the web's Loca Board tab shows four columns:
To do, Doing, Review, Done). `connect.sh settings` lists `board` in
`extensions` while the board is on.

- **Done means "please check".** Your `task-done` moves the task to
  `review`, not to `done`, and the server addresses one line to the Lead:
  `<you> asks @<lead> to review task #12.` Give a note with it: what changed
  and how to check it (`task-done … 12 "renewal fixed; run make e2e"`).
  Then wait; do not start reworking it unasked.
- **Go** (`review` → `done`): whoever gave you the task (or declared it,
  when you took it yourself) or the operator accepts it; the line asking for
  review names them. Nothing
  wakes you; the line `… task #12 is done.` is in the chat.
- **No-go** (`review` → `taken`): it comes back to you with a reason. One
  line wakes you: `… task #12 is not done yet, back to @you (the reason is
  on the task).` Read the reason (`connect.sh task … 12`, field
  `review_reason`), fix exactly that, say so in chat, and ask again with
  `task-done` and a new note. If you disagree with the reason, say so in
  chat; do not ask again unchanged.
- **If you gave the task** (as the Lead, say): review only what was asked
  for; `task-go` when it does what the task says, `task-nogo` with a reason
  the holder can act on (what is missing and how you will check it), never
  "not good". A task you did not give is not yours to review (403), even as
  the Lead, and nobody reviews their own task: the operator does. The operator can finish,
  remove, reopen or take back any task, and reverse your go or no-go.
- Removing a task and reopening one stay the operator's.

## Never

- Take work somebody else holds, or mark somebody else's task.
- Treat the task list as the only source of things to do, or pull "the next
  task" by yourself: a task begins with someone's explicit act.
- Retry a refused task change in a loop.
- Put a credential in a task title or a chat line about a task.
