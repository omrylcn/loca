const { test, expect } = require("@playwright/test");
const MASTER = { "x-admin-token": "MASTER" };
// Screenshots go to this test's own output directory (test-results/).
const shot = name => test.info().outputPath(name);

async function signIn(browser, info, name, width) {
  const page = await browser.newPage({ viewport: { width, height: 900 } });
  await page.addInitScript(({ t, e, n }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token: t, expiresAt: e, name: n }));
  }, { t: info.session_token, e: info.expires_at, n: name });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate(() => switchTab("people"));
  return page;
}

// The Master Agent card in Building settings (owner's decisions,
// 2026-11-18): off by default; only the Master turns it on, names its
// floors and limits, appoints one (its key shown once) and revokes it. A
// Smaster never sees the card.
test("Master Agent: the Master's switch, floors and limits; a key shown once; revoke; a Smaster sees none of it", async ({ browser, request }) => {
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const master = await (await request.post("/sessions", { headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" } })).json();
  const { token } = await (await request.post("/smasters", { headers: MASTER, data: { name: "ma-deputy" } })).json();
  const deputy = await (await request.post("/sessions", { headers: { "x-admin-token": token }, data: { name: "ma-deputy", kind: "user" } })).json();
  const floor = await (await request.post("/groups", { headers: { "x-session-token": deputy.session_token }, data: { name: "project-floor" } })).json();

  for (const width of [1280, 390]) {
    const page = await signIn(browser, master, "Master", width);
    await expect.poll(() => page.evaluate(() => state.profile?.building_role)).toBe("master");
    await page.locator("#btabSettings").click();
    const card = page.locator("#maCard");
    await expect(card).toBeVisible();
    await expect(card.locator("#maH")).toHaveText("Master Agent");

    if (width === 1280) {
      // Off by default; appointing waits for the switch.
      await expect(card.locator("#maOn")).not.toBeChecked();
      await expect(card.locator("#maAppoint button")).toBeDisabled();

      await card.locator("#maOn").check();
      await expect(card.locator(".maswitch span")).toHaveText("On");
      await card.locator(`input[name=maFloor][value="${floor.id}"]`).check();
      await card.locator("input[name=live_agents]").fill("5");
      await card.locator("#maForm button[type=submit]").click();
      await expect(card.locator("#maNote")).toHaveText("Saved.");
      const setting = await (await request.get("/building/settings/master-agent", { headers: MASTER })).json();
      expect(setting.on).toBe(true);
      expect(setting.floors).toEqual([floor.id]);
      expect(setting.limits.live_agents).toBe(5);
      expect(setting.limits.locas_per_floor).toBe(20);
      await expect(card.locator(".malog summary")).toHaveText("Changes");

      // A limit the server refuses says why and changes nothing.
      await card.locator("input[name=live_agents]").evaluate(el => el.removeAttribute("min"));
      await card.locator("input[name=live_agents]").fill("0");
      await card.locator("#maForm button[type=submit]").click();
      await expect(card.locator("#maNote")).toContainText("from 1 to 1000");
      expect((await (await request.get("/building/settings/master-agent", { headers: MASTER })).json()).limits.live_agents).toBe(5);

      // Appoint: the key is shown once, with where it belongs.
      await card.locator("#maName").fill("legion-core");
      await card.locator("#maAppoint button[type=submit]").click();
      const secret = card.locator("#maSecret");
      await expect(secret).toHaveText(/^ma_/);
      await expect(card.locator("#maIssued")).toContainText("Shown once");
      await expect(card.locator("#maIssued")).toContainText("Never give it to a model");
      await expect(card.locator(".marow", { hasText: "legion-core" })).toContainText("automation");
      await page.screenshot({ path: shot(`master-agent-key-${width}.png`) });
      await card.locator("[data-ma-done]").click();
      await expect(card.locator("#maIssued")).toBeHidden();
      await expect(page.locator("body")).not.toContainText(/ma_[0-9a-f]{8}/);
    }

    await card.scrollIntoViewIfNeeded();
    await page.screenshot({ path: shot(`master-agent-${width}.png`) });
    await card.screenshot({ path: shot(`master-agent-card-${width}.png`) });
    expect(await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth)).toBe(false);
    await page.close();
  }

  // Revoke, asked in words under the row.
  const page = await signIn(browser, master, "Master", 1280);
  await page.locator("#btabSettings").click();
  const card = page.locator("#maCard");
  const row = card.locator(".marow", { hasText: "legion-core" });
  await row.locator("[data-ma-revoke]").click();
  await expect(card.locator(".maask")).toContainText("Revoke legion-core?");
  await page.screenshot({ path: shot("master-agent-revoke-1280.png") });
  await card.locator("[data-ma-yes]").click();
  await expect(card.locator(".marow.marevoked", { hasText: "legion-core" })).toContainText("revoked");
  const agents = await (await request.get("/master-agents", { headers: MASTER })).json();
  expect(agents.find(a => a.name === "legion-core").revoked_at).not.toBeNull();
  await page.close();

  // A Smaster never sees the card, and the server refuses it the setting.
  const smaster = await signIn(browser, deputy, "ma-deputy", 1280);
  await expect.poll(() => smaster.evaluate(() => state.profile?.building_role)).toBe("smaster");
  await smaster.locator("#btabSettings").click();
  await expect(smaster.locator("#buildingSettings .bbtag")).toHaveText("Read only");
  await expect(smaster.locator("#maCard")).toHaveCount(0);
  const status = await smaster.evaluate(async () => (await fetch("/building/settings/master-agent", { headers: adminHeaders({}) })).status);
  expect(status).toBe(403);
  await smaster.close();

  // Leave the Building as it was: the switch off.
  const off = await request.put("/building/settings/master-agent", { headers: MASTER, data: { on: false, floors: [] } });
  expect(off.ok()).toBe(true);
});
