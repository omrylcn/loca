const { test, expect } = require("@playwright/test");
const MASTER = { "x-admin-token": "MASTER" };
// Screenshots go to this test's own output directory (test-results/), never
// to a path on somebody's machine.
const shot = name => test.info().outputPath(name);

async function signIn(browser, info, name, width) {
  const page = await browser.newPage({ viewport: { width, height: 860 } });
  await page.addInitScript(({ t, e, n }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token: t, expiresAt: e, name: n }));
  }, { t: info.session_token, e: info.expires_at, n: name });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate(() => switchTab("people"));
  return page;
}

// BuildingSettings.dc.html: the Building's Settings tab. What comes from the
// server's environment is shown read-only under its variable's name; the
// Master changes only the Building's name; a Smaster reads the same page.
test("Building settings: the Master names the Building, the rest is the environment's, a Smaster only reads", async ({ browser, request }) => {
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const master = await (await request.post("/sessions", { headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" } })).json();
  const { token } = await (await request.post("/smasters", { headers: MASTER, data: { name: "settings-deputy" } })).json();
  const deputy = await (await request.post("/sessions", { headers: { "x-admin-token": token }, data: { name: "settings-deputy", kind: "user" } })).json();

  for (const width of [1280, 390]) {
    const page = await signIn(browser, master, "Master", width);
    await expect.poll(() => page.evaluate(() => state.profile?.building_role)).toBe("master");
    await expect(page.locator("#btabSettings")).toBeVisible();
    await page.locator("#btabSettings").click();
    const pane = page.locator("#buildingSettings");
    await expect(pane).toBeVisible();
    await expect(page.locator("#peopleList")).toBeHidden();
    await expect(pane.locator(".bbtitle")).toHaveText("Building settings");
    await expect(pane.locator(".bbtag")).toHaveText("Master only");
    // Read-only values carry their variable's name; the suite's server
    // sets NOTIFICATION_RATE_LIMIT=1000 and a database.
    for (const variable of ["PUBLIC_SERVER_URL", "REQUIRE_INVITE", "REQUIRE_SESSIONS", "LOCA_CAPACITY", "LOCA_CAPACITY_MAX", "NOTIFICATION_RATE_LIMIT", "LOCA_CARETAKERS", "DB_PATH"]) {
      await expect(pane.locator("code.bsvar", { hasText: new RegExp(`^${variable}$`) })).toHaveCount(1);
    }
    await expect(pane).toContainText("1000 every 10 minutes");
    await expect(pane).toContainText("kept on disk");
    // No whole-Building download: the backup doc instead.
    await expect(pane.locator("a.bslink")).toHaveAttribute("href", /docs\/backup-restore\.md$/);
    await expect(pane.getByRole("link", { name: /download/i })).toHaveCount(0);
    // Seats are not editable here: the only field is the name.
    await expect(pane.locator(".bscard:not(#maCard) input")).toHaveCount(1);

    if (width === 1280) {
      await pane.locator("#bsName").fill("  Omnelo   Tower ");
      await pane.locator("#bsNameForm button[type=submit]").click();
      await expect(pane.locator("#bsNameNote")).toHaveText("Saved.");
      await expect(pane.locator("#bsName")).toHaveValue("Omnelo Tower");
      const view = await (await request.get("/building/settings", { headers: MASTER })).json();
      expect(view.name).toBe("Omnelo Tower");

      // A refused name says why and keeps what was saved (the field's own
      // maxlength is lifted to reach the server's rule).
      await page.evaluate(() => $("bsName").removeAttribute("maxlength"));
      await pane.locator("#bsName").fill("x".repeat(65));
      await pane.locator("#bsNameForm button[type=submit]").click();
      await expect(pane.locator("#bsNameNote")).toContainText("at most 64");
      expect((await (await request.get("/building/settings", { headers: MASTER })).json()).name).toBe("Omnelo Tower");
    }
    await page.screenshot({ path: shot(`building-settings-master-${width}.png`) });
    expect(await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth)).toBe(false);
    await page.close();
  }

  // A Smaster sees the same page with nothing to change; the server refuses
  // the change too.
  const page = await signIn(browser, deputy, "settings-deputy", 1280);
  await expect.poll(() => page.evaluate(() => state.profile?.building_role)).toBe("smaster");
  await page.locator("#btabSettings").click();
  const pane = page.locator("#buildingSettings");
  await expect(pane.locator(".bbtag")).toHaveText("Read only");
  await expect(pane).toContainText("Omnelo Tower");
  await expect(pane.locator("input")).toHaveCount(0);
  const status = await page.evaluate(async () => (await fetch("/building/settings", {
    method: "PUT", headers: adminHeaders({ "content-type": "application/json" }), body: JSON.stringify({ name: "Deputy's" }),
  })).status);
  expect(status).toBe(403);
  await page.screenshot({ path: shot("building-settings-smaster-1280.png") });
  await page.close();

  // Clear the name so later specs meet the Building as it was.
  const cleared = await request.put("/building/settings", { headers: MASTER, data: { name: null } });
  expect(cleared.ok()).toBe(true);
});
