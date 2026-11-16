const { test, expect } = require("@playwright/test");

const MASTER = { "x-admin-token": "MASTER" };

// The live Building is reached over plain http on a VPN address: no secure
// context, so no Clipboard API. Copy must still put the whole code on the
// clipboard, and that code must sign a Smaster in from the door.
test("Key generator Copy works without a secure context, and the copied Smaster key signs in", async ({ browser, request }) => {
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const m = await (await request.post("/sessions", { headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" } })).json();
  const ctx = await browser.newContext();
  const page = await ctx.newPage();
  await page.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "" }));
    // Behave like http://10.x: no secure context, no navigator.clipboard.
    Object.defineProperty(window, "isSecureContext", { value: false });
    Object.defineProperty(navigator, "clipboard", { value: undefined });
    // Record what execCommand("copy") would put on the clipboard.
    window.__copied = null;
    const real = document.execCommand.bind(document);
    document.execCommand = (cmd, ...rest) => {
      if (cmd === "copy") { window.__copied = document.activeElement?.value ?? String(window.getSelection()); return true; }
      return real(cmd, ...rest);
    };
  }, { token: m.session_token, expiresAt: m.expires_at });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate(() => switchTab("people"));
  await page.locator("#btabKeys").click();
  await page.locator("#keyKind").selectOption("smaster");
  await page.locator("#keyForm button[type=submit]").click();
  const shown = (await page.locator("#keySecret").textContent()).trim();
  expect(shown.length).toBeGreaterThan(16);
  await page.locator("[data-key-copy]").click();
  await expect(page.locator("[data-key-copy]")).toHaveText("Copied");
  const copied = await page.evaluate(() => window.__copied);
  expect(copied).toBe(shown);
  await ctx.close();

  // The copied code opens the door for a Smaster who types their name.
  const door = await browser.newPage();
  await door.goto("/");
  await door.locator("#entryKind").selectOption("smaster");
  await door.locator("#name").fill("copy-check");
  await door.locator("#roomToken").fill(copied);
  await door.locator("#connectBtn").click();
  await expect.poll(() => door.evaluate(() => state.authStatus)).toBe("authenticated");
  await expect.poll(() => door.evaluate(() => state.name)).toBe("copy-check");
  await door.close();
});
