const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

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

async function visibleTabs(page) {
  return page.locator(".tabs [role=tab]:visible").allTextContents();
}

test("the + screen ticks the defaults and creates the loca with the chosen set", async ({ browser, request }) => {
  await provisionRooms(request, ["ext-home"]);
  const page = await masterPage(browser, request);
  await page.evaluate(() => joinRoom("ext-home"));
  await expect.poll(() => page.evaluate(() => state.ws?.readyState)).toBe(1);
  // An existing loca keeps every tab and the live toggle.
  await expect.poll(() => visibleTabs(page)).toEqual(["Chat", "Notes", "Focus", "Journal", "Loca Wiki"]);
  await expect(page.locator("#liveToggle")).toBeVisible();

  await page.locator("#newRoomName").fill("ext-new");
  await page.locator("#newRoomBtn").click();
  const dialog = page.locator("#extCreate");
  await expect(dialog).toBeVisible();
  await expect(page.locator("#extCreateName")).toHaveValue("ext-new");
  const defaults = page.locator("#extCreateDefault input");
  await expect(defaults).toHaveCount(4); // Chat (fixed) + notes, journal, wiki
  expect(await defaults.evaluateAll(rows => rows.map(r => [r.disabled ? "chat" : r.value, r.checked, r.disabled])))
    .toEqual([["chat", true, true], ["notes", true, false], ["journal", true, false], ["wiki", true, false]]);
  expect(await page.locator("#extCreateAdd input").evaluateAll(rows => rows.map(r => [r.value, r.checked])))
    .toEqual([["focus", false], ["live", false]]);
  await expect(page.locator("#extCreateTabs")).toHaveText("Tabs in this loca: Chat · Notes · Journal · Loca Wiki");

  // Add Focus, drop the Wiki: the tabs line follows.
  await page.locator("#extCreateAdd .extrow", { hasText: "Focus" }).click();
  await page.locator("#extCreateDefault .extrow", { hasText: "Loca Wiki" }).click();
  await expect(page.locator("#extCreateTabs")).toHaveText("Tabs in this loca: Chat · Notes · Focus · Journal");

  // Cancel leaves nothing behind; reopening starts from the defaults again.
  await page.locator("#extCreateCancel").click();
  await expect(dialog).toBeHidden();
  expect((await request.get("/rooms/ext-new/extensions", { headers: MASTER })).status()).not.toBe(200);
  await page.locator("#newRoomBtn").click();
  await expect(page.locator("#extCreateTabs")).toHaveText("Tabs in this loca: Chat · Notes · Journal · Loca Wiki");
  await page.locator("#extCreateAdd .extrow", { hasText: "Focus" }).click();
  await page.locator("#extCreateDefault .extrow", { hasText: "Loca Wiki" }).click();
  await page.locator("#extCreateSubmit").click();
  await expect(dialog).toBeHidden();
  await expect.poll(() => page.evaluate(() => state.room)).toBe("ext-new");

  const server = await (await request.get("/rooms/ext-new/extensions", { headers: MASTER })).json();
  expect(server.enabled).toEqual(["notes", "focus", "journal"]);
  await expect.poll(() => visibleTabs(page)).toEqual(["Chat", "Notes", "Focus", "Journal"]);
  await expect(page.locator("#liveToggle")).toBeHidden();
  // The server refuses the Wiki, not just the tab.
  expect((await request.get("/rooms/ext-new/wiki", { headers: MASTER })).status()).toBe(403);
  await page.close();
});

test("This Loca › Extensions shows chips and Manage changes the tabs; a participant only reads", async ({ browser, request }) => {
  const room = "ext-manage";
  const created = await request.post("/rooms", { headers: MASTER, data: { name: room, extensions: ["notes", "journal"] } });
  expect([201, 409]).toContain(created.status());
  const page = await masterPage(browser, request);
  await page.evaluate((room) => joinRoom(room), room);
  await expect.poll(() => page.evaluate(() => state.ws?.readyState)).toBe(1);
  await expect.poll(() => visibleTabs(page)).toEqual(["Chat", "Notes", "Journal"]);

  await page.locator("#sideLocaTab").click();
  const block = page.locator("#locaSummary .locaext");
  await expect(block.locator(".extchip")).toHaveText(["Notes", "Journal"]);
  await expect(block.locator("#extManageOpen")).toHaveText("Manage");
  await block.locator("#extManageOpen").click();
  const manage = page.locator("#extManage");
  await expect(manage).toBeVisible();
  const item = (name) => manage.locator(".extitem", { has: page.locator("b", { hasText: new RegExp(`^${name}$`) }) });
  await expect(item("Notes").locator("button")).toHaveText("Added");
  await expect(item("Focus").locator("button")).toHaveText("Add");

  await item("Loca Wiki").locator("button").click();
  await expect(item("Loca Wiki").locator("button")).toHaveText("Added");
  await item("Live").locator("button").click();
  await expect(item("Live").locator("button")).toHaveText("Added");
  await item("Notes").locator("button").click();
  await expect(item("Notes").locator("button")).toHaveText("Add");
  await expect.poll(() => visibleTabs(page)).toEqual(["Chat", "Journal", "Loca Wiki"]);
  await expect(block.locator(".extchip")).toHaveText(["Journal", "Loca Wiki", "Live"]);
  await page.keyboard.press("Escape");
  await expect(manage).toBeHidden();
  await expect(page.locator("#liveToggle")).toBeVisible();

  // The Journal records who changed what.
  const journal = await (await request.get(`/rooms/${room}/journal`, { headers: MASTER })).json();
  const lines = journal.map(entry => entry.text);
  expect(lines).toEqual(expect.arrayContaining([
    "extensions: added Loca Wiki", "extensions: added Live", "extensions: removed Notes",
  ]));

  // A participant sees the same chips and a read-only list.
  const name = "ext-reader";
  expect((await request.post("/members", { headers: MASTER, data: { name, kind: "user" } })).ok()).toBeTruthy();
  const invite = await (await request.post(`/rooms/${room}/invites`, { headers: MASTER, data: { name } })).json();
  const session = await (await request.post("/sessions", {
    headers: { "x-room-token": invite.token }, data: { name, kind: "user", loca: room },
  })).json();
  expect((await request.put(`/rooms/${room}/extensions/notes`, {
    headers: { "x-session-token": session.session_token },
  })).status()).toBe(403);
  const reader = await browser.newPage();
  await reader.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await reader.goto("/");
  await reader.locator("#name").fill(name);
  await reader.locator("#roomToken").fill(invite.token);
  await reader.evaluate((room) => window.doConnect(room), room);
  await expect(reader.locator("#curRoom")).toHaveText(room);
  await expect.poll(() => reader.evaluate(() => state.ws?.readyState)).toBe(1);
  await expect.poll(() => visibleTabs(reader)).toEqual(["Chat", "Journal", "Loca Wiki"]);
  await reader.locator("#sideLocaTab").click();
  await expect(reader.locator("#locaSummary .extchip")).toHaveText(["Journal", "Loca Wiki", "Live"]);
  await expect(reader.locator("#extManageOpen")).toHaveText("View");
  await reader.locator("#extManageOpen").click();
  await expect(reader.locator("#extManage")).toBeVisible();
  await expect(reader.locator("#extManageList button")).toHaveCount(0);
  await expect(reader.locator("#extManageNote")).toContainText("Only this loca's operator");
  await reader.locator("#extManageClose").click();

  // A change by the Master reaches the reader live.
  expect((await request.put(`/rooms/${room}/extensions/focus`, { headers: MASTER })).ok()).toBeTruthy();
  await expect.poll(() => visibleTabs(reader)).toEqual(["Chat", "Focus", "Journal", "Loca Wiki"]);
  await reader.close();
  await page.close();
});

test("settings that could not be read turn no extension on: no tabs, no extension requests", async ({ browser, request }) => {
  const page = await masterPage(browser, request);
  const cases = [
    ["ext-unknown-503", route => route.fulfill({ status: 503, body: "settings unavailable" })],
    ["ext-unknown-offline", route => route.abort()],
  ];
  for (const [room, fail] of cases) {
    const created = await request.post("/rooms", { headers: MASTER, data: { name: room, extensions: ["notes"] } });
    expect([201, 409]).toContain(created.status());
    const asked = [];
    const record = (req) => {
      const path = new URL(req.url()).pathname;
      const match = path.match(new RegExp(`^/rooms/${room}/(notes|tasks|goals|journal|wiki)(/|$)`));
      if (match) asked.push(match[1]);
    };
    page.on("request", record);
    const settingsUrl = `**/rooms/${room}/settings`;
    await page.route(settingsUrl, fail);

    // The settings answer, then the care reads that follow it in the same
    // turn as any extension read: once those are back, nothing else is due.
    const settled = page.waitForEvent("requestfinished", r => r.url().endsWith(`/rooms/${room}/settings`)).catch(() => null);
    const failed = page.waitForEvent("requestfailed", r => r.url().endsWith(`/rooms/${room}/settings`)).catch(() => null);
    const care = page.waitForResponse(r => r.url().endsWith(`/rooms/${room}/attentions`));
    await page.evaluate((room) => joinRoom(room), room);
    await Promise.race([settled, failed]);
    await care;
    await expect.poll(() => page.evaluate(() => state.ws?.readyState)).toBe(1);

    // Soft, so a regression reports all three: the tabs, the requests, the state.
    expect.soft(await visibleTabs(page)).toEqual(["Chat"]);
    expect.soft(asked).toEqual([]);
    expect.soft(await page.evaluate(() => [isLocaContextReady("settings"), currentExtensions()])).toEqual([false, null]);
    await expect(page.locator("#liveToggle")).toBeHidden();
    await page.locator("#sideLocaTab").click();
    await expect(page.locator("#locaSummary .extchip")).toHaveCount(0);
    await expect(page.locator("#locaSummary .extnone")).toHaveText("Loading…");

    // The request above has been answered, so no handler is left waiting.
    await page.unroute(settingsUrl);
    // A later load that succeeds says what is on here, and only that is asked.
    await page.evaluate(() => fetchSettings());
    await expect.poll(() => visibleTabs(page)).toEqual(["Chat", "Notes"]);
    expect(await page.evaluate(() => currentExtensions())).toEqual(["notes"]);
    await expect(page.locator("#locaSummary .extchip")).toHaveText(["Notes"]);
    expect(asked.filter(name => name !== "notes")).toEqual([]);
    page.off("request", record);
  }
  await page.close();
});
