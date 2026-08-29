const { test, expect } = require("@playwright/test");

// Getting Started is audience-aware and TABBED. It AUTO-opens only in a Host
// context — the Desktop Host wrapper (window.__LOCA_HOST__) or a verified Master.
// An anonymous client/visitor is never auto-shown it and never told they are the
// Master; they open a general (client) view from the "?" Help button. The Desktop
// Host loads this SAME shared web UI, so Web self-host Master and Desktop Host
// behave identically — no desktop fork. Static content: no session, no secret.
// The Skill Library tab's install command is built from the Host's own origin.

test("Desktop Host: tabbed guide auto-opens; tabs switch; dismiss persists", async ({ page }) => {
  await page.addInitScript(() => {
    window.__LOCA_HOST__ = true;
  });
  await page.goto("/");

  const gs = page.locator("#gsOverlay");
  await expect(gs).toBeVisible();
  await expect(gs).toContainText("Welcome to loca");

  // The Host sees the Host guide; the client view is hidden.
  const host = page.locator("#gsOverlay .gs-host");
  await expect(host).toBeVisible();
  await expect(page.locator("#gsOverlay .gs-client")).toBeHidden();

  // The four tabs are present, and Welcome is the default panel.
  for (const label of ["Welcome", "Your Host", "Skill Library", "Join & Request"]) {
    await expect(host.locator(".gstab", { hasText: label })).toBeVisible();
  }
  await expect(page.locator("[data-gspanel='welcome']")).toBeVisible();
  await expect(page.locator("[data-gspanel='welcome']")).toContainText("Building");

  // Switching tabs shows that panel and hides Welcome.
  await host.locator(".gstab", { hasText: "Your Host" }).click();
  await expect(page.locator("[data-gspanel='host']")).toBeVisible();
  await expect(page.locator("[data-gspanel='welcome']")).toBeHidden();
  await expect(page.locator("[data-gspanel='host']")).toContainText("Master");

  // The full-setup doc link exists and actually serves on a closed building.
  await expect(host.locator("a[href='/docs/getting-started.md']")).toHaveCount(1);
  const docResp = await page.request.get("/docs/getting-started.md");
  expect(docResp.status()).toBe(200);

  // Dismiss -> hidden and remembered across a reload.
  await page.locator("#gsGot").click();
  await expect(gs).toBeHidden();
  await page.reload();
  await expect(page.locator("#gsOverlay")).toBeHidden();
});

test("Client/visitor: no auto-open; Help shows the client view without Host/Master framing", async ({ page }) => {
  await page.goto("/");
  await page.waitForTimeout(2000);
  await expect(page.locator("#gsOverlay")).toBeHidden();

  await page.locator("#helpBtn").click();
  await expect(page.locator("#gsOverlay")).toBeVisible();
  await expect(page.locator("#gsOverlay .gs-client")).toBeVisible();
  await expect(page.locator("#gsOverlay .gs-client")).toContainText("Take your seat");
  await expect(page.locator("#gsOverlay .gs-host")).toBeHidden();
});

test("Host: Skill Library install command uses the Host origin; runtime toggle; copy; Esc/backdrop close", async ({ page }) => {
  await page.addInitScript(() => {
    window.__LOCA_HOST__ = true;
  });
  await page.goto("/");

  // Clear the automatic first-open, then reopen from Help.
  await page.locator("#gsGot").click();
  await expect(page.locator("#gsOverlay")).toBeHidden();
  await page.locator("#helpBtn").click();
  await expect(page.locator("#gsOverlay .gs-host")).toBeVisible();

  // Skill Library tab: the install command points at THIS Host's origin and the
  // /downloads/skills/loca endpoint, into the runtime's skills directory — no
  // repo, no git, no credential; a COPY button, never an installer.
  await page.locator(".gstab", { hasText: "Skill Library" }).click();
  const origin = new URL(page.url()).origin;
  const claudeCmd = page.locator("#gsInstallClaude");
  await expect(claudeCmd).toBeVisible();
  await expect(claudeCmd).toContainText(origin + "/downloads/skills/loca");
  await expect(claudeCmd).toContainText("~/.claude/skills");
  await expect(page.locator(".gscopy").first()).toBeVisible();

  // Runtime toggle -> the Codex path; the Claude command hides. Assert the
  // singular agent command id — `.gscmd[data-plat-body='claude']` also matches
  // the caretaker command body, which would trip Playwright strict mode.
  await page.locator(".gsplat button[data-plat='codex']").click();
  const codexCmd = page.locator("#gsInstallCodex");
  await expect(codexCmd).toBeVisible();
  await expect(codexCmd).toContainText("~/.codex/skills");
  await expect(page.locator("#gsInstallClaude")).toBeHidden();

  // Esc closes; reopen; backdrop closes.
  await page.keyboard.press("Escape");
  await expect(page.locator("#gsOverlay")).toBeHidden();
  await page.locator("#helpBtn").click();
  await expect(page.locator("#gsOverlay")).toBeVisible();
  await page.locator("#gsBackdrop").click({ position: { x: 5, y: 5 } });
  await expect(page.locator("#gsOverlay")).toBeHidden();

  // The guide never embeds a credential/secret token.
  await page.locator("#helpBtn").click();
  const overlayText = await page.locator("#gsOverlay").textContent();
  expect(overlayText).not.toContain("mb_");
  expect(overlayText).not.toContain("jrs_");
});

test("Host with a local Skill Library shows the real path and a copy-from-disk command", async ({ page }) => {
  await page.addInitScript(() => {
    window.__LOCA_HOST__ = true;
    // The Desktop Host injects the real versioned library path at boot.
    window.__LOCA_SKILL_LIBRARY__ = "/home/user/.local/share/loca/skill-library/0.8.5";
  });
  await page.goto("/");
  await page.locator("#gsGot").click();
  await page.locator("#helpBtn").click();
  await page.locator(".gstab", { hasText: "Skill Library" }).click();

  // The real absolute path is shown as the primary, offline flow.
  await expect(page.locator("#gsLibPath")).toBeVisible();
  await expect(page.locator("#gsLibPath")).toContainText("/loca/skill-library/0.8.5");

  // The agent command copies loca from the local library — no curl — and is
  // idempotent (removes any old copy first) and never nests loca/loca.
  const claudeCmd = page.locator("#gsInstallClaude");
  await expect(claudeCmd).toContainText("cp -R");
  await expect(claudeCmd).toContainText("rm -rf");
  await expect(claudeCmd).toContainText("~/.claude/skills/loca");
  await expect(claudeCmd).not.toContainText("curl");
  await expect(claudeCmd).not.toContainText("loca/loca");

  // The caretaker command installs BOTH skills (loca + loca-care).
  const careCmd = page.locator("#gsInstallClaudeCare");
  await expect(careCmd).toContainText("loca-care");
  await expect(careCmd).toContainText("cp -R");
});

test("Host: a Skill Library failure shows a visible unavailable status and the download fallback", async ({ page }) => {
  await page.addInitScript(() => {
    window.__LOCA_HOST__ = true;
    // The Desktop reports a failed local install; no path is injected.
    window.__LOCA_SKILL_LIBRARY_ERROR__ = "permission denied";
  });
  await page.goto("/");
  await page.locator("#gsGot").click();
  await page.locator("#helpBtn").click();
  await page.locator(".gstab", { hasText: "Skill Library" }).click();

  // A visible health status names the failure — never a silent swallow.
  await expect(page.locator("#gsLibError")).toBeVisible();
  await expect(page.locator("#gsLibError")).toContainText("Skill Library unavailable");
  await expect(page.locator("#gsLibError")).toContainText("permission denied");
  await expect(page.locator("#gsLibPath")).toBeHidden();

  // The install command falls back to downloading from the Host.
  await expect(page.locator("#gsInstallClaude")).toContainText("/downloads/skills/loca");
});
