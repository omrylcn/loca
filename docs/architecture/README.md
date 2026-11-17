# Architecture docs

These Markdown files are the source of truth. The HTML pages shared with the
team are generated from them and never edited by hand.

| Doc | What it covers |
|---|---|
| [`notifications.md`](notifications.md) | Building notifications: who sends to whom, audiences, the sending limit, storage, delivery, read state, agents, API, failure modes and known gaps. |
| [`extensions.md`](extensions.md) | Loca extensions: the rules, the catalog, the fence, the Vault and the Loca Board. |

Like every doc in `docs/`, these describe shipped behavior. A doc's
front-matter `status: Shipped` says so for the whole page. Where a reader
could assume something exists that does not (for example because
`PRINCIPLES.md` names it), a line starting with **Not built** says so, one
line each and without design detail. Planned work is tracked in GitHub
issues, not here. The generated page shows **Shipped** and **Not built** as
badges.

## Regenerate a page

```sh
python3 scripts/docs-to-page.py docs/architecture/notifications.md notifications.html
python3 scripts/docs-to-page.py docs/architecture/extensions.md extensions.html
```

The script uses only the Python standard library and writes one
self-contained HTML file: no scripts, no external assets. The page header
shows the doc's H1 (2–4 words, used as the page title), its first paragraph
as a one-line summary, the front-matter fields, and the source path with the
git short SHA it was generated at. Commit the doc first, so the SHA on the
page points at the text it shows. The generated page is not committed.

Keep `text` diagrams at most 50 characters wide; wider ones scroll sideways
on a phone.
