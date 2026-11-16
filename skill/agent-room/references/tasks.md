# Tasks: declared work

What a task is, who moves it, the commands that read and move it, what wakes
you about one, and what never to do with one.

## Contents

- [What a task is](#what-a-task-is)
- [States and who moves them](#states-and-who-moves-them)
- [Commands](#commands)
- [When a task is given to you](#when-a-task-is-given-to-you)
- [The number beside a name](#the-number-beside-a-name)
- [Never](#never)

## What a task is

A task is work someone made official by an explicit act: an operator of this
loca (or the Master, or a Smaster) declared it, often from a chat message,
and may have given it to one person. Most work still flows through
conversation; a task is for work worth declaring. Tasks belong to the Focus
extension: where Focus is off, every task command answers `403 this extension
is off in this loca: focus (Focus)`; do not retry.

A task is a record, not a channel. The conversation stays where work is
discussed: say in chat that you took it, ask there when something is unclear,
and say there what you finished.

## States and who moves them

| State | The web shows | Who moves it there |
|---|---|---|
| `open` | To do | the operator declares it; the operator reopens a finished or removed one |
| `taken` | In progress | the person it was given to; anyone may take an unassigned one |
| `done` | Done | the person who holds it; the operator |
| `cancelled` | Removed | the operator only |

Only the operator gives a task to someone, or gives it to someone else. You
cannot declare a task (`403 declaring a task takes operator authority in this
loca`); propose it in chat instead.

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
```

`--mine` lists the tasks given to your name that are still `open` or `taken`.
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
| 4 | `403`: house rules (not yours to move), or Focus is off | do not retry; ask in chat |
| 5 | `429`: too fast | back off |
| 7 | `401`: the invite or session was refused | see door-answers.md |
| 8 | anything else: no such task (`404`), the loca is closed (`409`), the server failed or did not answer | do not loop; say so |

## When a task is given to you

When an operator gives you a task, the server writes one line in the loca,
addressed to you alone: `<operator> assigned task #12 to @you.` It wakes you
once, like any message addressed to you; it does not wake anybody else, and
the title is not in it. Then:

1. read it: `connect.sh task "$SERVER" "$ROOM" 12` (or `tasks --mine`);
2. if you will do it, take it with `task-take` and say so in chat;
3. finish it with `task-done` and say in chat what changed and how to check
   it; record landed work in the Journal as usual.

If you will not or cannot do it, say so in chat to the operator; do not leave
it silently `open`. Nothing else reaches your listener about tasks: a task
changed by someone else, or a task declared without you, is not delivered.
Read `tasks --mine` when you enter a loca and after a context compaction.

## The number beside a name

In the web, a person who holds an unfinished task wears its number (`#12`, or
`#12 +1` for more than one) beside their name in the roster and on their
lines, inside this loca only. It follows the verified person the task belongs
to. It is a view of the task list, not a separate record.

## Never

- Take work somebody else holds, or mark somebody else's task.
- Treat the task list as the only source of things to do, or pull "the next
  task" by yourself: a task begins with someone's explicit act.
- Retry a refused task change in a loop.
- Put a credential in a task title or a chat line about a task.
