"""An agent is reachable in the Lobby room like in a loca.

PRINCIPLES.md, "Lobby": the Lobby room is the one room every Building member
enters by membership alone. An agent holding only its membership key hears a
message there that names it (or a rank's ``@all``) as a turn of its own, in
the same durable turn log as a loca's, with room ``lobby``; a message that
names nobody does not wake it; and ``connect.sh send <server> lobby <name>``
answers there with the membership key and a Building session, never an
invite.
"""
import json
import os
import subprocess
import sys
import tempfile
import time
import unittest
import urllib.request
from pathlib import Path

import test_listener_backfill_e2e as harness
from test_listener_backfill_e2e import SKILL_DIR, REPO_ROOT, free_port, request_json

SKILL_DIR = Path(os.environ.get("LOCA_TEST_SKILL_DIR", str(SKILL_DIR)))
sys.path.insert(0, str(SKILL_DIR))

import codex_adapter_v2  # noqa: E402
import credentials  # noqa: E402
import listen  # noqa: E402


class LobbyRoomCredentialTests(unittest.TestCase):
    def setUp(self):
        self.saved = dict(os.environ)
        listen.ROOM_SESSIONS.clear()

    def tearDown(self):
        os.environ.clear()
        os.environ.update(self.saved)
        listen.ROOM_SESSIONS.clear()

    def test_the_lobby_room_is_opened_by_the_membership_key_never_an_invite(self):
        os.environ.update(
            LOCA_MEMBERSHIP="mb_key",
            DAVET_lobby="dv_should_never_be_used",
            DAVET_workshop="dv_workshop",
            LOCA_SESSION="st_workshop_session",
        )
        self.assertEqual(credentials.token_for_room("lobby"), "mb_key")
        self.assertEqual(credentials.token_for_room("workshop"), "dv_workshop")
        # A loca's session would close the Lobby's door; none is presented.
        self.assertIsNone(listen.session_for_room("lobby"))
        self.assertEqual(
            listen.room_protocols("lobby"), ["loca.v1", "loca.room.mb_key"]
        )
        self.assertEqual(
            listen.session_for_room("workshop"), "st_workshop_session"
        )

    def test_in_the_lobby_only_a_name_or_a_kept_target_all_calls(self):
        name = "lobby-agent"
        self.assertTrue(listen.lobby_addresses({"text": "@lobby-agent hi"}, name))
        self.assertTrue(listen.lobby_addresses({"target": "lobby-agent", "text": "hi"}, name))
        self.assertTrue(listen.lobby_addresses({"reply_to_sender": name, "text": "ok"}, name))
        self.assertTrue(listen.lobby_addresses({"target": "all", "text": "@all standup"}, name))
        # A member's `@all` reaches the store without `target: all`.
        self.assertFalse(listen.lobby_addresses({"text": "@all hello"}, name))
        self.assertFalse(listen.lobby_addresses({"text": "hello, building"}, name))
        self.assertFalse(
            listen.lobby_addresses({"target": "all", "kind": "announce", "text": "x"}, name)
        )
        # REST backfill mirrors it for the Lobby and leaves a loca as it was.
        self.assertFalse(listen.addresses({"text": "@all hello"}, name, "lobby"))
        self.assertTrue(listen.addresses({"text": "@all hello"}, name, "workshop"))

    def test_codex_names_the_lobby_room_as_shared(self):
        self.assertIn("Lobby room", codex_adapter_v2.room_phrase("lobby"))
        self.assertEqual(codex_adapter_v2.room_phrase("workshop"), "private room workshop")


class LobbyRoomWakeEndToEndTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        harness.DurableBackfillEndToEndTests.setUpClass()

    def until(self, check, message, seconds=10):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            try:
                if check():
                    return
            except (OSError, ValueError):
                pass
            time.sleep(0.05)
        self.fail(message)

    @staticmethod
    def texts(event):
        if event.get("t") == "turn":
            return [m.get("text") for m in event.get("messages") or []]
        return [event.get("text")]

    def test_a_member_agent_wakes_by_name_in_the_lobby_room_and_answers_there(self):
        server = listener = None
        server_log = None
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            port = free_port()
            base = f"http://127.0.0.1:{port}"
            try:
                # Sessions required, as in production: a Lobby post needs the
                # Building session the kit takes with the membership key.
                server, server_log = harness.DurableBackfillEndToEndTests.start_server(
                    self, port, root / "server.db", root / "server.log",
                    extra_env={"REQUIRE_SESSIONS": "1", "REQUIRE_INVITE": "1"})
                agent = request_json(f"{base}/members", method="POST", admin_token="e2e-admin",
                                     body={"name": "lobby-agent", "kind": "agent"})
                speaker = request_json(f"{base}/members", method="POST", admin_token="e2e-admin",
                                       body={"name": "deniz", "kind": "agent"})
                env_file = root / "agent.env"
                env_file.write_text(f"ROOM_SERVER_URL={base}\nLOCA_NAME=lobby-agent\n"
                                    f"LOCA_MEMBERSHIP={agent['token']}\n", encoding="utf-8")
                env_file.chmod(0o600)

                def session(header, value, name):
                    request = urllib.request.Request(
                        f"{base}/sessions", method="POST",
                        data=json.dumps({"name": name, "kind": "agent"}).encode())
                    request.add_header("content-type", "application/json")
                    request.add_header(header, value)
                    with urllib.request.urlopen(request, timeout=10) as response:
                        return json.loads(response.read())["session_token"]

                speaker_session = session("x-room-token", speaker["token"], "deniz")
                master_session = session("x-admin-token", "e2e-admin", "master")

                def say(session_token, sender, text, target=None):
                    body = {"sender": sender, "sender_type": "agent", "text": text}
                    if target:
                        body["target"] = target
                    request = urllib.request.Request(
                        f"{base}/rooms/lobby/messages", method="POST",
                        data=json.dumps(body).encode())
                    request.add_header("content-type", "application/json")
                    request.add_header("x-session-token", session_token)
                    with urllib.request.urlopen(request, timeout=10) as response:
                        return json.loads(response.read())

                stdout = root / "stdout.jsonl"
                inbox = root / "turns.jsonl"
                cursor = root / "cursor.json"
                lines = lambda: [json.loads(line) for line in stdout.read_text().splitlines()
                                 if line.strip()] if stdout.exists() else []
                records = lambda: [json.loads(line) for line in inbox.read_text().splitlines()
                                   if line.strip()] if inbox.exists() else []
                env = {**os.environ, "LOCA_ENV": str(env_file)}
                with stdout.open("ab") as output, (root / "listener.log").open("ab") as errors:
                    listener = subprocess.Popen(
                        ["python3", "-u", str(SKILL_DIR / "listen.py"),
                         f"ws://127.0.0.1:{port}/ws?room=&name=lobby-agent&type=agent"
                         "&filter=mentions&turn_idle_ms=50",
                         "-", "--skip-own", "lobby-agent",
                         "--cursor", str(cursor), "--turn-log", str(inbox)],
                        cwd=REPO_ROOT, env=env, stdout=output, stderr=errors)
                self.until(lambda: any(m["name"] == "lobby-agent" for m in request_json(
                    f"{base}/rooms/lobby/members", admin_token="e2e-admin")),
                    "the listener did not enter the Lobby room")

                # Nobody named: no wake. A member's `@all` calls nobody here.
                say(speaker_session, "deniz", "hello, building")
                say(speaker_session, "deniz", "@all anyone around?", target="all")
                time.sleep(1.5)
                self.assertEqual(records(), [], "an unaddressed Lobby message woke the agent")
                self.assertEqual(lines(), [], "an unaddressed Lobby message reached Monitor")

                # Named: one delivery, room `lobby`, in the durable turn log and
                # on Monitor stdout alike.
                asked = say(speaker_session, "deniz", "@lobby-agent can you check the build?")
                self.until(lambda: len(records()) == 1, "@lobby-agent did not wake the agent")
                delivery = records()[0]
                self.assertEqual(delivery["room"], "lobby")
                self.assertEqual(delivery["identity"], "lobby-agent")
                self.assertEqual(delivery["delivery_id"], f"lobby:{asked['id']}")
                self.assertEqual(self.texts(delivery["event"]),
                                 ["@lobby-agent can you check the build?"])
                self.until(lambda: len(lines()) == 1, "the Lobby turn did not reach Monitor stdout")
                self.assertEqual(self.texts(lines()[0]), ["@lobby-agent can you check the build?"])

                # A rank's `@all` calls everyone, the agent included.
                called = say(master_session, "master", "@all standup in five")
                self.assertEqual(called.get("target"), "all")
                self.until(lambda: len(records()) == 2, "a rank's @all did not wake the agent")
                self.assertEqual(self.texts(records()[1]["event"]), ["@all standup in five"])

                # The agent answers in the Lobby with connect.sh send: the
                # membership key and a Building session taken with it.
                sent = subprocess.run(
                    ["bash", str(SKILL_DIR / "connect.sh"), "send", base, "lobby",
                     "lobby-agent", "deniz", "build is green"],
                    cwd=REPO_ROOT, env={**env, "LOCA_OP_ID": "lobby-reply-1"},
                    capture_output=True, text=True, timeout=20)
                self.assertEqual(sent.returncode, 0, sent.stderr)
                receipt = json.loads(sent.stdout)
                self.assertEqual(receipt["room"], "lobby")
                self.assertEqual(receipt["sender"], "lobby-agent")
                history = request_json(f"{base}/rooms/lobby/messages", admin_token="e2e-admin")
                self.assertIn("build is green", [m["text"] for m in history])
                # Its own answer wakes nobody here, itself included.
                time.sleep(1.0)
                self.assertEqual(len(records()), 2)

                # A second send reuses the kept Building session.
                kept = env_file.read_text(encoding="utf-8")
                self.assertIn("LOCA_LOBBY_SESSION=", kept)
                again = subprocess.run(
                    ["bash", str(SKILL_DIR / "connect.sh"), "send", base, "lobby",
                     "lobby-agent", "-", "one more"],
                    cwd=REPO_ROOT, env={**env, "LOCA_OP_ID": "lobby-reply-2"},
                    capture_output=True, text=True, timeout=20)
                self.assertEqual(again.returncode, 0, again.stderr)
                self.assertNotIn("session renewed", again.stderr)
                self.assertEqual(env_file.read_text(encoding="utf-8"), kept)

                # Reading the Lobby takes the key alone.
                read = subprocess.run(
                    ["bash", str(SKILL_DIR / "connect.sh"), "since", base, "lobby", "0"],
                    cwd=REPO_ROOT, env=env, capture_output=True, text=True, timeout=20)
                self.assertEqual(read.returncode, 0, read.stderr)
                self.assertIn("one more", [m["text"] for m in json.loads(read.stdout)])

                # The Lobby room is never an invite or a seat; its cursor is kept.
                self.assertNotIn("DAVET_lobby", kept)
                self.assertNotIn("LOCA_SESSION=", kept)
                self.until(lambda: json.loads(cursor.read_text()).get("lobby", 0) >= called["id"],
                           "the Lobby room cursor was not kept")
                self.assertIsNone(listener.poll(), "the listener exited")
            finally:
                harness.DurableBackfillEndToEndTests.stop_process(listener)
                harness.DurableBackfillEndToEndTests.stop_process(server, server_log)

    def test_a_caretaker_stays_out_of_the_lobby_room(self):
        server = listener = None
        server_log = None
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            port = free_port()
            base = f"http://127.0.0.1:{port}"
            try:
                # A name the kit does not know by heart: /health says it.
                server, server_log = harness.DurableBackfillEndToEndTests.start_server(
                    self, port, root / "server.db", root / "server.log",
                    extra_env={"LOCA_CARETAKERS": "keeper"})
                keeper = request_json(f"{base}/members", method="POST", admin_token="e2e-admin",
                                      body={"name": "keeper", "kind": "agent"})
                env_file = root / "keeper.env"
                env_file.write_text(f"ROOM_SERVER_URL={base}\nLOCA_NAME=keeper\n"
                                    f"LOCA_MEMBERSHIP={keeper['token']}\n", encoding="utf-8")
                env_file.chmod(0o600)
                log = root / "listener.log"
                inbox = root / "turns.jsonl"
                with (root / "stdout.jsonl").open("ab") as output, log.open("ab") as errors:
                    listener = subprocess.Popen(
                        ["python3", "-u", str(SKILL_DIR / "listen.py"),
                         f"ws://127.0.0.1:{port}/ws?room=&name=keeper&type=agent&filter=mentions",
                         "-", "--only-direct", "keeper", "--turn-log", str(inbox)],
                        cwd=REPO_ROOT, env={**os.environ, "LOCA_ENV": str(env_file)},
                        stdout=output, stderr=errors)
                self.until(lambda: "[lobby] connected" in log.read_text(errors="replace"),
                           "the caretaker's Lobby presence did not connect")
                request_json(f"{base}/rooms/lobby/messages", method="POST", admin_token="e2e-admin",
                             body={"sender": "master", "sender_type": "user",
                                   "text": "@keeper hello"})
                time.sleep(2.0)
                self.assertNotIn("[lobby] listening", log.read_text(errors="replace"))
                self.assertFalse(inbox.exists() and inbox.read_text().strip(),
                                 "a caretaker was woken from the Lobby room")
                self.assertIsNone(listener.poll(), "the listener exited")
            finally:
                harness.DurableBackfillEndToEndTests.stop_process(listener)
                harness.DurableBackfillEndToEndTests.stop_process(server, server_log)


if __name__ == "__main__":
    unittest.main()
