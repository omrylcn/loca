const { test, expect } = require("@playwright/test");
const MASTER = { "x-admin-token": "MASTER" };

// A key ledger that could not be read is unknown, not empty. "No keys here"
// would tell the Master that nothing was ever issued.

async function masterAtKeys(browser, request) {
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const info = await (await request.post("/sessions", { headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" } })).json();
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.addInitScript(({ t, e }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token: t, expiresAt: e, name: "Master" }));
  }, { t: info.session_token, e: info.expires_at });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate(() => switchTab("people"));
  await expect.poll(() => page.evaluate(() => state.profile?.building_role)).toBe("master");
  return { context, page };
}

test("a key ledger that fails to load says so and never shows an empty ledger", async ({ browser, request }) => {
  const { context, page } = await masterAtKeys(browser, request);
  let failing = true;
  await page.route("**/keys", route => {
    if (route.request().method() !== "GET" || !failing) return route.fallback();
    return route.fulfill({ status: 503, body: "the key ledger could not be read" });
  });
  await page.locator("#btabKeys").click();
  const ledger = page.locator("#keyLedger");
  await expect(ledger).toContainText("The key ledger could not be loaded.");
  await expect(ledger).not.toContainText("No keys here.");
  // The generator stays usable: the failure is about reading, not issuing.
  await expect(ledger.locator("#keyForm")).toBeVisible();

  // Once it can be read, the notice goes and the real ledger shows.
  failing = false;
  await page.evaluate(() => fetchKeyLedger());
  await expect(ledger).not.toContainText("The key ledger could not be loaded.");
  await expect(ledger.locator(".keyrow").first()).toBeVisible();
  await context.close();
});

test("keys already on screen stay, marked as possibly out of date, when a later read fails", async ({ browser, request }) => {
  const { context, page } = await masterAtKeys(browser, request);
  await page.locator("#btabKeys").click();
  const ledger = page.locator("#keyLedger");
  await expect(ledger.locator(".keyrow").first()).toBeVisible();
  const before = await ledger.locator(".keyrow").count();

  await page.route("**/keys", route =>
    route.request().method() === "GET" ? route.abort("connectionreset") : route.fallback());
  await page.evaluate(() => fetchKeyLedger());
  await expect(ledger).toContainText("The key ledger could not be loaded. What is shown may be out of date.");
  expect(await ledger.locator(".keyrow").count()).toBe(before);
  await expect(ledger).not.toContainText("No keys here.");
  await context.close();
});
