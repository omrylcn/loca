const { test, expect } = require("@playwright/test");

// The Loca Board stands alone: its own tab, without Focus, holding the four
// columns, declaring and the review. The Focus tab holds no task list: a
// line with the board's counts while it is on, and a note while it is off
// and tasks are stored, with the operator's button to turn it on.

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

async function openLoca(request, room, extensions, lead) {
  const made = await request.post("/rooms", { headers: MASTER, data: { name: room, extensions } });
  expect([201, 409]).toContain(made.status());
  if (lead) {
    const named = await request.post(`/rooms/${room}/lead`, { headers: MASTER, data: { lead } });
    expect(named.ok(), await named.text()).toBeTruthy();
  }
}

async function boardOn(request, room) {
  const on = await request.put(`/rooms/${room}/extensions/board`, { headers: MASTER });
  expect(on.ok(), await on.text()).toBeTruthy();
}

// Task writes need a session here (REQUIRE_SESSIONS): the Master's own.
async function masterSession(request) {
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const info = await (await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" },
  })).json();
  return { "x-session-token": info.session_token };
}

async function declare(request, room, title, assigned_to) {
  const made = await request.post(`/rooms/${room}/tasks`, { headers: await masterSession(request), data: { title, by: "x", assigned_to } });
  expect(made.ok(), await made.text()).toBeTruthy();
  return made.json();
}

const visibleTabs = page => page.locator(".tabs [role=tab]:visible").allTextContents();
const column = (page, status) => page.locator(`#boardPanel .taskboard .boardcol[data-column="${status}"]`);
const card = (page, status, title) => column(page, status).locator(".task", { hasText: title });

test("the Loca Board tab works without Focus: declare, start, review, go", async ({ browser, request }) => {
  const room = "lb-alone";
  await openLoca(request, room, ["notes"]);
  const lead = await memberPage(browser, request, "lb-ada", room);
  const bob = await memberPage(browser, request, "lb-bob", room);
  // The Lead is named once its member exists (the title binds to it), then
  // the board turns on under the open pages.
  const named = await request.post(`/rooms/${room}/lead`, { headers: MASTER, data: { lead: "lb-ada" } });
  expect(named.ok(), await named.text()).toBeTruthy();
  await boardOn(request, room);
  for (const page of [lead, bob]) {
    await expect.poll(() => visibleTabs(page)).toEqual(["Chat", "Notes", "Loca Board"]);
    await page.locator("#tabBoard").click();
    await expect(page.locator("#boardPanel")).toBeVisible();
  }
  await lead.locator("#taskCreate summary").click();
  await lead.locator("#tkTitle").fill("write the upgrade note");
  await lead.locator("#tkAssign").fill("lb-bob");
  await lead.locator("#tkCreate").click();
  for (const page of [lead, bob]) {
    await expect(page.locator("#boardPanel .boardhead")).toHaveText(["To do · 1", "Doing · 0", "Review · 0", "Done · 0"]);
  }
  await card(bob, "open", "write the upgrade note").getByRole("button", { name: "Start" }).click();
  await card(bob, "taken", "write the upgrade note").getByRole("button", { name: "Ask for review" }).click();
  await expect(card(lead, "review", "write the upgrade note")).toHaveCount(1);
  await card(lead, "review", "write the upgrade note").getByRole("button", { name: "Approve", exact: true }).click();
  for (const page of [lead, bob]) await expect(card(page, "done", "write the upgrade note")).toHaveCount(1);
  for (const page of [lead, bob]) await page.close();
});

test("Focus shows the board's counts and opens the Loca Board; it lists no task", async ({ browser, request }) => {
  const room = "lb-line";
  await openLoca(request, room, ["notes", "focus"], "lb-cem");
  await boardOn(request, room);
  const first = await declare(request, room, "check the rollback path");
  await declare(request, room, "draft the release note");
  const bob = await memberPage(browser, request, "lb-dan", room);
  await expect.poll(() => visibleTabs(bob)).toEqual(["Chat", "Notes", "Focus", "Loca Board"]);
  await bob.locator("#tabTasks").click();
  await expect(bob.locator("#tasksPanel .task")).toHaveCount(0);
  const line = bob.locator("#focusBoardLine");
  await expect(line).toContainText("Loca Board: 2 to do");
  // Live: a task moving on the board changes the line.
  const moved = await request.patch(`/rooms/${room}/tasks/${first.id}`, { headers: await masterSession(request), data: { status: "taken", by: "x", assigned_to: "lb-dan" } });
  expect(moved.ok(), await moved.text()).toBeTruthy();
  await expect(line).toContainText("Loca Board: 1 to do · 1 doing");
  await line.getByRole("button", { name: "Open Loca Board" }).click();
  await expect.poll(() => bob.evaluate(() => state.tab)).toBe("board");
  await expect(bob.locator("#boardPanel")).toBeVisible();
  await bob.close();
});

test("with the board off, Focus says where the stored tasks went and the operator turns it on", async ({ browser, request }) => {
  const room = "lb-note";
  await openLoca(request, room, ["notes", "focus"], "lb-eda");
  await boardOn(request, room);
  await declare(request, room, "kept while off");
  const off = await request.delete(`/rooms/${room}/extensions/board?open_tasks=keep`, { headers: MASTER });
  expect(off.ok(), await off.text()).toBeTruthy();

  const page = await masterPage(browser, request);
  await page.evaluate(room => joinRoom(room), room);
  await expect.poll(() => visibleTabs(page)).toEqual(["Chat", "Notes", "Focus"]);
  await page.locator("#tabTasks").click();
  const line = page.locator("#focusBoardLine");
  await expect(line).toContainText("Tasks now live in the Loca Board tab, which needs a Lead.");
  await line.getByRole("button", { name: "Turn on Loca Board" }).click();
  await expect.poll(() => visibleTabs(page)).toEqual(["Chat", "Notes", "Focus", "Loca Board"]);
  await page.locator("#tabBoard").click();
  await expect(column(page, "open").locator(".task", { hasText: "kept while off" })).toHaveCount(1);
  await page.close();
});

test("without a Lead the note says one is needed and offers no button", async ({ browser, request }) => {
  const room = "lb-nolead";
  await openLoca(request, room, ["focus"], "lb-fay");
  await boardOn(request, room);
  await declare(request, room, "waiting for a Lead");
  expect((await request.delete(`/rooms/${room}/extensions/board?open_tasks=keep`, { headers: MASTER })).ok()).toBeTruthy();
  expect((await request.post(`/rooms/${room}/lead`, { headers: MASTER, data: { lead: null } })).ok()).toBeTruthy();

  const page = await masterPage(browser, request);
  await page.evaluate(room => joinRoom(room), room);
  await page.locator("#tabTasks").click();
  const line = page.locator("#focusBoardLine");
  await expect(line).toContainText("Tasks now live in the Loca Board tab, which needs a Lead.");
  await expect(line).toContainText("Name a Lead to turn it on.");
  await expect(line.getByRole("button", { name: "Turn on Loca Board" })).toHaveCount(0);
  await page.close();
});
