import importlib.util
import unittest
from pathlib import Path
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[3]
AGENTD_PATH = ROOT / "adapters" / "generic-command" / "agentd.py"
SPEC = importlib.util.spec_from_file_location("loca_generic_agentd", AGENTD_PATH)
AGENTD = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(AGENTD)


class GenericAgentContextTests(unittest.TestCase):
    def test_active_goal_joins_the_same_generic_delivery_context(self):
        def fake_api(method, url, token=None, body=None):
            self.assertEqual(method, "GET")
            self.assertEqual(token, "room-token")
            if url.endswith("/messages"):
                return [{"id": 1}, {"id": 2}]
            if url.endswith("/goals"):
                return [
                    {"id": 1, "status": "achieved", "outcome": "old"},
                    {
                        "id": 2,
                        "status": "active",
                        "outcome": "publish the verified release",
                        "checkpoint": "review is green",
                    },
                ]
            if url.endswith("/notes"):
                return [{"key": "release"}]
            self.fail(f"unexpected URL: {url}")

        with patch.object(AGENTD, "api", side_effect=fake_api):
            context = AGENTD.fetch_context(
                "https://loca.example", "review", "room-token", 1, True
            )

        self.assertEqual(context["messages"], [{"id": 2}])
        self.assertEqual(context["notes"], [{"key": "release"}])
        self.assertEqual(context["goal"]["id"], 2)

    def test_missing_goal_does_not_drop_the_delivery_context(self):
        def fake_api(method, url, token=None, body=None):
            if url.endswith("/messages"):
                return [{"id": 7}]
            if url.endswith("/goals"):
                raise OSError("temporary resolver failure")
            return []

        with patch.object(AGENTD, "api", side_effect=fake_api):
            context = AGENTD.fetch_context(
                "https://loca.example", "review", "room-token", 10, False
            )

        self.assertEqual(context["messages"], [{"id": 7}])
        self.assertIsNone(context["goal"])


if __name__ == "__main__":
    unittest.main()
