const { test, expect } = require("@playwright/test");

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({
      token: "st_provisional",
      expiresAt: Date.now() + 60_000,
      name: "operator",
    }));
  });
});

async function expectLoginHidden(page) {
  await expect(page.locator("body")).toHaveClass(/auth-pending/);
  await expect(page.locator("#connBox")).toBeHidden();
  await expect(page.locator("#pairingCode")).toBeHidden();
}

test("a slow whoami never renders the login door and times out to a retry state", async ({ page }) => {
  await page.route("**/whoami", async (route) => {
    await new Promise(resolve => setTimeout(resolve, 10_000));
    await route.fulfill({ status: 200, contentType: "application/json", body: '{"name":"operator"}' });
  });

  await page.goto("/");
  await expectLoginHidden(page);
  await expect(page.locator("#doorline")).toContainText("connection unavailable", { timeout: 4_000 });
  await expectLoginHidden(page);
});

test("an explicit whoami 401 renders login and clears the rejected session", async ({ page }) => {
  await page.route("**/whoami", route => route.fulfill({ status: 401, body: "unauthorized" }));

  await page.goto("/");
  await expect(page.locator("body")).toHaveClass(/locked/);
  await expect(page.locator("body")).not.toHaveClass(/auth-pending/);
  await expect(page.locator("#connBox")).toBeVisible();
  await expect(page.locator("#pairingCode")).toBeVisible();
  await expect.poll(() => page.evaluate(() => localStorage.getItem("loca-admin-session"))).toBeNull();
});

for (const failure of ["5xx", "network error"]) {
  test(`a whoami ${failure} preserves identity and shows retry instead of login`, async ({ page }) => {
    await page.route("**/whoami", route => failure === "5xx"
      ? route.fulfill({ status: 503, body: "unavailable" })
      : route.abort("connectionfailed"));

    await page.goto("/");
    await expect(page.locator("#doorline")).toContainText("connection unavailable");
    await expectLoginHidden(page);
    await expect.poll(() => page.evaluate(() => {
      const cached = JSON.parse(localStorage.getItem("loca-admin-session") || "null");
      return cached?.token || null;
    })).toBe("st_provisional");
  });
}

test("entering a loca does not add a synthetic login entry to browser history", async ({ page, request }) => {
  const pairing = await request.post("/pairings?ttl_hours=1", {
    headers: { "x-admin-token": "MASTER" },
  });
  expect(pairing.status()).toBe(201);
  const { pairing_code: pairingCode } = await pairing.json();
  const session = await request.post("/sessions", {
    headers: { "x-pairing-code": pairingCode },
    data: { name: "operator", kind: "user" },
  });
  expect(session.ok()).toBeTruthy();
  const info = await session.json();

  await page.goto("/health");
  await page.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-admin-session", JSON.stringify({
      token,
      expiresAt,
      name: "operator",
    }));
  }, { token: info.session_token, expiresAt: info.expires_at });
  await page.goto("/");
  await expect(page.locator("#roomList button.room").first()).toBeVisible();
  const before = await page.evaluate(() => history.length);
  await page.locator("#roomList button.room").first().click();
  await expect.poll(() => page.evaluate(() => state.room)).not.toBeNull();
  expect(await page.evaluate(() => history.length)).toBe(before);

  await page.goBack();
  await expect(page).toHaveURL(/\/health$/);
});

test("moving between locas never replays stale credentials or renders login", async ({ page, request }) => {
  const pairing = await request.post("/pairings?ttl_hours=1", {
    headers: { "x-admin-token": "MASTER" },
  });
  const { pairing_code: pairingCode } = await pairing.json();
  const session = await request.post("/sessions", {
    headers: { "x-pairing-code": pairingCode },
    data: { name: "operator", kind: "user" },
  });
  const info = await session.json();
  await page.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "operator" }));
  }, { token: info.session_token, expiresAt: info.expires_at });
  await page.route("**/rooms", route => route.fulfill({
    status: 200,
    contentType: "application/json",
    body: JSON.stringify([
      { room: "alpha-switch", humans: 1, agents: 0 },
      { room: "beta-switch", humans: 1, agents: 0 },
    ]),
  }));
  await page.goto("/");
  await expect(page.locator('#roomList button.room:has-text("alpha-switch")')).toBeVisible();
  await page.locator('#roomList button.room:has-text("alpha-switch")').click();
  await expect.poll(() => page.evaluate(() => state.room)).toBe("alpha-switch");
  await page.locator("#pairingCode").evaluate(el => { el.value = "stale-spent-credential"; });
  await page.evaluate(() => {
    window.__loginSeenDuringRoomSwitch = false;
    const loginVisible = () => document.body.classList.contains("locked");
    new MutationObserver(() => {
      if (loginVisible()) window.__loginSeenDuringRoomSwitch = true;
    }).observe(document.documentElement, { attributes: true, childList: true, subtree: true });
  });
  const before = await page.evaluate(() => localStorage.getItem("loca-admin-session"));

  await page.locator('#roomList button.room:has-text("beta-switch")').click();
  await expect.poll(() => page.evaluate(() => state.room)).toBe("beta-switch");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await expect(page.locator("body")).not.toHaveClass(/locked|auth-pending/);
  expect(await page.evaluate(() => window.__loginSeenDuringRoomSwitch)).toBe(false);
  expect(await page.evaluate(() => localStorage.getItem("loca-admin-session"))).toBe(before);
});
