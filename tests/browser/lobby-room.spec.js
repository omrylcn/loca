const { test, expect } = require("@playwright/test");

// The Lobby room (PRINCIPLES "Lobby"): everyone in the Building lands there
// first and may talk. A member with no seat lands in it and talks there with
// another member; the Master lands in Care, and reaches the Lobby room from
// the sidebar entry above the floors. It is no loca: not in the loca list,
// and it carries only Chat.

const MASTER = { "x-admin-token": "MASTER" };

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
});

async function memberPage(browser, request, name) {
  const admitted = await request.post("/members", { headers: MASTER, data: { name, kind: "user" } });
  expect(admitted.ok(), await admitted.text()).toBeTruthy();
  const { token } = await admitted.json();
  const page = await browser.newPage();
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await page.goto("/");
  await page.locator("#entryKind").selectOption("human");
  await page.locator("#name").fill(name);
  await page.locator("#roomToken").fill(token);
  await page.locator("#connectBtn").click();
  await expect.poll(() => page.evaluate(() => state.room)).toBe("lobby");
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  return page;
}

test("members land in the Lobby room and talk there; the Master walks in from Care", async ({ browser, request }) => {
  const ana = await memberPage(browser, request, "lobby-ana");
  const bo = await memberPage(browser, request, "lobby-bo");

  await expect(ana.locator("#curRoom")).toHaveText("Lobby");
  await expect(ana.locator("#lobbyRoomLink")).toHaveClass(/active/);
  expect(await ana.locator("#roomList .rname").allTextContents()).not.toContain("lobby");
  // Only Chat: the Lobby room carries no extensions.
  await expect(ana.locator("#tabNotes")).toBeHidden();
  await expect(ana.locator("#tabJournal")).toBeHidden();

  await ana.locator("#msg").fill("hello from the lobby");
  await ana.locator("#msg").press("Enter");
  await expect(bo.locator("#feed")).toContainText("hello from the lobby");

  // The Master lands in Care, and the Lobby room is one click away.
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const info = await (await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" },
  })).json();
  const master = await browser.newPage();
  await master.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "" }));
  }, { token: info.session_token, expiresAt: info.expires_at });
  await master.goto("/");
  await expect.poll(() => master.evaluate(() => state.authStatus)).toBe("authenticated");
  await master.evaluate(() => joinRoom("iye"));
  await master.locator("#lobbyRoomLink").click();
  await expect.poll(() => master.evaluate(() => state.room)).toBe("lobby");
  await expect(master.locator("#feed")).toContainText("hello from the lobby");
  await expect(master.locator("#lobbyRoomLink .lobbyroomcount")).toContainText("here");

  for (const page of [ana, bo, master]) await page.close();
});
