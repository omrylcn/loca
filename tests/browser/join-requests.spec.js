const { test, expect } = require("@playwright/test");

// The Getting Started guide auto-shows on a fresh browser (empty localStorage)
// and its backdrop intercepts clicks. These specs drive the connected app, not
// onboarding, so start past the guide. (getting-started.spec.js deliberately
// does NOT seed this — it tests the first-open guide itself.)
test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    try {
      localStorage.setItem("loca-gs-seen", "1");
    } catch (e) {}
  });
});

// Piece 1+2 acceptance at the shared Web UI (same code the Desktop wrapper
// loads, so this pins Web + Host parity): an outside agent's join request is
// visible in the MAIN app and a Master approves it there — not in the hidden
// SSH-tunnel Master Desk. Locks the four review blockers: pending shows in the
// BUILDING view, a failed list fetch shows an error (never a misleading "no
// pending"), approve calls the building-admin-authed endpoint, and no request
// secret ever enters the DOM.
test("join requests are visible and approvable in the main app, with no secret in the DOM", async ({ page, request }) => {
  const admin = { "x-admin-token": "MASTER" };

  // A Master session in the browser, through the real pairing door.
  const paired = await request.post("/pairings?ttl_hours=1", { headers: admin });
  const { pairing_code } = await paired.json();
  await page.goto("/");
  await page.locator("#pairingCode").fill(pairing_code);
  await page.evaluate(() => window.doConnect("iye"));
  await expect(page.locator("#whoami")).toContainText("MASTER");

  // Pre-mint one admission right so approve can consume it, then an outside
  // agent (authless) requests to join.
  await request.post("/admission-stock", { headers: admin, data: { count: 1 } });
  const created = await request.post("/join-requests", { data: { name: "visitor", kind: "agent" } });
  const { request_secret } = await created.json();

  // BUILDING view: the pending request is visible in the main app, actionable.
  await page.locator("#tabPeople").click();
  const jr = page.locator(".jrsection");
  await expect(jr).toContainText("visitor");
  await expect(jr.locator("[data-jr-approve]").first()).toBeVisible();

  // The per-request secret must never appear anywhere in the DOM.
  const html = await page.content();
  expect(html).not.toContain(request_secret);
  expect(html).not.toContain("jrs_");

  // Approve it right here -> it leaves the pending list (one stock right spent).
  await jr.locator("[data-jr-approve]").first().click();
  await expect(page.locator(".jrsection")).toContainText("no pending join requests");

  // A failed list fetch is shown AS AN ERROR, never as a misleading "no pending".
  await page.route("**/join-requests", route => route.fulfill({ status: 500, body: "boom" }));
  await page.evaluate(() => { state.tab = "people"; return refreshPeopleRuntime(); });
  await expect(page.locator(".jrsection")).toContainText("could not load");
});

// Host-mode parity + deny + transport-failure resilience. The Desktop Host loads
// the SAME shared Web UI (it may set window.__LOCA_HOST__), so the panel and its
// actions must behave identically — there is no desktop fork.
test("Host wrapper shows the same panel; deny works and a transport failure re-enables the button", async ({ page, request }) => {
  const admin = { "x-admin-token": "MASTER" };
  // Simulate the Desktop Host wrapper injecting its flag before the app loads.
  await page.addInitScript(() => { window.__LOCA_HOST__ = true; });

  const paired = await request.post("/pairings?ttl_hours=1", { headers: admin });
  const { pairing_code } = await paired.json();
  await page.goto("/");
  await page.locator("#pairingCode").fill(pairing_code);
  await page.evaluate(() => window.doConnect("iye"));
  await expect(page.locator("#whoami")).toContainText("MASTER");

  // An outside agent requests; the SAME panel is visible under the Host flag.
  await request.post("/join-requests", { data: { name: "hostguest", kind: "agent" } });
  await page.locator("#tabPeople").click();
  const jr = page.locator(".jrsection");
  await expect(jr).toContainText("hostguest");

  // DENY calls the authenticated deny endpoint and the request leaves the list.
  const denyReq = page.waitForRequest(r => r.url().includes("/deny") && r.method() === "POST");
  await jr.locator("[data-jr-deny]").first().click();
  await denyReq;
  await expect(page.locator(".jrsection")).toContainText("no pending join requests");

  // A transport failure on approve must NOT consume the request or leave the
  // button stuck disabled: the request stays pending and the button is usable.
  await request.post("/join-requests", { data: { name: "hostguest2", kind: "agent" } });
  await page.evaluate(() => { state.tab = "people"; return refreshPeopleRuntime(); });
  await expect(page.locator(".jrsection")).toContainText("hostguest2");
  await page.route("**/join-requests/*/approve", route => route.abort());
  await page.locator(".jrsection [data-jr-approve]").first().click();
  await expect(page.locator(".jrsection")).toContainText("hostguest2");
  await expect(page.locator(".jrsection [data-jr-approve]").first()).toBeEnabled();
});
