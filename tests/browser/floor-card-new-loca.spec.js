const { test, expect } = require("@playwright/test");

// A loca opened into a floor from the "+ new loca" panel is on that floor's
// card in Building › Floors at once, without a reload or a click on the tab.

const MASTER = { "x-admin-token": "MASTER" };

test("a loca opened into a floor shows on the floor's card without a reload", async ({ browser, request }) => {
  const tag = Date.now().toString(36);
  const floorName = `fc-floor-${tag}`, loca = `fc-loca-${tag}`;
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const signed = await (await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name: "fc-master", kind: "user" },
  })).json();
  const floor = await (await request.post("/groups", {
    headers: { "x-session-token": signed.session_token }, data: { name: floorName },
  })).json();
  expect(floor.id).toBeTruthy();

  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  await page.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "fc-master" }));
  }, { token: signed.session_token, expiresAt: signed.expires_at });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");

  // The Floors tab is open, and the floor has no loca yet.
  await page.evaluate(() => switchTab("people"));
  await page.locator("#btabGroups").click();
  const card = page.locator("#groupsPane .grpcard", { hasText: floorName });
  await expect(card).toBeVisible();
  await expect(card.locator(".grplocas")).not.toContainText(loca);

  // Open a loca on that floor from "+ new loca".
  await page.locator("#newRoomName").fill(loca);
  await page.locator("#newRoomBtn").click();
  await expect(page.locator("#extCreateGroupField")).toBeVisible();
  await page.locator("#extCreateGroup").selectOption(floor.id);
  await page.locator("#extCreateSubmit").click();
  await expect(page.locator("#extCreate")).toBeHidden();
  await expect.poll(() => page.evaluate(() => state.room)).toBe(loca);
  await expect.poll(() => page.evaluate(() => state.ws?.readyState)).toBe(1);
  await expect(page.locator("#feed")).toBeVisible();

  // Back in the Building, the Floors tab is still the one shown: its card
  // already lists the new loca.
  await page.evaluate(() => switchTab("people"));
  await expect(page.locator("#groupsPane")).toBeVisible();
  await expect(card.locator(".grplocas")).toContainText(loca);
  await page.close();
});
