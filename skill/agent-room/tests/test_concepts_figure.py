"""The Building map (docs/concepts-at-a-glance.svg) is drawn once and shown
twice: on GitHub through docs/concepts.md, and in the web interface's Getting
Started panel (web/index.html #gsMap). These tests keep the copies the same
and keep the map's words the interface's words."""

import re
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
SVG = ROOT / "docs/concepts-at-a-glance.svg"
INDEX = ROOT / "web/index.html"
STYLES = ROOT / "web/assets/guide.css"
CONCEPTS = ROOT / "docs/concepts.md"

STYLE_ELEMENT = re.compile(r"<style>\n(.*?)</style>\n", re.S)
# The map's place in index.html: the whole <figure>, words included.
FIGURE = re.compile(r'<figure class="gsmap" id="gsMap">.*?</figure>', re.S)

# Every interface term the map shows. A rename in the interface must fail
# here, so each is looked for in what the interface says outside the map.
# Matched as whole words; the first letter may be either case (a heading
# says "Floor", a sentence "floors") and a plural "s" may follow.
TERMS = (
    "Building", "Master", "Smaster", "Lobby", "Ground floor", "Floor", "loca",
    "loca-care", "caretaker", "Messages", "invite", "call", "release",
    "Operator", "Lead", "participant", "Chat", "Notes", "Focus", "Journal",
    "Loca Wiki", "Live", "Vault", "Loca Board", "extension", "member", "agent", "people", "rank",
    "key", "notification", "seat",
)
# Plain English the map uses around those terms. A word that is in neither
# list fails the vocabulary test, so a new term cannot slip into the map
# without being held to the interface.
PLAIN = {
    "a", "admit", "advises", "always", "and", "appoint", "are", "at", "be",
    "called", "cannot", "drawn", "enter", "everyone", "everything", "final",
    "for", "found", "gave", "give", "gives", "has", "holds", "in", "is", "it",
    "its", "manage", "mode", "moderation", "most", "no", "on", "one", "only",
    "own", "owns", "private", "revoke", "runs", "see", "sits", "the", "them",
    "they", "to", "turns", "two", "wait", "who", "word", "work", "deputies",
    "with", "by", "or", "everyone's", "an", "within", "bounds",
}


def figure_svg_source():
    return SVG.read_text(encoding="utf-8")


def svg_without_style(source):
    return STYLE_ELEMENT.sub("", source, count=1).rstrip("\n")


def shown_words(svg):
    """The text the map draws: its title and every <text>."""
    parts = re.findall(r"<title[^>]*>(.*?)</title>", svg, re.S)
    parts += re.findall(r"<text[^>]*>(.*?)</text>", svg, re.S)
    return " ".join(re.sub(r"<[^>]+>", "", part) for part in parts)


def shown_lines(svg):
    return [re.sub(r"<[^>]+>", "", part) for part in re.findall(r"<text[^>]*>(.*?)</text>", svg, re.S)]


def term_pattern(term):
    first, rest = term[0], re.escape(term[1:])
    return re.compile(r"(?<![\w-])[" + first.upper() + first.lower() + "]" + rest + r"s?(?![\w-])")


def interface_text():
    """What the interface says, apart from the map itself: index.html without
    its comments and the map's <figure>, and the string literals of
    web/assets/*.js (identifiers and comments do not count)."""
    html = INDEX.read_text(encoding="utf-8")
    html = re.sub(r"<!--.*?-->", "", html, flags=re.S)
    html = FIGURE.sub("", html)
    token = re.compile(
        r"//[^\n]*|/\*.*?\*/|\"(?:\\.|[^\"\\\n])*\"|'(?:\\.|[^'\\\n])*'|`(?:\\.|[^`\\])*`",
        re.S,
    )
    strings = []
    for path in sorted((ROOT / "web/assets").glob("*.js")):
        for match in token.finditer(path.read_text(encoding="utf-8")):
            text = match.group(0)
            if not text.startswith("/"):
                strings.append(text)
    return html + "\n" + "\n".join(strings)


class ConceptsFigureTests(unittest.TestCase):
    def test_interface_carries_the_docs_figure_less_its_style_element(self):
        source = figure_svg_source()
        self.assertEqual(len(STYLE_ELEMENT.findall(source)), 1)
        html = INDEX.read_text(encoding="utf-8")
        figure = FIGURE.search(html)
        self.assertIsNotNone(figure, "web/index.html has no #gsMap figure")
        found = re.search(r"<svg class=\"locamap\".*?</svg>", figure.group(0), re.S)
        self.assertIsNotNone(found, "the #gsMap figure carries no map")
        self.assertEqual(found.group(0), svg_without_style(source))
        # The shell carries no style element (crates/server/tests/ws/server.rs).
        self.assertNotIn("<style", html)

    def test_interface_styles_are_the_figure_style_element(self):
        rules = STYLE_ELEMENT.search(figure_svg_source()).group(1)
        css = STYLES.read_text(encoding="utf-8")
        block = re.search(r"/\* concepts map: begin \*/\n(.*?)/\* concepts map: end \*/\n", css, re.S)
        self.assertIsNotNone(block, "guide.css has no concepts map block")
        self.assertEqual(block.group(1), rules)

    def test_concepts_doc_shows_the_figure_before_the_long_prose(self):
        doc = CONCEPTS.read_text(encoding="utf-8")
        image = doc.find("](concepts-at-a-glance.svg)")
        self.assertGreater(image, 0, "docs/concepts.md does not show the figure")
        self.assertLess(image, doc.find("## Identity, login, and authority"))

    def test_every_term_the_figure_shows_is_one_the_interface_uses(self):
        words = shown_words(figure_svg_source())
        interface = interface_text()
        missing = [t for t in TERMS if term_pattern(t).search(words) and not term_pattern(t).search(interface)]
        self.assertEqual(missing, [], "the map shows words the interface no longer uses")
        # Every word on the map is either such a term or plain English.
        covered = words
        for term in sorted(TERMS, key=len, reverse=True):
            covered = term_pattern(term).sub(" ", covered)
        unknown = sorted(
            {w for w in re.findall(r"[^\W\d_][\w'’]*", covered) if w.lower() not in PLAIN}
        )
        self.assertEqual(unknown, [], "words on the map that are neither an interface term nor plain")

    def test_the_figure_lists_the_extension_catalog(self):
        # The extension names come from the server's catalog, not from the
        # web's own strings, so they are held to the catalog itself.
        lines = shown_lines(figure_svg_source())
        start = next(i for i, line in enumerate(lines) if line.endswith("Extensions:"))
        shown = [name.strip() for line in lines[start + 1:] for name in line.split("·")]
        protocol = (ROOT / "crates/protocol/src/lib.rs").read_text(encoding="utf-8")
        catalog = protocol.split("pub const EXTENSIONS", 1)[1].split("];", 1)[0]
        self.assertEqual(shown, re.findall(r'\bname: "([^"]+)"', catalog))


if __name__ == "__main__":
    unittest.main()
