const { test, expect } = require("@playwright/test");

// Wait and Reminder are core. Where Focus is off, its tab still holds
// "Waiting on" and "Reminders": always for whoever runs the loca, and for
// everyone once the loca has a wait or a reminder rule on. The goal, the
// tasks, and the reminder rules about them stay Focus's.

const MASTER = { "x-admin-token": "MASTER" };

async function masterPage(browser, request) {
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const info = await (await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" },
  })).json();
  const page = await browser.newPage();
  await page.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "Master" }));
  }, { token: info.session_token, expiresAt: info.expires_at });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  return page;
}

async function memberPage(browser, request, name, room) {
  const { token } = await (await request.post("/members", { headers: MASTER, data: { name, kind: "user" } })).json();
  const invited = await request.post(`/rooms/${room}/invites`, { headers: MASTER, data: { name } });
  expect(invited.ok(), await invited.text()).toBeTruthy();
  const page = await browser.newPage();
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await page.goto("/");
  await page.locator("#entryKind").selectOption("human");
  await page.locator("#name").fill(name);
  await page.locator("#roomToken").fill(token);
  await page.locator("#connectBtn").click();
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate(room => joinRoom(room), room);
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  return page;
}

const visibleTabs = page => page.locator(".tabs [role=tab]:visible").allTextContents();
const ruleRow = (page, id) => page.locator(".reminderrow", { has: page.locator(`#${id}`) });

test("without Focus the operator keeps Waiting on and Reminders, and no goal or task", async ({ browser, request }) => {
  const room = "core-operator";
  const made = await request.post("/rooms", { headers: MASTER, data: { name: room, extensions: ["notes", "journal"] } });
  expect([201, 409]).toContain(made.status());
  const page = await masterPage(browser, request);
  await page.evaluate(room => joinRoom(room), room);
  await expect.poll(() => page.evaluate(() => state.ws?.readyState)).toBe(1);
  await expect.poll(() => visibleTabs(page)).toEqual(["Chat", "Notes", "Focus", "Journal"]);

  await page.locator("#tabTasks").click();
  await expect(page.locator("#tasksPanel")).toBeVisible();
  await expect(page.locator("#reminderSettings")).toBeVisible();
  await expect(page.locator("#focusGoalSection")).toBeHidden();
  await expect(page.locator("#tasksPanel #taskList")).toHaveCount(0);
  // The rules about goals and tasks are Focus's; waits and silence are core.
  await expect(ruleRow(page, "careGoalOn")).toBeHidden();
  await expect(ruleRow(page, "careTaskOn")).toBeHidden();
  await expect(ruleRow(page, "careWaitOn")).toBeVisible();
  await expect(ruleRow(page, "careSilenceOn")).toBeVisible();

  // Focus turned on brings the goal and tasks back, in the same tab.
  const on = await request.put(`/rooms/${room}/extensions/focus`, { headers: MASTER });
  expect(on.ok(), await on.text()).toBeTruthy();
  await expect(page.locator("#focusGoalSection")).toBeVisible();
  await expect(ruleRow(page, "careGoalOn")).toBeVisible();
  await page.close();
});

test("without Focus a participant sees the tab once a wait is declared", async ({ browser, request }) => {
  const room = "core-member";
  const made = await request.post("/rooms", { headers: MASTER, data: { name: room, extensions: ["notes"] } });
  expect([201, 409]).toContain(made.status());
  const bob = await memberPage(browser, request, "cc-bob", room);
  await expect.poll(() => visibleTabs(bob)).toEqual(["Chat", "Notes"]);

  // Someone declares that they wait for cc-bob, through a session of their
  // own (sessions are required here); the server names the waiter.
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const waiter = await (await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" },
  })).json();
  const as = { "x-session-token": waiter.session_token };
  const waited = await request.post(`/rooms/${room}/waits`, {
    headers: as, data: { by: "x", waiting_for: "cc-bob", reason: "the schema draft" },
  });
  expect(waited.ok(), await waited.text()).toBeTruthy();
  const { waiter: name } = await waited.json();
  await expect.poll(() => visibleTabs(bob)).toEqual(["Chat", "Notes", "Focus"]);
  await bob.locator("#tabTasks").click();
  await expect(bob.locator("#waitList")).toContainText(`${name} is waiting for cc-bob — the schema draft`);
  await expect(bob.locator("#focusGoalSection")).toBeHidden();

  // The wait cleared: nothing left to show this participant.
  const cleared = await request.delete(`/rooms/${room}/waits/${encodeURIComponent(name)}`, { headers: as, data: { by: "x" } });
  expect(cleared.ok(), await cleared.text()).toBeTruthy();
  await expect.poll(() => visibleTabs(bob)).toEqual(["Chat", "Notes"]);
  await bob.close();
});
