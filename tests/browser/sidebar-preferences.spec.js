const { test, expect } = require("@playwright/test");

test("unresolved or failed identity never sees or mutates another principal's loca preferences", async ({ page, request }) => {
  const paired = await request.post("/pairings?ttl_hours=1", {
    headers: { "x-admin-token": "MASTER" },
  });
  const { pairing_code: pairingCode } = await paired.json();
  await page.goto("/");
  await page.locator("#pairingCode").fill(pairingCode);
  await page.evaluate(() => window.doConnect("preference-security"));
  await expect(page.locator("#whoami")).toContainText("MASTER");
  await page.locator("#sideBuildingTab").click();
  await page.evaluate(() => {
    state.rooms = [{ room: "private-a" }, { room: "private-b" }];
    renderRooms();
  });
  const item = room => page.locator("#roomList .roomitem").filter({
    has: page.locator(".rname", { hasText: room }),
  });
  const choose = async (room, action) => {
    await item(room).locator(".roompreftrigger").click();
    await item(room).locator(`[data-room-preference="${action}"]`).click();
  };
  await choose("private-b", "pin");
  await choose("private-a", "hide");
  const principalA = await page.evaluate(() => state.principalId);
  const keyA = `loca-room-preferences:${new URL(page.url()).origin}:${principalA}`;
  const storedA = await page.evaluate(key => localStorage.getItem(key), keyA);
  expect(JSON.parse(storedA).pinned).toContain("private-b");
  expect(JSON.parse(storedA).hidden).toContain("private-a");

  let releaseB;
  const bGate = new Promise(resolve => { releaseB = resolve; });
  await page.route("**/profile?room=preference-security", async route => {
    await bGate;
    await route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({
        principal: { id: "principal-b", display_name: "second", kind: "user" },
        building_role: "member",
        loca: { room: "preference-security", roles: ["participant"] },
        session: { bounded: true, expires_at: Date.now() + 60_000 },
      }),
    });
  });
  await page.evaluate(() => {
    resetRoomPreferenceIdentity();
    state.profile = null;
    state.rooms = [{ room: "private-a" }, { room: "private-b" }];
    window.__delayedPrincipalB = fetchProfile();
    renderRooms();
  });
  await expect(item("private-a")).toHaveCount(1);
  await expect(item("private-b")).toHaveCount(1);
  await expect(page.locator("#hiddenLocas")).toBeHidden();
  await expect(page.locator("#roomList [data-room-preference]").first()).toBeDisabled();
  await page.evaluate(() => updateRoomPreference("hide", "private-b"));
  expect(await page.evaluate(key => localStorage.getItem(key), keyA)).toBe(storedA);

  releaseB();
  await page.evaluate(() => window.__delayedPrincipalB);
  await expect(page.locator("#roomList [data-room-preference]").first()).toBeEnabled();
  await expect(item("private-a")).toHaveCount(1);
  await expect(item("private-b")).toHaveCount(1);
  await choose("private-a", "pin");
  expect(await page.evaluate(key => localStorage.getItem(key), keyA)).toBe(storedA);
  const keyB = `loca-room-preferences:${new URL(page.url()).origin}:principal-b`;
  expect(JSON.parse(await page.evaluate(key => localStorage.getItem(key), keyB)).pinned).toContain("private-a");

  await page.unroute("**/profile?room=preference-security");
  await page.route("**/profile?room=preference-security", route => route.fulfill({ status: 503 }));
  await page.evaluate(async () => {
    resetRoomPreferenceIdentity();
    state.profile = null;
    await fetchProfile();
    renderRooms();
  });
  await expect(item("private-a")).toHaveCount(1);
  await expect(item("private-b")).toHaveCount(1);
  await expect(page.locator("#hiddenLocas")).toBeHidden();
  await expect(page.locator("#roomList [data-room-preference]").first()).toBeDisabled();
  expect(await page.evaluate(key => localStorage.getItem(key), keyA)).toBe(storedA);
});
