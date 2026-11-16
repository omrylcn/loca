#!/usr/bin/env python3
"""Turn one Markdown doc into a standalone, self-contained HTML page.

    python3 scripts/docs-to-page.py docs/architecture/notifications.md OUT.html

The Markdown file is the source of truth; the page is generated from it and
never edited by hand. Only the Python standard library is used. The converter
covers what the architecture docs use: optional front matter (`title:`,
`summary:`), ATX headings, paragraphs, nested lists, pipe tables, fenced code,
blockquotes, rules, inline code, bold, italic and links. Every character of
the source is HTML-escaped; nothing in it is passed through as markup.

Page shape: the H1 (or front-matter `title`) is the page <title> and must be
2-4 words; the first paragraph (or `summary`) is the one-line lede and is not
repeated in the body; the H2s become the table of contents. A paragraph,
list item or table cell that starts with **Shipped** or **Not built** gets a
status badge, as does a front-matter `status:` with one of those values. The
docs describe shipped behavior only; planned work lives in GitHub issues.
"""

from __future__ import annotations

import html
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

BADGES = {
    "Shipped": "shipped",
    "Not built": "no",
}


# ---------------------------------------------------------------- inline


def esc(text: str) -> str:
    return html.escape(text, quote=True)


def safe_href(url: str) -> str | None:
    """Absolute web links and in-page anchors only. A relative repository
    path means nothing in a standalone page, so it is not linked."""
    url = url.strip()
    if url.startswith("#") or re.match(r"(?i)^(https?:|mailto:)", url):
        return url
    return None


def inline(text: str) -> str:
    """Render one line of inline Markdown. Code spans are cut out first so
    nothing inside them is formatted; everything else is escaped before any
    tag is added."""
    out: list[str] = []
    pos = 0
    for m in re.finditer(r"(`+)(.+?)\1", text):
        out.append(_inline_plain(text[pos : m.start()]))
        out.append(f"<code>{esc(m.group(2).strip())}</code>")
        pos = m.end()
    out.append(_inline_plain(text[pos:]))
    return "".join(out)


def _inline_plain(text: str) -> str:
    links: list[str] = []

    def link(m: re.Match) -> str:
        label = _emphasis(esc(m.group(1)))
        href = safe_href(m.group(2))
        if href is None:
            rendered = f'<span class="xref" title="{esc(m.group(2))}">{label}</span>'
        else:
            rendered = f'<a href="{esc(href)}">{label}</a>'
        links.append(rendered)
        return f"\x00{len(links) - 1}\x00"

    text = re.sub(r"\[([^\]]+)\]\(([^)\s]+)\)", link, text)
    text = _emphasis(esc(text))
    return re.sub(r"\x00(\d+)\x00", lambda m: links[int(m.group(1))], text)


def _emphasis(escaped: str) -> str:
    escaped = re.sub(r"\*\*(.+?)\*\*", r"<strong>\1</strong>", escaped)
    escaped = re.sub(r"(?<![\w*])\*(?!\s)(.+?)(?<!\s)\*(?![\w*])", r"<em>\1</em>", escaped)
    escaped = re.sub(r"(?<![\w_])_(?!\s)(.+?)(?<!\s)_(?![\w_])", r"<em>\1</em>", escaped)
    return escaped


def with_badge(rendered: str) -> str:
    for word, cls in BADGES.items():
        lead = f"<strong>{word}</strong>"
        if rendered.startswith(lead):
            rest = rendered[len(lead) :].lstrip()
            return f'<span class="badge {cls}">{word}</span>{rest}'
    return rendered


def plain(text: str) -> str:
    """Markdown inline -> plain text (for <title> and the lede)."""
    text = re.sub(r"\[([^\]]+)\]\([^)]+\)", r"\1", text)
    text = re.sub(r"[*_`]", "", text)
    return " ".join(text.split())


# ---------------------------------------------------------------- blocks


def slugify(text: str, seen: dict[str, int]) -> str:
    slug = re.sub(r"[^\w\s-]", "", plain(text).lower())
    slug = re.sub(r"[\s_]+", "-", slug).strip("-") or "section"
    n = seen.get(slug, 0)
    seen[slug] = n + 1
    return slug if n == 0 else f"{slug}-{n}"


def split_row(line: str) -> list[str]:
    line = line.strip()
    if line.startswith("|"):
        line = line[1:]
    if line.endswith("|") and not line.endswith("\\|"):
        line = line[:-1]
    cells, cell, i = [], "", 0
    in_code = False
    while i < len(line):
        ch = line[i]
        if ch == "\\" and i + 1 < len(line) and line[i + 1] == "|":
            cell += "|"
            i += 2
            continue
        if ch == "`":
            in_code = not in_code
        if ch == "|" and not in_code:
            cells.append(cell.strip())
            cell = ""
        else:
            cell += ch
        i += 1
    cells.append(cell.strip())
    return cells


TABLE_SEP = re.compile(r"^\s*\|?\s*:?-{3,}:?\s*(\|\s*:?-{3,}:?\s*)*\|?\s*$")
LIST_ITEM = re.compile(r"^(\s*)([-*+]|\d+[.)])\s+(.*)$")
FENCE = re.compile(r"^(\s*)(```+|~~~+)\s*([\w+-]*)\s*$")


class Doc:
    def __init__(self) -> None:
        self.title = ""
        self.summary = ""
        self.toc: list[tuple[str, str]] = []
        self.body: list[str] = []
        self.ids: dict[str, int] = {}
        self.meta: list[tuple[str, str]] = []


def convert(source: str) -> Doc:
    doc = Doc()
    lines = source.replace("\r\n", "\n").split("\n")
    meta: dict[str, str] = {}
    if lines and lines[0].strip() == "---":
        for end in range(1, len(lines)):
            if lines[end].strip() == "---":
                for raw in lines[1:end]:
                    if ":" in raw:
                        key, value = raw.split(":", 1)
                        key = key.strip().lower()
                        meta[key] = value.strip().strip("\"'")
                        if key not in ("title", "summary"):
                            doc.meta.append((key, meta[key]))
                lines = lines[end + 1 :]
                break
    doc.title = meta.get("title", "")
    doc.summary = meta.get("summary", "")
    want_lede = not doc.summary

    i = 0
    para: list[str] = []

    def flush_para() -> None:
        nonlocal want_lede
        if not para:
            return
        text = " ".join(s.strip() for s in para)
        para.clear()
        if want_lede and doc.title:
            doc.summary = plain(text)
            want_lede = False
            return
        want_lede = False
        doc.body.append(f"<p>{with_badge(inline(text))}</p>")

    while i < len(lines):
        line = lines[i]
        stripped = line.strip()

        fence = FENCE.match(line)
        if fence:
            flush_para()
            marker, lang = fence.group(2), fence.group(3)
            code: list[str] = []
            i += 1
            while i < len(lines) and not lines[i].strip().startswith(marker):
                code.append(lines[i])
                i += 1
            i += 1
            cls = f' class="lang-{esc(lang)}"' if lang else ""
            label = f'<span class="codelang">{esc(lang)}</span>' if lang and lang != "text" else ""
            doc.body.append(f'<div class="codebox">{label}<pre><code{cls}>{esc(chr(10).join(code))}</code></pre></div>')
            continue

        if not stripped:
            flush_para()
            i += 1
            continue

        heading = re.match(r"^(#{1,6})\s+(.*?)\s*#*\s*$", line)
        if heading:
            flush_para()
            level, text = len(heading.group(1)), heading.group(2)
            if level == 1 and not doc.title:
                doc.title = plain(text)
            elif level == 1:
                pass
            else:
                hid = slugify(text, doc.ids)
                if level == 2:
                    doc.toc.append((hid, plain(text)))
                doc.body.append(
                    f'<h{level} id="{hid}">{inline(text)}'
                    f'<a class="anchor" href="#{hid}" aria-label="Link to this section">#</a></h{level}>'
                )
            i += 1
            continue

        if re.match(r"^\s*([-*_])(\s*\1){2,}\s*$", line):
            flush_para()
            doc.body.append("<hr>")
            i += 1
            continue

        if "|" in line and i + 1 < len(lines) and TABLE_SEP.match(lines[i + 1]):
            flush_para()
            head = split_row(line)
            aligns = []
            for cell in split_row(lines[i + 1]):
                cell = cell.strip()
                aligns.append(
                    "center" if cell.startswith(":") and cell.endswith(":")
                    else "right" if cell.endswith(":") else ""
                )
            i += 2
            rows = []
            while i < len(lines) and lines[i].strip() and "|" in lines[i]:
                rows.append(split_row(lines[i]))
                i += 1

            def cell_html(tag: str, text: str, col: int) -> str:
                align = aligns[col] if col < len(aligns) else ""
                style = f' style="text-align:{align}"' if align else ""
                if tag == "td" and col < len(head) and head[col]:
                    # The column name, shown beside the value when a phone
                    # stacks each row into a card.
                    style += f' data-label="{esc(plain(head[col]))}"'
                content = with_badge(inline(text))
                if tag == "td" and content:
                    # One box per cell, so the phone layout's two-column grid
                    # holds the label and the whole value, not each word.
                    content = f'<span class="cell">{content}</span>'
                return f"<{tag}{style}>{content}</{tag}>"

            thead = "".join(cell_html("th", c, n) for n, c in enumerate(head))
            tbody = "".join(
                "<tr>" + "".join(cell_html("td", (r + [""] * len(head))[n], n) for n in range(len(head))) + "</tr>"
                for r in rows
            )
            doc.body.append(
                f'<div class="tablewrap" tabindex="0"><table><thead><tr>{thead}</tr></thead>'
                f"<tbody>{tbody}</tbody></table></div>"
            )
            continue

        if stripped.startswith(">"):
            flush_para()
            quote: list[str] = []
            while i < len(lines) and lines[i].strip().startswith(">"):
                quote.append(re.sub(r"^\s*>\s?", "", lines[i]))
                i += 1
            inner = convert_fragment("\n".join(quote), doc)
            doc.body.append(f"<blockquote>{inner}</blockquote>")
            continue

        if LIST_ITEM.match(line):
            flush_para()
            block: list[str] = []
            while i < len(lines):
                cur = lines[i]
                if not cur.strip():
                    nxt = lines[i + 1] if i + 1 < len(lines) else ""
                    if nxt.strip() and (LIST_ITEM.match(nxt) or nxt.startswith("  ")):
                        i += 1
                        continue
                    break
                if not LIST_ITEM.match(cur) and not cur.startswith((" ", "\t")) and block:
                    # A lazy continuation line of the previous item.
                    if FENCE.match(cur) or cur.lstrip().startswith(("#", "|", ">")):
                        break
                block.append(cur)
                i += 1
            doc.body.append(render_list(block))
            continue

        para.append(line)
        i += 1

    flush_para()
    return doc


def convert_fragment(text: str, parent: Doc) -> str:
    sub = convert(text)
    return "".join(sub.body)


def render_list(lines: list[str]) -> str:
    """Nested lists by indentation. Each item's continuation lines join it."""
    items: list[tuple[int, bool, str]] = []  # indent, ordered, text
    for line in lines:
        m = LIST_ITEM.match(line)
        if m:
            items.append((len(m.group(1).expandtabs(4)), m.group(2)[0].isdigit(), m.group(3)))
        elif items:
            indent, ordered, text = items[-1]
            items[-1] = (indent, ordered, text + " " + line.strip())
    out: list[str] = []
    stack: list[tuple[int, str]] = []  # indent, tag
    for indent, ordered, text in items:
        tag = "ol" if ordered else "ul"
        while stack and indent < stack[-1][0]:
            out.append(f"</li></{stack.pop()[1]}>")
        if stack and indent == stack[-1][0]:
            out.append("</li>")
            if stack[-1][1] != tag:
                # A bullet list followed by a numbered one: two lists.
                out.append(f"</{stack.pop()[1]}>")
        if not stack or indent > stack[-1][0]:
            out.append(f"<{tag}>")
            stack.append((indent, tag))
        out.append(f"<li>{with_badge(inline(text))}")
    while stack:
        out.append(f"</li></{stack.pop()[1]}>")
    return "".join(out)


# ---------------------------------------------------------------- page


def git_stamp(path: Path) -> str:
    def git(*args: str) -> str:
        try:
            return subprocess.run(
                ["git", *args], cwd=path.parent, capture_output=True, text=True, check=True
            ).stdout.strip()
        except (OSError, subprocess.CalledProcessError):
            return ""

    sha = git("rev-parse", "--short", "HEAD")
    if not sha:
        return ""
    dirty = git("status", "--porcelain", "--", path.name)
    return sha + (" + local edits" if dirty else "")


def source_label(path: Path) -> str:
    try:
        return path.resolve().relative_to(ROOT).as_posix()
    except ValueError:
        return path.name


CSS = """
:root {
  --bg: #f7f7f5; --panel: #ffffff; --panel2: #f0f0ec; --line: #e1e1db; --line2: #cfcfc7;
  --fg: #1d1f22; --muted: #565b63; --dim: #767b83;
  --accent: #1f6f66; --accent-soft: #e2f0ed;
  --shipped: #1d7a4a; --shipped-bg: #e3f3ea;
  --no: #a33a3a; --no-bg: #f8e6e6;
  --code-bg: #f1f1ed;
  --sans: system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
  --mono: ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, "Liberation Mono", monospace;
  color-scheme: light;
}
@media (prefers-color-scheme: dark) {
  :root:not([data-theme="light"]) {
    --bg: #111315; --panel: #171a1d; --panel2: #1d2125; --line: #2a2f35; --line2: #3a4048;
    --fg: #e6e8ea; --muted: #a9afb7; --dim: #868d96;
    --accent: #5ccfbf; --accent-soft: #15302c;
    --shipped: #6fd49c; --shipped-bg: #16301f;
    --no: #ff9a9a; --no-bg: #3a1c1c;
    --code-bg: #1b1f23;
    color-scheme: dark;
  }
}
:root[data-theme="dark"] {
  --bg: #111315; --panel: #171a1d; --panel2: #1d2125; --line: #2a2f35; --line2: #3a4048;
  --fg: #e6e8ea; --muted: #a9afb7; --dim: #868d96;
  --accent: #5ccfbf; --accent-soft: #15302c;
  --shipped: #6fd49c; --shipped-bg: #16301f;
  --no: #ff9a9a; --no-bg: #3a1c1c;
  --code-bg: #1b1f23;
  color-scheme: dark;
}
* { box-sizing: border-box; }
html { -webkit-text-size-adjust: 100%; }
body { margin: 0; background: var(--bg); color: var(--fg); font: 16px/1.65 var(--sans); }
.page { max-width: 1120px; margin: 0 auto; padding: 48px 16px 80px;
  padding-left: max(16px, env(safe-area-inset-left, 0px)); padding-right: max(16px, env(safe-area-inset-right, 0px)); }
.layout { display: grid; grid-template-columns: minmax(0, 760px); justify-content: center; gap: 0 48px; }
header.dochead { border-bottom: 1px solid var(--line); padding-bottom: 20px; margin-bottom: 8px; }
.eyebrow { font: 600 12px/1.4 var(--mono); letter-spacing: .08em; text-transform: uppercase; color: var(--accent); margin: 0 0 10px; }
h1 { font-size: 34px; line-height: 1.2; letter-spacing: -.015em; margin: 0 0 10px; text-wrap: balance; }
.lede { font-size: 18px; color: var(--muted); margin: 0 0 14px; max-width: 64ch; }
.source { font: 12.5px/1.5 var(--mono); color: var(--dim); margin: 0; overflow-wrap: anywhere; }
.source code { font-size: inherit; padding: 0; background: none; border: 0; color: var(--muted); }
nav.toc { margin: 20px 0 8px; padding: 14px 18px; background: var(--panel); border: 1px solid var(--line); border-radius: 10px; }
nav.toc h2 { font: 600 12px/1.4 var(--mono); letter-spacing: .08em; text-transform: uppercase; color: var(--dim); margin: 0 0 8px; border: 0; padding: 0; }
nav.toc ol { margin: 0; padding-left: 20px; columns: 2; column-gap: 28px; }
nav.toc li { margin: 2px 0; break-inside: avoid; font-size: 14.5px; }
nav.toc a { color: var(--fg); text-decoration: none; }
nav.toc a:hover { color: var(--accent); text-decoration: underline; }
main h2 { font-size: 24px; line-height: 1.3; margin: 44px 0 12px; padding-top: 12px; border-top: 1px solid var(--line); scroll-margin-top: 16px; text-wrap: balance; }
main h3 { font-size: 18px; line-height: 1.35; margin: 30px 0 8px; scroll-margin-top: 16px; }
main h4 { font-size: 16px; margin: 24px 0 6px; }
.anchor { margin-left: 8px; color: var(--dim); text-decoration: none; opacity: 0; font-weight: 400; }
h2:hover .anchor, h3:hover .anchor, .anchor:focus { opacity: 1; }
p { margin: 0 0 14px; }
ul, ol { margin: 0 0 16px; padding-left: 22px; }
li { margin: 4px 0; }
li > ul, li > ol { margin: 4px 0 0; }
a { color: var(--accent); text-underline-offset: 2px; }
a:focus-visible, .tablewrap:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; border-radius: 4px; }
.xref { border-bottom: 1px dotted var(--dim); }
strong { font-weight: 650; }
code { font: .875em/1.5 var(--mono); background: var(--code-bg); border: 1px solid var(--line); border-radius: 5px; padding: .08em .35em; overflow-wrap: anywhere; }
.codebox { position: relative; margin: 0 0 18px; }
.codelang { position: absolute; top: 6px; right: 10px; font: 600 10.5px var(--mono); text-transform: uppercase; letter-spacing: .08em; color: var(--dim); }
pre { margin: 0; font-size: 13px; line-height: 1.5; background: var(--code-bg); border: 1px solid var(--line); border-radius: 10px; padding: 14px 16px; overflow-x: auto; }
pre code { font-size: 13px; line-height: 1.5; background: none; border: 0; padding: 0; overflow-wrap: normal; white-space: pre; }
.tablewrap { overflow-x: auto; margin: 0 0 20px; border: 1px solid var(--line); border-radius: 10px; background: var(--panel); }
table { width: 100%; border-collapse: collapse; font-size: 14.5px; line-height: 1.5; }
th, td { text-align: left; vertical-align: top; padding: 10px 14px; border-bottom: 1px solid var(--line); }
th { font: 600 12px/1.4 var(--sans); letter-spacing: .03em; text-transform: uppercase; color: var(--dim); background: var(--panel2); white-space: nowrap; }
tbody tr:last-child td { border-bottom: 0; }
td code { white-space: nowrap; overflow-wrap: normal; }
td:first-child { min-width: 9em; }
blockquote { margin: 0 0 16px; padding: 4px 16px; border-left: 3px solid var(--line2); color: var(--muted); }
hr { border: 0; border-top: 1px solid var(--line); margin: 32px 0; }
.badge { display: inline-block; vertical-align: 1px; margin-right: 8px; padding: 1px 9px; border-radius: 999px;
  font: 700 11px/1.6 var(--mono); letter-spacing: .05em; text-transform: uppercase; white-space: nowrap; }
.badge.shipped { color: var(--shipped); background: var(--shipped-bg); }
.badge.no { color: var(--no); background: var(--no-bg); }
td .badge { margin: 0 6px 0 0; }
dl.meta { display: flex; flex-wrap: wrap; gap: 8px 22px; margin: 0 0 14px; padding: 10px 14px;
  background: var(--panel); border: 1px solid var(--line); border-radius: 10px; font-size: 13.5px; }
dl.meta div { min-width: 0; }
dl.meta dt { font: 600 11px/1.4 var(--mono); letter-spacing: .06em; text-transform: uppercase; color: var(--dim); }
dl.meta dd { margin: 0; color: var(--fg); overflow-wrap: anywhere; }
dl.meta .badge { margin: 2px 0 0; }
footer.docfoot { margin-top: 56px; padding-top: 16px; border-top: 1px solid var(--line); color: var(--dim); font-size: 13px; }
@media (min-width: 1180px) {
  .layout { grid-template-columns: minmax(0, 760px) 240px; }
  header.dochead { grid-column: 1; }
  main { grid-column: 1; grid-row: 2; }
  nav.toc { grid-column: 2; grid-row: 1 / span 2; align-self: start; position: sticky; top: 24px; margin-top: 4px;
    background: transparent; border: 0; border-left: 1px solid var(--line); border-radius: 0; padding: 4px 0 4px 18px; }
  nav.toc ol { columns: 1; padding-left: 18px; }
  footer.docfoot { grid-column: 1; }
}
@media (max-width: 560px) {
  .page { padding-top: 28px; }
  h1 { font-size: 28px; }
  .lede { font-size: 17px; }
  nav.toc ol { columns: 1; }
  main h2 { font-size: 21px; margin-top: 36px; }
  /* Diagrams are drawn at most 50 columns wide so they fit at 390 px. */
  pre { font-size: 11px; padding: 10px; }
  pre code { font-size: 11px; }
  /* Each table row becomes a card; the column name sits beside its value. */
  .tablewrap { overflow: visible; border: 0; border-radius: 0; background: none; }
  .tablewrap table, .tablewrap tbody, .tablewrap tr, .tablewrap td { display: block; width: 100%; }
  .tablewrap thead { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); white-space: nowrap; }
  .tablewrap tr { margin: 0 0 10px; padding: 4px 0; background: var(--panel); border: 1px solid var(--line); border-radius: 10px; }
  .tablewrap td { display: grid; grid-template-columns: 6.5em minmax(0, 1fr); gap: 10px; padding: 6px 12px; border: 0; min-width: 0; }
  .tablewrap td::before { content: attr(data-label); font: 600 10.5px/1.5 var(--mono); letter-spacing: .04em; text-transform: uppercase; color: var(--dim); padding-top: 3px; overflow-wrap: anywhere; }
  .tablewrap td:first-child { display: block; font-weight: 600; padding-top: 8px; padding-bottom: 8px; border-bottom: 1px solid var(--line); }
  .tablewrap td:first-child::before { display: none; }
  .tablewrap td:empty { display: none; }
  .tablewrap td:empty + td { display: block; font-weight: 600; padding-top: 8px; padding-bottom: 8px; border-bottom: 1px solid var(--line); }
  .tablewrap td:empty + td::before { display: none; }
  .tablewrap td code { white-space: normal; overflow-wrap: anywhere; }
}
@media print {
  nav.toc { display: none; }
  .tablewrap, pre { overflow: visible; }
  pre code { white-space: pre-wrap; }
}
"""


META_LABELS = {"doc_version": "Doc version", "loca_version": "Loca version", "code": "Code", "updated": "Updated"}


def page(doc: Doc, label: str, stamp: str) -> str:
    title = doc.title
    toc = ""
    if doc.toc:
        entries = "".join(f'<li><a href="#{hid}">{esc(text)}</a></li>' for hid, text in doc.toc)
        toc = f'<nav class="toc" aria-label="On this page"><h2>On this page</h2><ol>{entries}</ol></nav>'
    source = f"Generated from <code>{esc(label)}</code>"
    if stamp:
        source += f" at <code>{esc(stamp)}</code>"
    lede = f'<p class="lede">{esc(doc.summary)}</p>' if doc.summary else ""
    description = f'<meta name="description" content="{esc(doc.summary)}">' if doc.summary else ""
    meta = ""
    if doc.meta:
        cells = []
        for key, value in doc.meta:
            name = META_LABELS.get(key, key.replace("_", " ").capitalize())
            if key == "status" and value in BADGES:
                shown = f'<span class="badge {BADGES[value]}">{esc(value)}</span>'
            else:
                shown = inline(value)
            cells.append(f"<div><dt>{esc(name)}</dt><dd>{shown}</dd></div>")
        meta = f'<dl class="meta">{"".join(cells)}</dl>'
    return f"""<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{esc(title)}</title>
{description}
<meta name="generator" content="scripts/docs-to-page.py">
<style>{CSS}</style>
</head>
<body>
<div class="page">
<div class="layout">
<header class="dochead">
<p class="eyebrow">Loca · architecture</p>
<h1>{esc(title)}</h1>
{lede}
{meta}
<p class="source">{source}</p>
</header>
{toc}
<main>
{chr(10).join(doc.body)}
</main>
<footer class="docfoot">This page is generated. Edit <code>{esc(label)}</code> and run <code>python3 scripts/docs-to-page.py {esc(label)} OUT.html</code>.</footer>
</div>
</div>
</body>
</html>
"""


def main(argv: list[str]) -> int:
    if len(argv) != 3 or argv[1] in ("-h", "--help"):
        print(__doc__.strip().split("\n\n")[1].strip(), file=sys.stderr)
        return 2
    src, out = Path(argv[1]), Path(argv[2])
    try:
        text = src.read_text(encoding="utf-8")
    except OSError as error:
        print(f"cannot read {src}: {error}", file=sys.stderr)
        return 1
    doc = convert(text)
    if not doc.title:
        print(f"{src}: needs an H1 or a front-matter title", file=sys.stderr)
        return 1
    words = len(doc.title.split())
    if not 2 <= words <= 4:
        print(f"{src}: the title {doc.title!r} must be 2-4 words (set `title:` in front matter)", file=sys.stderr)
        return 1
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(page(doc, source_label(src), git_stamp(src.resolve())), encoding="utf-8")
    print(f"wrote {out} ({doc.title}, {len(doc.toc)} sections)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
