const { test, expect } = require("@playwright/test");

// Getting Started: a first-open guide that shows once per browser and can be
// reopened anytime from the "?" Help button. The Desktop Host loads the SAME
// shared web UI (it may set window.__LOCA_HOST__), so Web self-host Master and
// Desktop Host must behave identically — there is no desktop fork. The guide is
// static content: no session, no network, no secret.

test("first open shows the guide once; 'Got it' dismisses it and it stays dismissed", async ({ page }) => {
  await page.goto("/");

  // A fresh browser (empty localStorage) sees the guide automatically.
  const gs = page.locator("#gsOverlay");
  await expect(gs).toBeVisible();
  await expect(gs).toContainText("Welcome to loca");
  // Content fence: the acceptance-contract items must actually be in the guide.
  await expect(gs).toContainText("You host the building");
  await expect(gs).toContainText("Claude Code or Codex");
  await expect(gs).toContainText("loca-care");
  await expect(gs).toContainText("Care");

  // Dismiss with "Got it" -> hidden and remembered.
  await page.locator("#gsGot").click();
  await expect(gs).toBeHidden();

  // A returning visitor (reload keeps localStorage) is NOT shown it again.
  await page.reload();
  await expect(page.locator("#gsOverlay")).toBeHidden();
});

test("Help reopens the guide; Esc and backdrop close it; identical under the Host flag", async ({ page }) => {
  // Simulate the Desktop Host wrapper injecting its flag before the app loads.
  await page.addInitScript(() => {
    window.__LOCA_HOST__ = true;
  });
  await page.goto("/");

  // Clear the automatic first-open so we exercise the reopen path cleanly.
  await page.locator("#gsGot").click();
  await expect(page.locator("#gsOverlay")).toBeHidden();

  // The "?" Help button reopens the same guide.
  await page.locator("#helpBtn").click();
  await expect(page.locator("#gsOverlay")).toBeVisible();

  // Esc closes it.
  await page.keyboard.press("Escape");
  await expect(page.locator("#gsOverlay")).toBeHidden();

  // Reopen, then close by clicking the backdrop (outside the card).
  await page.locator("#helpBtn").click();
  await expect(page.locator("#gsOverlay")).toBeVisible();
  await page.locator("#gsBackdrop").click({ position: { x: 5, y: 5 } });
  await expect(page.locator("#gsOverlay")).toBeHidden();

  // The static guide never embeds a credential/secret token.
  const html = await page.content();
  expect(html).not.toContain("mb_");
  expect(html).not.toContain("jrs_");
});
