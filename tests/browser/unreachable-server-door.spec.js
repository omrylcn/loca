const { test, expect } = require("@playwright/test");

// A desktop client opened with a server it cannot reach (the baked default,
// or a Host that moved) holds no identity yet. It used to show the room view
// with a sidebar form beside an empty "not connected" chat, and typing the
// right address changed nothing until the window was reloaded. It must show
// the sign-in page, say the server cannot be reached, and follow a corrected
// address without a reload.

const UNREACHABLE = "http://127.0.0.1:9";

test.beforeEach(async ({ page }) => {
  await page.addInitScript((server) => {
    localStorage.setItem("loca-gs-seen", "1");
    window.__LOCA_DEFAULT_SERVER__ = server;
  }, UNREACHABLE);
});

test("an unreachable server shows the sign-in page and says so", async ({ page }) => {
  await page.goto("/");
  await expect(page.locator("body")).toHaveClass(/locked/);
  await expect(page.locator("body")).not.toHaveClass(/auth-pending/);
  await expect(page.locator("#doorline")).toContainText(`cannot reach ${UNREACHABLE}`);
  await expect(page.locator("#server")).toBeVisible();
  await expect(page.locator("#server")).toHaveValue(UNREACHABLE);
});

test("typing the right address recovers without a reload", async ({ page, baseURL }) => {
  await page.goto("/");
  await expect(page.locator("#doorline")).toContainText("cannot reach");
  let reloaded = false;
  page.on("framenavigated", frame => { if (frame === page.mainFrame()) reloaded = true; });

  await page.locator("#server").fill(baseURL);
  await page.locator("#server").dispatchEvent("change");

  await expect(page.locator("#doorline")).toContainText("sign in with your key");
  await expect(page.locator("#doorline")).not.toContainText("cannot reach");
  await expect(page.locator("body")).toHaveClass(/locked/);
  await expect(page.locator("#masterEntryButton")).toBeVisible();
  expect(reloaded).toBe(false);

  // And the sign-in goes to the address typed.
  const pairing = await page.request.post(`${baseURL}/pairings?ttl_hours=1`, {
    headers: { "x-admin-token": "MASTER" },
  });
  expect(pairing.status()).toBe(201);
  const { pairing_code: code } = await pairing.json();
  await page.locator("#masterEntryButton").click();
  await page.locator("#pairingCode").fill(code);
  await page.evaluate(() => doConnect("iye"));
  await expect(page.locator("#connBox")).toBeHidden({ timeout: 10_000 });
  await expect(page.locator("#whoami")).toContainText("MASTER");
});

test("a wrong address typed after a right one says so again", async ({ page, baseURL }) => {
  await page.goto("/");
  await page.locator("#server").fill(baseURL);
  await page.locator("#server").dispatchEvent("change");
  await expect(page.locator("#doorline")).toContainText("sign in with your key");
  await page.locator("#server").fill(UNREACHABLE);
  await page.locator("#server").dispatchEvent("change");
  await expect(page.locator("#doorline")).toContainText(`cannot reach ${UNREACHABLE}`);
});
