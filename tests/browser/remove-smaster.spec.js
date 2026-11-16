const { test, expect } = require("@playwright/test");

// People › Ranks: beside a Smaster the Master has Remove (no Rename: a
// Smaster names themselves at sign-in). Remove asks in words under the row.

const MASTER = { "x-admin-token": "MASTER" };

async function masterPage(browser, request) {
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const signed = await (await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name: "rs-master", kind: "user" },
  })).json();
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  await page.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "rs-master" }));
  }, { token: signed.session_token, expiresAt: signed.expires_at });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await expect.poll(() => page.evaluate(() => state.profile?.building_role)).toBe("master");
  await page.evaluate(() => switchTab("people"));
  return page;
}

test("the Master removes a Smaster after a yes in words; No keeps them; the removed one is signed out", async ({ browser, request }) => {
  const { token } = await (await request.post("/smasters", { headers: MASTER, data: { name: "rs-deputy" } })).json();
  const deputy = await (await request.post("/sessions", {
    headers: { "x-admin-token": token }, data: { name: "rs-deputy", kind: "user" },
  })).json();
  expect((await request.get("/rooms", { headers: { "x-session-token": deputy.session_token } })).status()).toBe(200);

  const page = await masterPage(browser, request);
  const row = page.locator("#peopleList .prow.rankrow", { hasText: "rs-deputy" });
  await expect(row).toBeVisible();
  // The Master only removes; a Smaster names themselves.
  await expect(row.locator(".pacts .btn")).toHaveText(["Remove"]);

  // No: nothing happens.
  await row.locator("[data-remove-smaster]").click();
  const ask = page.locator("#peopleList .rankconfirm");
  await expect(ask).toContainText("Remove rs-deputy from the Building?");
  await expect(ask).toContainText("Smaster key stops working");
  await expect(ask).toContainText("signed out");
  await expect(ask).toContainText("floors they founded pass to the Master");
  await ask.locator("[data-remove-smaster-no]").click();
  await expect(ask).toHaveCount(0);
  await expect(row).toBeVisible();
  expect((await request.get("/rooms", { headers: { "x-session-token": deputy.session_token } })).status()).toBe(200);

  // Yes: gone from Ranks; their session and their key open nothing.
  await row.locator("[data-remove-smaster]").click();
  await ask.locator("[data-remove-smaster-yes]").click();
  await expect(page.locator("#peopleList .prow.rankrow", { hasText: "rs-deputy" })).toHaveCount(0);
  await expect.poll(async () => (await request.get("/rooms", {
    headers: { "x-session-token": deputy.session_token },
  })).status()).toBe(401);
  const again = await request.post("/sessions", {
    headers: { "x-admin-token": token }, data: { name: "rs-deputy", kind: "user" },
  });
  expect(again.ok()).toBe(false);
  await page.close();
});

test("an unused Smaster code is offered for deletion, in words about the code", async ({ browser, request }) => {
  await request.post("/smasters", { headers: MASTER, data: { name: "rs-unused" } });
  const page = await masterPage(browser, request);
  const row = page.locator("#peopleList .prow.rankrow", { hasText: "rs-unused" });
  await expect(row).toContainText("unused code");
  await row.locator("[data-remove-smaster]").click();
  const ask = page.locator("#peopleList .rankconfirm");
  await expect(ask).toContainText("Delete the unused Smaster code rs-unused?");
  await ask.locator("[data-remove-smaster-yes]").click();
  await expect(page.locator("#peopleList .prow.rankrow", { hasText: "rs-unused" })).toHaveCount(0);
  await page.close();
});
