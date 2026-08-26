const { test, expect } = require("@playwright/test");

// Getting Started is audience-aware. It AUTO-opens only in a Host context — the
// Desktop Host wrapper (window.__LOCA_HOST__) or a verified Master. An anonymous
// client/visitor is never auto-shown it and never told they are the Master; they
// open a general (client) view from the "?" Help button. The Desktop Host loads
// this SAME shared web UI, so Web self-host Master and Desktop Host behave
// identically — no desktop fork. Static content: no session, no network, no
// secret. Both views live in the DOM (one hidden via CSS), so we assert
// VISIBILITY to prove which view the audience actually sees.

test("Desktop Host: guide auto-opens with the Host view; dismiss persists", async ({ page }) => {
  await page.addInitScript(() => {
    window.__LOCA_HOST__ = true;
  });
  await page.goto("/");

  const gs = page.locator("#gsOverlay");
  await expect(gs).toBeVisible();
  await expect(gs).toContainText("Welcome to loca");

  // The Host sees the Host view; the client view is hidden.
  await expect(page.locator("#gsOverlay .gs-host")).toBeVisible();
  await expect(page.locator("#gsOverlay .gs-client")).toBeHidden();

  // Content fence: the acceptance-contract items are in the Host view.
  const host = page.locator("#gsOverlay .gs-host");
  await expect(host).toContainText("You host the building");
  await expect(host).toContainText("Claude Code");
  await expect(host).toContainText("Codex");
  await expect(host).toContainText("loca-care");
  await expect(host).toContainText("Care & health");
  await expect(host).toContainText("Focus → Reminders");
  // The full-setup doc link points at the locally-served walkthrough — and it
  // must actually serve (it 401'd on a closed prod building for lacking an
  // allow-list entry; the browser server here runs closed too).
  await expect(host.locator("a[href='/docs/getting-started.md']")).toBeVisible();
  const docResp = await page.request.get("/docs/getting-started.md");
  expect(docResp.status()).toBe(200);

  // Dismiss -> hidden and remembered across a reload.
  await page.locator("#gsGot").click();
  await expect(gs).toBeHidden();
  await page.reload();
  await expect(page.locator("#gsOverlay")).toBeHidden();
});

test("Client/visitor: no auto-open; Help shows the client view without Host/Master framing", async ({ page }) => {
  // No Host flag, no connection -> not a Host context.
  await page.goto("/");

  // The guide must NOT auto-open for an anonymous client (wait past one
  // auto-open interval tick to be sure).
  await page.waitForTimeout(2000);
  await expect(page.locator("#gsOverlay")).toBeHidden();

  // Help opens the general client view.
  await page.locator("#helpBtn").click();
  await expect(page.locator("#gsOverlay")).toBeVisible();
  await expect(page.locator("#gsOverlay .gs-client")).toBeVisible();
  await expect(page.locator("#gsOverlay .gs-client")).toContainText("Take your seat");

  // The client is NEVER shown the "you host / you are Master" framing.
  await expect(page.locator("#gsOverlay .gs-host")).toBeHidden();
});

test("Host: Help reopens; runtime toggle switches Claude Code / Codex; Esc and backdrop close", async ({ page }) => {
  await page.addInitScript(() => {
    window.__LOCA_HOST__ = true;
  });
  await page.goto("/");

  // Clear the automatic first-open, then reopen from Help.
  await page.locator("#gsGot").click();
  await expect(page.locator("#gsOverlay")).toBeHidden();
  await page.locator("#helpBtn").click();
  await expect(page.locator("#gsOverlay .gs-host")).toBeVisible();

  // Runtime toggle: Claude Code shows the ~/.claude path; Codex swaps to ~/.codex.
  const claudeBody = page.locator("[data-plat-body='claude']");
  const codexBody = page.locator("[data-plat-body='codex']");
  await expect(claudeBody).toBeVisible();
  await expect(claudeBody).toContainText("~/.claude/skills/loca");
  await expect(codexBody).toBeHidden();
  await page.locator(".gsplat button[data-plat='codex']").click();
  await expect(codexBody).toBeVisible();
  await expect(codexBody).toContainText("~/.codex/skills/loca");
  await expect(claudeBody).toBeHidden();

  // Esc closes.
  await page.keyboard.press("Escape");
  await expect(page.locator("#gsOverlay")).toBeHidden();

  // Reopen, then close via the backdrop (outside the card).
  await page.locator("#helpBtn").click();
  await expect(page.locator("#gsOverlay")).toBeVisible();
  await page.locator("#gsBackdrop").click({ position: { x: 5, y: 5 } });
  await expect(page.locator("#gsOverlay")).toBeHidden();

  // The guide never embeds a credential/secret token.
  const overlayText = await page.locator("#gsOverlay").textContent();
  expect(overlayText).not.toContain("mb_");
  expect(overlayText).not.toContain("jrs_");
});
