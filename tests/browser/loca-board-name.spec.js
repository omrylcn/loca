const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

// The `board` extension reads "Loca Board" in This Loca › Extensions and in
// the Manage list. The page keeps no list of names: both come from the
// server's catalog, under the unchanged id `board`.

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

test("This Loca and the Manage list call the board Loca Board", async ({ browser, request }) => {
  const room = "board-name";
  await provisionRooms(request, [room]);
  const lead = await request.post(`/rooms/${room}/lead`, { headers: MASTER, data: { lead: "bn-ada" } });
  expect(lead.ok(), await lead.text()).toBeTruthy();
  const on = await request.put(`/rooms/${room}/extensions/board`, { headers: MASTER });
  expect(on.ok(), await on.text()).toBeTruthy();

  const page = await masterPage(browser, request);
  await page.evaluate(room => joinRoom(room), room);
  await expect.poll(() => page.evaluate(() => extensionOn("board"))).toBe(true);
  await page.locator("#sideLocaTab").click();
  const block = page.locator("#locaSummary .locaext");
  await expect(block.locator(".extchip", { hasText: "Loca Board" })).toHaveCount(1);
  await expect(block).not.toContainText("Task board");

  await block.locator("#extManageOpen").click();
  const row = page.locator('#extManageList .extitem[data-ext="board"]');
  await expect(row.locator("b")).toHaveText("Loca Board");
  await expect(row.locator("[data-ext-remove]")).toHaveAttribute("title", "Remove Loca Board");
  await page.close();
});
