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

test("the Building has People, Keys and Private messages tabs; only the Master sees the last two", async ({ browser, request }) => {
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const master = await (await request.post("/sessions", { headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" } })).json();
  const { token } = await (await request.post("/smasters", { headers: MASTER, data: { name: "tabs-deputy" } })).json();
  const deputy = await (await request.post("/sessions", { headers: { "x-admin-token": token }, data: { name: "tabs-deputy", kind: "user" } })).json();

  for (const width of [1280, 390]) {
    const page = await signIn(browser, master, "Master", width);
    await expect.poll(() => page.evaluate(() => state.profile?.building_role)).toBe("master");
    await expect(page.locator("#btabKeys")).toBeVisible();
    await expect(page.locator("#btabPrivate")).toBeVisible();
    await expect(page.locator("#peopleList")).toBeVisible();
    await expect(page.locator("#keyLedger")).toBeHidden();
    await page.screenshot({ path: shot(`btabs-people-${width}.png`) });

    await page.locator("#btabKeys").click();
    await expect(page.locator("#keyLedger")).toBeVisible();
    await expect(page.locator("#peopleList")).toBeHidden();
    await expect(page.locator("#keyLedger")).toContainText("Key generator");
    if (width === 1280) {
      await page.locator("#keyKind").selectOption("smaster");
      await page.locator("#keyForm button[type=submit]").click();
      await expect(page.locator("#keyIssued")).toContainText("copy it now");
    }
    await page.screenshot({ path: shot(`btabs-keys-${width}.png`) });

    await page.locator("#btabPrivate").click();
    await expect(page.locator("#birebirOverview")).toBeVisible();
    await expect(page.locator("#birebirOverview")).toContainText("Private messages");
    await page.screenshot({ path: shot(`btabs-private-${width}.png`) });
    expect(await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth)).toBe(false);
    await page.close();
  }

  // A Smaster sees the Building's People, never Keys or Private messages.
  const page = await signIn(browser, deputy, "tabs-deputy", 1280);
  await expect.poll(() => page.evaluate(() => state.profile?.building_role)).toBe("smaster");
  await expect(page.locator("#peopleList")).toBeVisible();
  await expect(page.locator("#btabKeys")).toBeHidden();
  await expect(page.locator("#btabPrivate")).toBeHidden();
  const status = await page.evaluate(async () => (await fetch("/keys", { headers: adminHeaders({}) })).status);
  expect(status).toBe(403);
  await page.close();
});
