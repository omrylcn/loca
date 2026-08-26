const { test, expect } = require("@playwright/test");

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
