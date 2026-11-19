const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

// Leaving the Building (PRINCIPLES "Authority and departure matrix": Leave)
// from the account menu: a quiet link under Sign out opens a question that
// takes the word "leave"; then the membership ends on the server, this
// browser signs out and says so, and the key no longer signs in.

const MASTER = { "x-admin-token": "MASTER" };
const ROOM = "leave-building";

test("a member leaves the building from the account menu, and the key stops working", async ({ page, request }) => {
  await provisionRooms(request, [ROOM]);
  const { token } = await (await request.post("/members", { headers: MASTER, data: { name: "leaving-lee", kind: "user" } })).json();
  const invited = await request.post(`/rooms/${ROOM}/invites`, { headers: MASTER, data: { name: "leaving-lee" } });
  expect(invited.ok(), await invited.text()).toBeTruthy();

  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await page.goto("/");
  await page.locator("#entryKind").selectOption("human");
  await page.locator("#name").fill("leaving-lee");
  await page.locator("#roomToken").fill(token);
  await page.locator("#connectBtn").click();
  await expect.poll(() => page.evaluate(() => state.room)).toBe(ROOM);

  await page.locator("#whoami .profilemenu > summary").click();
  await page.locator("#leaveBuildingBtn").click();
  await expect(page.locator("#leaveBuildingConfirm")).toBeVisible();
  // Nothing happens until the word is typed.
  await expect(page.locator("#leaveBuildingYes")).toBeDisabled();
  await page.locator("#leaveBuildingWord").fill("leave");
  await expect(page.locator("#leaveBuildingYes")).toBeEnabled();
  await page.locator("#leaveBuildingYes").click();

  await expect(page.locator("#doorline")).toContainText("you left the building");
  expect(await page.evaluate(() => state.authStatus)).toBe("unauthenticated");
  const members = await (await request.get("/members", { headers: MASTER })).json();
  expect(members.map(m => m.name)).not.toContain("leaving-lee");
  const whoami = await request.get("/whoami", { headers: { "x-room-token": token } });
  expect(whoami.status()).not.toBe(200);
});

test("the Master is not offered to leave their own building", async ({ browser, request }) => {
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const info = await (await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" },
  })).json();
  const page = await browser.newPage();
  await page.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "" }));
  }, { token: info.session_token, expiresAt: info.expires_at });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.locator("#whoami .profilemenu > summary").click();
  await expect(page.locator("#leaveBtn")).toBeVisible();
  await expect(page.locator("#leaveBuildingBtn")).toHaveCount(0);
  // And the server says no if asked anyway.
  const asked = await request.post("/membership/leave", {
    headers: { "x-session-token": info.session_token }, data: { confirm: "leave" },
  });
  expect(asked.status()).toBe(403);
  await page.close();
});
