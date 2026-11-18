const { test, expect } = require("@playwright/test");

// Turning the Loca Board off from Manage while tasks are not done: the page
// asks with the counts and offers keep (the default), done or remove. With
// nothing unfinished it turns off at once.

const MASTER = { "x-admin-token": "MASTER" };

async function masterSession(request) {
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  return (await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" },
  })).json();
}

async function masterPage(browser, request) {
  const info = await masterSession(request);
  const page = await browser.newPage();
  await page.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "Master" }));
  }, { token: info.session_token, expiresAt: info.expires_at });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  return page;
}

// A loca with Focus and the board on, and one task per given status.
async function boardWith(request, room, statuses) {
  const made = await request.post("/rooms", { headers: MASTER, data: { name: room, extensions: ["notes", "focus"] } });
  expect([201, 409]).toContain(made.status());
  expect((await request.post(`/rooms/${room}/lead`, { headers: MASTER, data: { lead: "bo-lead" } })).ok()).toBeTruthy();
  expect((await request.put(`/rooms/${room}/extensions/board`, { headers: MASTER })).ok()).toBeTruthy();
  const s = { "x-session-token": (await masterSession(request)).session_token };
  for (const [i, status] of statuses.entries()) {
    const task = await (await request.post(`/rooms/${room}/tasks`, { headers: s, data: { title: `task ${i} ${status}`, by: "x" } })).json();
    if (status !== "open") {
      const moved = await request.patch(`/rooms/${room}/tasks/${task.id}`, { headers: s, data: { status, by: "x" } });
      expect(moved.ok(), await moved.text()).toBeTruthy();
    }
  }
}

async function openManage(page, room) {
  await page.evaluate(room => joinRoom(room), room);
  await expect.poll(() => page.evaluate(() => extensionOn("board"))).toBe(true);
  await page.locator("#sideLocaTab").click();
  await page.locator("#locaSummary #extManageOpen").click();
  await expect(page.locator("#extManage")).toBeVisible();
}

const statusesOf = async (request, room) => {
  const on = await request.put(`/rooms/${room}/extensions/board`, { headers: MASTER });
  expect(on.ok(), await on.text()).toBeTruthy();
  return (await (await request.get(`/rooms/${room}/tasks`, { headers: MASTER })).json()).map(t => t.status);
};

test("removing the board with unfinished tasks asks first; keep is the default", async ({ browser, request }) => {
  const room = "bo-keep";
  await boardWith(request, room, ["open", "taken", "done"]);
  const page = await masterPage(browser, request);
  await openManage(page, room);
  await page.locator('#extManageList [data-ext-remove="board"]').click();
  const ask = page.locator("#boardOffAsk");
  await expect(ask).toBeVisible();
  await expect(ask).toContainText("2 tasks are not done");
  await expect(ask).toContainText("1 to do · 1 doing · 0 in review");
  // Nothing changed yet.
  expect(await page.evaluate(() => extensionOn("board"))).toBe(true);
  await expect(ask.getByRole("button", { name: "Keep them" })).toBeFocused();
  await ask.getByRole("button", { name: "Keep them" }).click();
  await expect.poll(() => page.evaluate(() => extensionOn("board"))).toBe(false);
  await expect(ask).toBeHidden();
  expect(await statusesOf(request, room)).toEqual(["open", "taken", "done"]);
  await page.close();
});

test("mark them done, or remove them, closes the unfinished tasks before the board goes", async ({ browser, request }) => {
  for (const [choice, button, closed] of [["done", "Mark them done", "done"], ["remove", "Remove them", "cancelled"]]) {
    const room = `bo-${choice}`;
    await boardWith(request, room, ["open", "taken"]);
    const page = await masterPage(browser, request);
    await openManage(page, room);
    await page.locator('#extManageList [data-ext-remove="board"]').click();
    await page.locator("#boardOffAsk").getByRole("button", { name: button }).click();
    await expect.poll(() => page.evaluate(() => extensionOn("board"))).toBe(false);
    expect(await statusesOf(request, room)).toEqual([closed, closed]);
    await page.close();
  }
});

test("cancel leaves the board on; with only finished tasks there is no question", async ({ browser, request }) => {
  await boardWith(request, "bo-cancel", ["open"]);
  const page = await masterPage(browser, request);
  await openManage(page, "bo-cancel");
  await page.locator('#extManageList [data-ext-remove="board"]').click();
  await page.locator("#boardOffAsk").getByRole("button", { name: "Cancel" }).click();
  await expect(page.locator("#boardOffAsk")).toBeHidden();
  expect(await page.evaluate(() => extensionOn("board"))).toBe(true);
  await page.locator("#extManageClose").click();

  await boardWith(request, "bo-finished", ["done"]);
  await openManage(page, "bo-finished");
  await page.locator('#extManageList [data-ext-remove="board"]').click();
  await expect.poll(() => page.evaluate(() => extensionOn("board"))).toBe(false);
  await expect(page.locator("#boardOffAsk")).toBeHidden();
  await page.close();
});
