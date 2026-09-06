#!/usr/bin/env python3
"""End-to-end reminder DELIVERY harness — real server, real agents, real listeners.

Answers one question that unit tests keep failing to answer:
    "when a reminder fires, does it actually reach each agent's runtime?"

Nothing is mocked. It boots the real room-server binary, admits real members,
mints real davets/sessions, runs the real listen.py for every agent, triggers a
real room-silence reminder, and then reports, per agent, whether the delivery
landed in that agent's own listener output.

Usage:
    delivery_harness.py --binary target/debug/room-server --listener ~/.claude/skills/loca/listen.py
                        [--agents alice,bob,carol] [--lead alice] [--recipient all|lead]

Exit 0 = every agent received it. Exit 5 = at least one agent did not.
"""
import argparse
import json
import os
import pathlib
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

ADMIN = "MASTER"


def http(method, url, body=None, headers=None, timeout=10):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("content-type", "application/json")
    for k, v in (headers or {}).items():
        req.add_header(k, v)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            raw = r.read().decode()
            if not raw.strip():
                return r.status, None
            try:
                return r.status, json.loads(raw)
            except json.JSONDecodeError:
                return r.status, raw[:200]
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()[:200]


def wait_health(base, tries=60):
    for _ in range(tries):
        try:
            code, body = http("GET", f"{base}/health", timeout=2)
            if code == 200:
                return body
        except Exception:
            pass
        time.sleep(1)
    raise SystemExit("server did not come up")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--binary", required=True)
    ap.add_argument("--listener", required=True)
    ap.add_argument("--agents", default="alice,bob,carol")
    ap.add_argument("--lead", default="alice")
    ap.add_argument("--recipient", default="all", choices=["all", "lead"])
    ap.add_argument("--room", default="probe")
    ap.add_argument("--port", type=int, default=18970)
    ap.add_argument("--silence-secs", type=int, default=1)
    ap.add_argument("--wait", type=int, default=25, help="seconds to watch for delivery")
    ap.add_argument("--restart", action="store_true",
                    help="after a green round, restart the server and verify again")
    ap.add_argument("--cooldown-secs", type=int, default=10)
    ap.add_argument("--supervised", action="store_true",
                    help="run listeners under monitor_listener.py so they publish a health lease")
    args = ap.parse_args()

    agents = [a.strip() for a in args.agents.split(",") if a.strip()]
    # Namespace the workdir by whoever is running: several agents run this same
    # harness on one machine as the SAME unix user, so ownership does not tell
    # runs apart and their artefacts silently intermix in /tmp.
    who = os.environ.get("LOCA_NAME") or os.environ.get("USER") or "anon"
    work = pathlib.Path(tempfile.mkdtemp(
        prefix=f"delivery-harness-{who}-{time.strftime('%H%M%S')}-"))
    base = f"http://127.0.0.1:{args.port}"
    procs = []
    print(f"workdir: {work}")

    try:
        # 1. real server
        env = {
            **os.environ,
            "PORT": str(args.port),
            "BIND_ADDR": "127.0.0.1",
            "ADMIN_TOKEN": ADMIN,
            "DB_PATH": str(work / "harness.db"),
            "CARE_SILENCE_SECS": str(args.silence_secs),
            "CARE_COOLDOWN_SECS": str(args.cooldown_secs),
            "REQUIRE_SESSIONS": "1",
            "ROOM_TOKEN": "building",
            "LEGACY_WS_QUERY_AUTH": "0",
        }
        server_log = open(work / "server.log", "w")
        procs.append(subprocess.Popen([args.binary], env=env, stdout=server_log, stderr=server_log))
        info = wait_health(base)
        print(f"server up: version={info.get('version')}")

        # 2. real members + davets + sessions + identity envs
        admin_h = {"x-admin-token": ADMIN}
        http("POST", f"{base}/members", {"name": "harness", "kind": "user"}, admin_h)
        hc, hinv = http("POST", f"{base}/rooms/{args.room}/invites",
                        {"name": "harness"}, admin_h)
        if not isinstance(hinv, dict) or not hinv.get("token"):
            raise SystemExit(f"harness davet failed: {hc} {hinv}")
        _, hs = http("POST", f"{base}/sessions",
                     {"name": "harness", "kind": "user", "loca": args.room},
                     {"x-room-token": hinv["token"]})
        htok = hs.get("session_token") if isinstance(hs, dict) else None
        if not htok:
            raise SystemExit(f"harness session mint failed: {hs}")
        post_h = {**admin_h, "x-session-token": htok}
        envs = {}
        for name in agents:
            http("POST", f"{base}/members", {"name": name, "kind": "agent"}, admin_h)
            code, inv = http("POST", f"{base}/rooms/{args.room}/invites", {"name": name}, admin_h)
            if code not in (200, 201) or not isinstance(inv, dict):
                raise SystemExit(f"davet failed for {name}: {code} {inv}")
            davet = inv["token"]
            scode, sess = http("POST", f"{base}/sessions",
                               {"name": name, "kind": "agent", "loca": args.room},
                               {"x-room-token": davet})
            token = sess.get("session_token") if isinstance(sess, dict) else None
            if not token:
                raise SystemExit(f"session mint failed for {name}: {scode}")
            envp = work / f"{name}.env"
            envp.write_text(
                f"ROOM_SERVER_URL={base}\nLOCA_NAME={name}\n"
                f"DAVET_{args.room.replace('-', '_')}={davet}\n"
                f"LOCA_SESSION={token}\n",
                encoding="utf-8",
            )
            envp.chmod(0o600)
            envs[name] = envp
        print(f"admitted + davetted: {', '.join(agents)}")

        # 3. real listeners, one per agent, each writing its own delivery log
        ws = base.replace("http://", "ws://")
        supervisor = pathlib.Path(args.listener).with_name("monitor_listener.py")
        for name in agents:
            out = open(work / f"{name}.deliveries", "w")
            listener_cmd = [
                sys.executable, "-u", args.listener,
                f"{ws}/ws?room={args.room}&name={name}&type=agent&filter=mentions&turn_max=1",
                "/dev/stdout", "--skip-own", name,
                "--cursor", str(work / f"{name}.cursor"),
            ]
            if args.supervised and supervisor.exists():
                # Real agents run under the supervisor, which is what authorises
                # the listener to publish its runtime-health lease. A bare
                # listener never becomes a healthy Care owner.
                cmd = [sys.executable, "-u", str(supervisor), "--name", name,
                       "--log", str(work / f"{name}.monitor.log"),
                       "--lock", str(work / f"{name}.lock"), "--"] + listener_cmd
            else:
                cmd = listener_cmd
            procs.append(subprocess.Popen(
                cmd,
                env={**os.environ, "LOCA_ENV": str(envs[name])},
                stdout=out, stderr=open(work / f"{name}.listener.err", "w"),
            ))
        time.sleep(6)

        # 3b. CONTROL: prove each listener's delivery path works before we claim
        # a reminder failed. A red without a control is indistinguishable from a
        # broken harness.
        for name in agents:
            pc, pb = http("POST", f"{base}/rooms/{args.room}/messages",
                          {"sender": "harness", "sender_type": "user",
                           "text": f"@{name} control ping"}, post_h)
            if pc not in (200, 201):
                print(f"  control post for {name} REJECTED: {pc} {pb}")
        control_deadline = time.time() + 12
        control = {}
        while time.time() < control_deadline and len(control) < len(agents):
            for name in agents:
                if name not in control and "control ping" in (work / f"{name}.deliveries").read_text(errors="replace"):
                    control[name] = True
            time.sleep(1)
        print("control (direct mention reaches listener):")
        for name in agents:
            print(f"  {name:12} {'ok' if name in control else 'NO — harness/listener problem, not a reminder bug'}")
        if len(control) < len(agents):
            print("\nABORT: control failed, so a reminder result would be meaningless.")
            print(f"logs kept: {work}")
            return 2

        # 4. configure the reminder target
        http("PUT", f"{base}/rooms/{args.room}/settings",
             {"care_recipient": {"kind": args.recipient}}, admin_h)
        # The lead is NOT a settings field — it has its own operator-authorised
        # route. Passing it in the settings body is silently ignored, which
        # leaves lead mode with no owner at all and looks like a delivery bug.
        code, _ = http("POST", f"{base}/rooms/{args.room}/lead", {"lead": args.lead}, admin_h)
        if args.recipient == "lead" and code not in (200, 201, 204):
            print(f"ABORT: could not set lead ({code}); lead-mode result would be meaningless.")
            print(f"logs kept: {work}")
            return 2
        # Prove the settings actually applied rather than trusting the writes.
        _, applied = http("GET", f"{base}/rooms/{args.room}/settings", headers=admin_h)
        if isinstance(applied, dict):
            print(f"applied: care_recipient={applied.get('care_recipient')} "
                  f"lead={applied.get('lead')}")

        # `all` must reach every agent; `lead` must reach the lead and NOBODY
        # ELSE. Demanding 3/3 in lead mode would be a false red, and ignoring the
        # non-recipients would hide a real leak — so both directions are asserted.
        expected = set(agents) if args.recipient == "all" else {args.lead}

        def run_round(label):
            """Arm one reminder and report delivery. Returns an exit code."""
            # Round 2 runs against files that already hold round 1's text, so
            # only the NEW tail counts as evidence.
            marks = {n: (work / f"{n}.deliveries").stat().st_size for n in agents}
            http("POST", f"{base}/rooms/{args.room}/messages",
                 {"sender": "harness", "sender_type": "user",
                  "text": f"arming silence ({label})"}, post_h)
            print(f"\n[{label}] armed: recipient={args.recipient} lead={args.lead}; "
                  f"watching {args.wait}s")
            got = {}
            # Wait for the expected recipients, not every connected agent.
            # Lead mode intentionally targets only one agent, so waiting for
            # all of them would consume the retry budget before restart.
            deadline = time.time() + args.wait
            while time.time() < deadline and not expected.issubset(got.keys()):
                for name in agents:
                    if name in got:
                        continue
                    with open(work / f"{name}.deliveries", errors="replace") as fh:
                        fh.seek(marks[name])
                        fresh = fh.read()
                    if "room_silence" in fresh or "silence check" in fresh:
                        got[name] = True
                time.sleep(1)
            # Keep observing non-recipients briefly so the early success exit
            # cannot hide a delayed fan-out leak.
            if expected.issubset(got.keys()) and len(expected) < len(agents):
                leak_deadline = time.time() + 5
                while time.time() < leak_deadline:
                    for name in agents:
                        if name in got:
                            continue
                        with open(work / f"{name}.deliveries", errors="replace") as fh:
                            fh.seek(marks[name])
                            fresh = fh.read()
                        if "room_silence" in fresh or "silence check" in fresh:
                            got[name] = True
                    time.sleep(1)

            print(f"=== DELIVERY RESULT [{label}] ===")
            for name in agents:
                want = "expect" if name in expected else "expect NO"
                hit = "RECEIVED" if name in got else "not received"
                ok = (name in got) == (name in expected)
                print(f"  {name:12} {hit:14} ({want}) {'ok' if ok else '<-- WRONG'}")
            missing = [a for a in agents if a in expected and a not in got]
            leaked = [a for a in agents if a not in expected and a in got]
            if leaked:
                print(f"  LEAK: {', '.join(leaked)} received a '{args.recipient}' "
                      "reminder they were not the recipient of")
            _, atts = http("GET", f"{base}/rooms/{args.room}/attentions", headers=admin_h)
            owned = unowned = 0
            if isinstance(atts, list):
                print(f"  attentions on server: {len(atts)}")
                for a in atts:
                    owner = a.get("owner")
                    print(f"    - owner={owner!r} status={a.get('status')!r} "
                          f"attempt={a.get('attempt')} delivered_at={a.get('delivered_at')}")
                    if owner:
                        owned += 1
                    else:
                        unowned += 1

            # The decisive diagnostic: an attention with NO owner was never meant
            # to be delivered (no health-ready recipient existed), which is
            # correct behaviour and an environment result — not a transport
            # defect. An owned attention that still never landed is the real bug.
            if missing and owned == 0 and unowned > 0:
                print(f"\nINCONCLUSIVE [{label}]: every attention was created with NO")
                print("owner, so the server correctly had nobody health-ready to")
                print("deliver to. This is an environment result, NOT proof of a")
                print("delivery defect. Fix runtime health before reporting a bug.")
                return 3
            # Receipt is a SEPARATE assertion from arrival. delivered_at is
            # stamped only when the client's ACK authenticates, so a broken ACK
            # path leaves the product unable to prove a delivery it actually
            # made — which is the failure the operator complained about most.
            # Printing it is not asserting it: assert it.
            unreceipted = []
            if isinstance(atts, list):
                for name in expected:
                    if name not in got:
                        continue
                    if not any(a.get("owner") == name
                               and a.get("status") == "open"
                               and a.get("delivered_at")
                               for a in atts):
                        unreceipted.append(name)
            if unreceipted:
                print(f"  NO RECEIPT: {', '.join(unreceipted)} received the frame but "
                      "no ACK stamped delivered_at (product cannot prove delivery)")
            if missing or leaked or unreceipted:
                if missing:
                    print(f"\nFAIL [{label}]: {len(missing)}/{len(expected)} expected "
                          f"recipient(s) never got it: {', '.join(missing)}")
                if unreceipted and not missing:
                    print(f"\nFAIL [{label}]: delivered but unproven for "
                          f"{', '.join(unreceipted)}")
                return 5
            print(f"  PASS [{label}]: reached exactly its {len(expected)} expected "
                  f"recipient(s): {', '.join(sorted(expected))}")
            return 0

        rc = run_round("round 1")
        if rc != 0 or not args.restart:
            if rc != 0:
                print(f"logs kept: {work}")
            return rc

        # 5. restart persistence: a reminder path that only works until the
        # server bounces is not fixed. Same DB, same room, same agents.
        print("\n--- restarting server (same DB), re-verifying ---")
        server = procs[0]
        server.send_signal(signal.SIGTERM)
        try:
            server.wait(timeout=15)
        except Exception:
            server.kill()
        time.sleep(args.cooldown_secs + 2)  # let the care cooldown lapse
        procs[0] = subprocess.Popen([args.binary], env=env, stdout=server_log, stderr=server_log)
        info = wait_health(base)
        print(f"server back up: version={info.get('version')}")
        time.sleep(8)  # listeners reconnect and re-publish their health lease

        rc = run_round("round 2 (after restart)")
        if rc != 0:
            print(f"logs kept: {work}")
        return rc
    finally:
        for p in procs:
            try:
                p.send_signal(signal.SIGTERM)
            except Exception:
                pass
        time.sleep(1)
        for p in procs:
            try:
                p.kill()
            except Exception:
                pass


if __name__ == "__main__":
    raise SystemExit(main())
