const { test, expect } = require("@playwright/test");

// The Getting Started guide auto-shows on a fresh browser (empty localStorage)
// and its backdrop intercepts clicks. These specs drive the connected app, not
// onboarding, so start past the guide. (getting-started.spec.js deliberately
// does NOT seed this — it tests the first-open guide itself.)
test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    try {
      localStorage.setItem("loca-gs-seen", "1");
    } catch (e) {}
  });
});

// Faz 3 (desktop native notifications) event contract, tested at the shared-UI
// layer: which WS frames call window.__LOCA_NOTIFY__ (and with what), driven
// through the real onFrame dispatcher. The foreground-dedup and the actual OS
// notification are desktop-shell behaviour and are checked in loca-dev's runtime
// review — here we pin the classification the desktop relies on.
test("native-notify events: only mentions, directed attention and reminders notify — no body leaks", async ({
  page,
  request,
}) => {
  const admin = { "x-admin-token": "MASTER" };
  await request.post("/members", { headers: admin, data: { name: "notifyuser", kind: "user" } });
  const invited = await request.post("/rooms/e2e/invites", {
    headers: admin,
    data: { name: "notifyuser", kind: "user" },
  });
  const { token } = await invited.json();

  await page.goto("/");
  await page.locator("#name").fill("notifyuser");
  await page.locator("#roomToken").fill(token);
  await page.evaluate(() => window.doConnect("e2e"));
  await expect(page.locator("#curRoom")).toHaveText("e2e");
  const me = await page.locator("#name").inputValue();

  // Spy on the hook the desktop shell would provide.
  await page.evaluate(() => {
    window.__notifyLog = [];
    window.__LOCA_NOTIFY__ = (ev) => window.__notifyLog.push(ev);
  });

  // Drive a battery of frames through the real dispatcher.
  await page.evaluate((me) => {
    const now = Date.now();
    // own message -> never
    window.onFrame({ t: "msg", message: { id: 9001, sender: me, text: "my own line", ts: now } });
    // ordinary message from someone else -> never
    window.onFrame({ t: "msg", message: { id: 9002, sender: "bob", text: "hello everyone", ts: now } });
    // @me in the body -> mention
    window.onFrame({ t: "msg", message: { id: 9003, sender: "bob", text: "@" + me + " ping", ts: now } });
    // server-set target -> mention
    window.onFrame({ t: "msg", message: { id: 9004, sender: "carol", target: me, text: "direct", ts: now } });
    // a reaction -> never
    window.onFrame({ t: "reaction", reaction: { message_id: 9002, emoji: "✓", owner: "bob" } });
    // actionable reminder -> reminder
    window.onFrame({ t: "attention", attention: { id: "att-rem", reason: "goal_reminder", room: "e2e" } });
    // directed attention (non-reminder) -> attention
    window.onFrame({ t: "attention", attention: { id: "att-dir", reason: "poke", room: "e2e" } });
    // DUPLICATES (frame repeat / reconnect re-delivery): same ids again must NOT
    // produce a second notification.
    window.onFrame({ t: "msg", message: { id: 9003, sender: "bob", text: "@" + me + " ping", ts: now } });
    window.onFrame({ t: "attention", attention: { id: "att-rem", reason: "goal_reminder", room: "e2e" } });
  }, me);

  const log = await page.evaluate(() => window.__notifyLog);

  // Exactly four notifications: the two mentions, the reminder, and the directed
  // attention. Reactions, ordinary/own messages, AND the two duplicate frames
  // all produced nothing (dedup by event id).
  expect(log.map((e) => e.kind)).toEqual(["mention", "mention", "reminder", "attention"]);

  // Each event carries its unique id (for dedup + click-routing) and its sender.
  expect(log[0]).toMatchObject({ kind: "mention", sender: "bob", id: "9003" });
  expect(log[1]).toMatchObject({ kind: "mention", sender: "carol", id: "9004" });
  expect(log[2]).toMatchObject({ kind: "reminder", id: "att-rem" });
  expect(log[3]).toMatchObject({ kind: "attention", id: "att-dir" });

  // Privacy: only who + kind (+ routing ids) are ever forwarded — no body/text.
  for (const e of log) {
    expect(Object.prototype.hasOwnProperty.call(e, "text")).toBeFalsy();
    expect(Object.prototype.hasOwnProperty.call(e, "body")).toBeFalsy();
  }
});

// An "Everyone" reminder fans out to one per-member Attention each: they share a
// server-derived `group` (generation) id but carry DISTINCT attention ids, and
// every socket in the room receives all of them. The client must collapse the
// whole fan-out into ONE @all chat bubble and ONE native notification (keyed by
// `group`) — never one bubble/notification per member.
test("everyone reminder: many grouped Attention frames -> one @all bubble + one notification", async ({
  page,
  request,
}) => {
  const admin = { "x-admin-token": "MASTER" };
  await request.post("/members", { headers: admin, data: { name: "alluser", kind: "user" } });
  const invited = await request.post("/rooms/e2e/invites", {
    headers: admin,
    data: { name: "alluser", kind: "user" },
  });
  const { token } = await invited.json();

  await page.goto("/");
  await page.locator("#name").fill("alluser");
  await page.locator("#roomToken").fill(token);
  await page.evaluate(() => window.doConnect("e2e"));
  await expect(page.locator("#curRoom")).toHaveText("e2e");

  await page.evaluate(() => {
    window.__notifyLog = [];
    window.__LOCA_NOTIFY__ = (ev) => window.__notifyLog.push(ev);
  });

  // Drive three per-member reminder frames of ONE generation through the real
  // dispatcher: same `group`, distinct ids, distinct owners.
  await page.evaluate(() => {
    const now = Date.now();
    const group = "attention:e2e:silence:1000";
    for (const owner of ["alice", "bob", "carol"]) {
      window.onFrame({
        t: "attention",
        attention: {
          id: `${group}:${owner}-pid`,
          group,
          reason: "room_silence",
          room: "e2e",
          owner,
          subject: "the room has gone quiet",
          delivered_at: now,
          created_at: now,
          attempt: 1,
        },
      });
    }
  });

  // Exactly one native notification for the whole generation (group-keyed dedup),
  // carrying the shared group id, not any single member's attention id.
  const log = await page.evaluate(() => window.__notifyLog);
  expect(log.map((e) => e.kind)).toEqual(["reminder"]);
  expect(log[0]).toMatchObject({ kind: "reminder", id: "attention:e2e:silence:1000" });

  // Exactly one reminder chat bubble, addressed to @all — never one per member.
  const bubbles = page.locator("#feed .row.locareminder");
  await expect(bubbles).toHaveCount(1);
  await expect(bubbles).toContainText("@all");
  await expect(bubbles).not.toContainText("@alice");
});
