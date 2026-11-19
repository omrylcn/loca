const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

// Home and People belong to no loca, so the sidebar's "This Loca" tab steps
// aside there and Your Locas stays; back in a loca the tabs return.

const MASTER = { "x-admin-token": "MASTER" };
const ROOM = "home-tabs";

test("Home and People hide the sidebar's This Loca tab; a loca shows it again", async ({ browser, request }) => {
  await provisionRooms(request, [ROOM]);
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const info = await (await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" },
  })).json();
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  await page.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "" }));
  }, { token: info.session_token, expiresAt: info.expires_at });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate(room => joinRoom(room), ROOM);
  await expect(page.locator(".sideviewtabs")).toBeVisible();
  await page.locator("#sideLocaTab").click();
  await expect(page.locator("#sideLocaView")).toBeVisible();

  for (const view of ["home", "people"]) {
    await page.evaluate(v => switchTab(v), view);
    await expect(page.locator(".sideviewtabs"), view).toBeHidden();
    await expect(page.locator("#sideBuildingView"), view).toBeVisible();
  }

  await page.evaluate(() => switchTab("chat"));
  await expect(page.locator(".sideviewtabs")).toBeVisible();
  await page.close();
});
