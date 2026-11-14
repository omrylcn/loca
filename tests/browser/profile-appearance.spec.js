const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

const ROOM = "appearance-room";
const DEFAULTS = { theme: "dark", accent: "teal", density: "comfortable", font_size: "normal" };

async function resetMasterAppearance(request) {
  const reset = await request.put("/profile/preferences", {
    headers: { "x-admin-token": "MASTER" }, data: DEFAULTS,
  });
  expect(reset.ok()).toBeTruthy();
}

async function signInAsMaster(page, request) {
  const paired = await request.post("/pairings?ttl_hours=1", {
    headers: { "x-admin-token": "MASTER" },
  });
  const { pairing_code: pairingCode } = await paired.json();
  await page.goto("/");
  await page.locator("#masterEntryButton").click();
  await page.locator("#pairingCode").fill(pairingCode);
  await page.evaluate(room => window.doConnect(room), ROOM);
  await expect(page.locator("#whoami")).toContainText("MASTER");
}

async function newPage(browser) {
  const context = await browser.newContext();
  await context.addInitScript(() => {
    try { localStorage.setItem("loca-gs-seen", "1"); } catch (e) {}
  });
  return { context, page: await context.newPage() };
}

const html = page => page.locator("html");
const saved = page => page.waitForResponse(response =>
  response.url().endsWith("/profile/preferences") && response.request().method() === "PUT" && response.ok());

test.beforeEach(async ({ request }) => {
  await provisionRooms(request, [ROOM]);
  await resetMasterAppearance(request);
});

test.afterEach(async ({ request }) => {
  await resetMasterAppearance(request);
});

test("appearance applies instantly, survives reload and follows the principal to another browser", async ({ browser, request }) => {
  const first = await newPage(browser);
  await signInAsMaster(first.page, request);
  await expect(html(first.page)).toHaveAttribute("data-theme", "dark");
  await expect(html(first.page)).toHaveAttribute("data-accent", "teal");

  await first.page.locator("#whoami .profilemenu > summary").click();
  const appearance = first.page.locator("#profileAppearance");
  await expect(appearance).toBeVisible();
  const background = () => first.page.evaluate(() => getComputedStyle(document.body).backgroundColor);
  const darkBackground = await background();

  let write = saved(first.page);
  await appearance.getByRole("radio", { name: "Light" }).check();
  await expect(html(first.page)).toHaveAttribute("data-theme", "light");
  expect(await background()).not.toBe(darkBackground);
  await write;
  write = saved(first.page);
  await appearance.getByRole("radio", { name: "Violet" }).check();
  await expect(html(first.page)).toHaveAttribute("data-accent", "violet");
  await write;
  write = saved(first.page);
  await appearance.getByRole("radio", { name: "Compact" }).check();
  await expect(html(first.page)).toHaveAttribute("data-density", "compact");
  await write;

  const stored = await request.get("/profile/preferences", { headers: { "x-admin-token": "MASTER" } });
  expect(await stored.json()).toEqual({ theme: "light", accent: "violet", density: "compact", font_size: "normal" });

  // Reload: the cached copy is on <html> before any profile request resolves.
  let releaseProfile;
  const profileGate = new Promise(resolve => { releaseProfile = resolve; });
  await first.page.route("**/profile/preferences", async route => {
    await profileGate;
    await route.continue();
  });
  await first.page.reload();
  await expect(html(first.page)).toHaveAttribute("data-theme", "light");
  await expect(html(first.page)).toHaveAttribute("data-accent", "violet");
  const profileResponse = first.page.waitForResponse(response =>
    response.url().endsWith("/profile/preferences") && response.request().method() === "GET");
  releaseProfile();
  await profileResponse;
  await first.page.unroute("**/profile/preferences");
  await expect(first.page.locator("#whoami")).toContainText("MASTER");
  await expect(html(first.page)).toHaveAttribute("data-theme", "light");
  await first.page.locator("#whoami .profilemenu > summary").click();
  await expect(first.page.locator("#profileAppearance").getByRole("radio", { name: "Violet" })).toBeChecked();

  // A second browser with an empty cache starts on the default look, then
  // adopts the principal's saved preferences once the session resolves.
  const second = await newPage(browser);
  await second.page.goto("/");
  await expect(html(second.page)).toHaveAttribute("data-theme", "dark");
  await signInAsMaster(second.page, request);
  await expect(html(second.page)).toHaveAttribute("data-theme", "light");
  await expect(html(second.page)).toHaveAttribute("data-accent", "violet");
  await expect(html(second.page)).toHaveAttribute("data-density", "compact");
  await second.page.locator("#whoami .profilemenu > summary").click();
  await expect(second.page.locator("#profileAppearance").getByRole("radio", { name: "Light" })).toBeChecked();

  await first.context.close();
  await second.context.close();
});
