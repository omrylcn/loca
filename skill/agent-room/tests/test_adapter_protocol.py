import importlib.util
import json
import os
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock


REPO = Path(__file__).resolve().parents[3]


def load_module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


AGENTD = load_module(
    "loca_generic_adapter_v1",
    REPO / "adapters" / "generic-command" / "agentd.py",
)
BOT = load_module(
    "loca_bot_adapter_v1",
    REPO / "skill" / "agent-room" / "bot.py",
)


def envelope():
    return {
        "protocol_version": "1",
        "delivery_id": "backend:152",
        "server": "https://loca.example",
        "room": "backend",
        "identity": "test-runner",
        "priority": "direct_user",
        "attempt": 1,
        "deadline_ms": 4000,
        "last_id": 152,
        "event": {
            "id": 152,
            "sender": "operator",
            "sender_type": "user",
            "target": "test-runner",
            "text": "run tests",
        },
    }


class AdapterProtocolTests(unittest.TestCase):
    def test_generic_adapter_receives_v1_and_posts_only_after_command_success(self):
        args = SimpleNamespace(
            room="backend",
            name="test-runner",
            server="https://loca.example",
            cmd="runner",
            context=15,
            notes=False,
            timeout=180,
        )
        with mock.patch.dict(
            os.environ,
            {"LOCA_DELIVERY": json.dumps(envelope())},
            clear=False,
        ), mock.patch.object(
            AGENTD, "token_for_room", return_value="davet"
        ), mock.patch.object(
            AGENTD, "fetch_context", return_value={"messages": [], "notes": []}
        ), mock.patch.object(
            AGENTD,
            "run_command",
            return_value='{"text":"green"}',
        ) as command, mock.patch.object(
            AGENTD, "post_reply"
        ) as post:
            self.assertEqual(AGENTD.invoke(args), 0)

        payload = command.call_args.args[1]
        self.assertEqual(payload["protocol_version"], "1")
        self.assertEqual(payload["delivery_id"], "backend:152")
        self.assertEqual(payload["trigger"]["id"], 152)
        post.assert_called_once()

    def test_generic_adapter_rejects_unknown_protocol_without_running_command(self):
        bad = envelope()
        bad["protocol_version"] = "2"
        args = SimpleNamespace(
            room="backend",
            name="test-runner",
            server="https://loca.example",
            cmd="runner",
            context=15,
            notes=False,
            timeout=180,
        )
        with mock.patch.dict(
            os.environ, {"LOCA_DELIVERY": json.dumps(bad)}, clear=False
        ), mock.patch.object(AGENTD, "run_command") as command:
            with self.assertRaisesRegex(RuntimeError, "unsupported"):
                AGENTD.invoke(args)
        command.assert_not_called()

    def test_bot_uses_stable_delivery_operation_id(self):
        args = SimpleNamespace(
            room="backend",
            name="test-runner",
            server="https://loca.example",
            brain="brain",
            context=5,
            system="",
        )
        with tempfile.TemporaryDirectory() as tmp, mock.patch.dict(
            os.environ,
            {
                "LOCA_DELIVERY": json.dumps(envelope()),
                "LOCA_ENV": str(Path(tmp) / "test-runner.env"),
            },
            clear=False,
        ), mock.patch.object(
            BOT, "token_for_room", return_value="davet"
        ), mock.patch.object(
            BOT, "recent", return_value=[]
        ), mock.patch.object(
            BOT, "think", return_value="done"
        ), mock.patch.object(
            BOT.subprocess, "run"
        ) as post:
            self.assertEqual(BOT.invoke(args), 0)

        sent_env = post.call_args.kwargs["env"]
        self.assertEqual(sent_env["LOCA_OP_ID"], "loca-backend:152")
        self.assertEqual(sent_env["LOCA_REPLY_TO"], "152")


if __name__ == "__main__":
    unittest.main()
