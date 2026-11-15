"""The kit listener accounts for every server frame in docs/protocol-frames.json.

Each frame type is either dispatched on by the listener or recorded in
``LOCA_FRAMES`` / ``LOBBY_FRAMES`` as deliberately ignored, with a reason. A new
server frame therefore fails here until somebody decides what the kit does
with it (docs/protocol.md).
"""

import ast
import importlib.util
import json
import sys
import unittest
from pathlib import Path


SKILL_DIR = Path(__file__).resolve().parents[1]
ROOT = SKILL_DIR.parents[1]
FRAME_LIST = ROOT / "docs" / "protocol-frames.json"
LISTENER_PATH = SKILL_DIR / "listen.py"

sys.path.insert(0, str(SKILL_DIR))
SPEC = importlib.util.spec_from_file_location("loca_listener_frames", LISTENER_PATH)
assert SPEC and SPEC.loader
LISTENER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(LISTENER)


def listed(socket, section="server_frames"):
    frames = json.loads(FRAME_LIST.read_text(encoding="utf-8"))[section]
    return {
        tag
        for tag, entry in frames.items()
        if not tag.startswith("$") and entry["socket"] == socket
    }


def dispatched(function_name, variable):
    """String constants the named listener function compares ``variable`` with."""
    tree = ast.parse(LISTENER_PATH.read_text(encoding="utf-8"))
    functions = [
        node
        for node in ast.walk(tree)
        if isinstance(node, ast.FunctionDef) and node.name == function_name
    ]
    assert len(functions) == 1, function_name
    tags = set()
    for node in ast.walk(functions[0]):
        if not (
            isinstance(node, ast.Compare)
            and isinstance(node.left, ast.Name)
            and node.left.id == variable
        ):
            continue
        for operand in node.comparators:
            values = operand.elts if isinstance(operand, (ast.Tuple, ast.List, ast.Set)) else [operand]
            for value in values:
                if isinstance(value, ast.Constant) and isinstance(value.value, str):
                    tags.add(value.value)
    return tags


class ProtocolFrameTableTests(unittest.TestCase):
    def check(self, table, socket, function_name, variable):
        # Every frame the server sends, plus any not-yet-sent frame the kit
        # already dispatches on, and nothing else.
        self.assertEqual(
            set(table) & listed(socket),
            listed(socket),
            f"{socket} frames the server sends, missing from listen.py",
        )
        self.assertLessEqual(
            set(table) - listed(socket),
            listed(socket, "not_sent_yet"),
            f"{socket} entries in listen.py that docs/protocol-frames.json does not list",
        )
        for tag, decision in table.items():
            if decision == "handled":
                continue
            self.assertTrue(decision.startswith("ignored: "), tag)
            self.assertGreater(len(decision), len("ignored: ") + 10, f"{tag} needs a reason")
        code = dispatched(function_name, variable)
        handled = {tag for tag, decision in table.items() if decision == "handled"}
        self.assertLessEqual(handled, code, f"'handled' frames {function_name}() never compares")
        self.assertLessEqual(code, set(table), f"{function_name}() dispatches on unlisted frames")

    def test_loca_socket_frames(self):
        self.check(LISTENER.LOCA_FRAMES, "loca", "watch", "t")

    def test_lobby_socket_frames(self):
        self.check(LISTENER.LOBBY_FRAMES, "lobby", "lobby_watch", "kind")


if __name__ == "__main__":
    unittest.main()
