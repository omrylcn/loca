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
  // Approve shows a visible success confirmation — it once looked like nothing
  // happened because the reload wiped the notice before the operator saw it.
  await expect(page.locator("#jrNotice")).toContainText("approved");

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

// Mint + deny each surface a visible success confirmation, not just a silent
// state change — the operator must see that pressing the control did something.
test("mint and deny each show a visible success confirmation", async ({ page, request }) => {
  const admin = { "x-admin-token": "MASTER" };
  const paired = await request.post("/pairings?ttl_hours=1", { headers: admin });
  const { pairing_code } = await paired.json();
  await page.goto("/");
  await page.locator("#pairingCode").fill(pairing_code);
  await page.evaluate(() => window.doConnect("iye"));
  await expect(page.locator("#whoami")).toContainText("MASTER");
  await page.locator("#tabPeople").click();

  // Minting admission rights confirms the count that was created.
  const mint = page.locator(".jrsection [data-jr-mint]").first();
  await expect(mint).toBeVisible();
  await mint.click();
  await expect(page.locator("#jrNotice")).toContainText("admission rights created");

  // Denying a request confirms the denial by name, then THAT row leaves the list.
  // Target the specific `denyme` row: a prior test can leave other requests
  // pending on the shared backend, so `.first()` would deny the wrong row, and a
  // global "no pending" assertion would falsely assume test isolation.
  await request.post("/join-requests", { data: { name: "denyme", kind: "agent" } });
  await page.evaluate(() => { state.tab = "people"; return refreshPeopleRuntime(); });
  const denyRow = page.locator(".jrrow", { hasText: "denyme" });
  await expect(denyRow).toBeVisible();
  await denyRow.locator("[data-jr-deny]").click();
  await expect(page.locator(".jrrow", { hasText: "denyme" })).toHaveCount(0);
  await expect(page.locator("#jrNotice")).toContainText("denied");
});

// The worst combination: approve returns a SERVER error (500, not a transport
// abort) AND the follow-up list refresh also fails. The approve must not consume
// the request, and the panel must stay recoverable — never a stuck, un-retryable
// state. Once the endpoints recover, a refresh restores a usable approve button.
test("a 500 on approve plus a failing refresh keeps the request and stays recoverable", async ({ page, request }) => {
  const admin = { "x-admin-token": "MASTER" };
  const paired = await request.post("/pairings?ttl_hours=1", { headers: admin });
  const { pairing_code } = await paired.json();
  await page.goto("/");
  await page.locator("#pairingCode").fill(pairing_code);
  await page.evaluate(() => window.doConnect("iye"));
  await expect(page.locator("#whoami")).toContainText("MASTER");

  await request.post("/admission-stock", { headers: admin, data: { count: 1 } });
  await request.post("/join-requests", { data: { name: "srverr", kind: "agent" } });
  await page.locator("#tabPeople").click();
  await expect(page.locator(".jrsection")).toContainText("srverr");

  // 500 on approve AND 500 on the list refresh that follows it.
  await page.route("**/join-requests/*/approve", route => route.fulfill({ status: 500, body: "approve failed" }));
  await page.route("**/join-requests", route => route.fulfill({ status: 500, body: "list down" }));
  await page.locator(".jrsection [data-jr-approve]").first().click();

  // The server still holds the pending request — a failed approve consumes nothing.
  const pending = await request.get("/join-requests", { headers: admin }).then(r => r.json());
  expect(pending.pending.map(r => r.name)).toContain("srverr");

  // Recover the endpoints; a refresh restores the request row with a usable,
  // ENABLED approve button (no stuck-disabled control survives the failure).
  await page.unroute("**/join-requests/*/approve");
  await page.unroute("**/join-requests");
  await page.evaluate(() => { state.tab = "people"; return refreshPeopleRuntime(); });
  await expect(page.locator(".jrsection")).toContainText("srverr");
  await expect(page.locator(".jrsection [data-jr-approve]").first()).toBeEnabled();
});
