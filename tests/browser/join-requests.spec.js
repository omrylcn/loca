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
  await page.locator("#masterEntryButton").click();
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
  await page.locator("#masterEntryButton").click();
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
  await page.locator("#masterEntryButton").click();
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
  await page.locator("#masterEntryButton").click();
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

// ---------------------------------------------------------------------------
// The door's own join request: what it does after approval. The server keeps
// the key for collection until the requester acknowledges it, or 24 hours
// after the approval. The door acknowledges only a key it has signed in with
// as the name that asked and kept in the seat it restores after a reload.
// ---------------------------------------------------------------------------
const { MASTER, smasterKey, freshDoor, switchTo, settle, openSignInForm } = require("./support/switch");
const { provisionRooms } = require("./support/building");

// Ask from the door as `name`; resolves with the request's id and secret
// (read from the server's answer, which only the page holds otherwise).
async function askAtTheDoor(page, name) {
  const created = page.waitForResponse(r => r.url().endsWith("/join-requests") && r.request().method() === "POST");
  await page.locator("#name").fill(name);
  await page.locator("#joinRequestBtn").click();
  const body = await (await created).json();
  await expect(page.locator("#joinRequestStatus")).toContainText("waiting for a Master");
  return { id: body.request_id, secret: body.request_secret };
}

async function approveIt(request, id) {
  expect((await request.post("/admission-stock", { headers: MASTER, data: { count: 1 } })).ok()).toBeTruthy();
  expect((await request.post(`/join-requests/${id}/approve`, { headers: MASTER })).ok()).toBeTruthy();
}

async function serverView(request, asked) {
  const r = await request.get(`/join-requests/${asked.id}`, { headers: { "x-join-secret": asked.secret } });
  expect(r.status()).toBe(200);
  return r.json();
}

async function signedInAs(page, name) {
  await expect.poll(() => page.evaluate(() => state.authStatus === "authenticated" ? state.name : null)).toBe(name);
}

function countAcks(page) {
  const acks = { sent: 0 };
  page.on("request", r => { if (r.method() === "POST" && /\/join-requests\/[^/]+\/ack$/.test(r.url())) acks.sent++; });
  return acks;
}

test("an approval whose window closed stops the watch and says what to do", async ({ browser }) => {
  const { context, page } = await freshDoor(browser);
  let polls = 0;
  await page.route(/\/join-requests\/jr_[^/]+$/, route => {
    if (route.request().method() !== "GET") return route.fallback();
    polls++;
    return route.fulfill({
      status: 200, contentType: "application/json",
      body: JSON.stringify({ status: "approved", bootstrap_ready: false, bootstrap_expired: true }),
    });
  });
  await askAtTheDoor(page, "lapsed-human");
  await expect(page.locator("#joinRequestStatus")).toContainText("not collected within 24 hours");
  await expect(page.locator("#joinRequestStatus")).toContainText("ask a Master to give you access again");
  const seen = polls;
  await settle(page, 4000);   // longer than the 3 s watch
  expect(polls).toBe(seen);
  await context.close();
});

test("a collection refused with 410 says the same and takes no seat", async ({ browser }) => {
  const { context, page } = await freshDoor(browser);
  await page.route(/\/join-requests\/jr_[^/]+$/, route => {
    if (route.request().method() !== "GET") return route.fallback();
    return route.fulfill({
      status: 200, contentType: "application/json",
      body: JSON.stringify({ status: "approved", bootstrap_ready: true, bootstrap_expired: false }),
    });
  });
  await page.route(/\/join-requests\/jr_[^/]+\/bootstrap$/, route =>
    route.fulfill({ status: 410, body: "gone" }));
  await askAtTheDoor(page, "racer-human");
  await expect(page.locator("#joinRequestStatus")).toContainText("not collected within 24 hours");
  expect(await page.evaluate(() => state.authStatus)).not.toBe("authenticated");
  await context.close();
});

test("the door acknowledges a kept key, and a reload finishes an acknowledgement that failed", async ({ browser, request }) => {
  const { context, page } = await freshDoor(browser);
  const acks = countAcks(page);
  await page.route(/\/join-requests\/[^/]+\/ack$/, route => route.abort());
  const asked = await askAtTheDoor(page, "jr-kept");
  await approveIt(request, asked.id);

  // Signed in with the collected key; the ACK was tried and failed. The
  // person stays in, and the server still holds the copy for a retry.
  await signedInAs(page, "jr-kept");
  await expect.poll(() => acks.sent).toBe(1);
  expect(await page.evaluate(() => JSON.parse(localStorage.getItem("loca-seat")).name)).toBe("jr-kept");
  expect(await serverView(request, asked)).toEqual({ status: "approved", bootstrap_ready: true, bootstrap_expired: false });

  // A reload retakes the kept seat and sends the owed ACK.
  await page.unroute(/\/join-requests\/[^/]+\/ack$/);
  await page.reload();
  await signedInAs(page, "jr-kept");
  await expect.poll(async () => (await serverView(request, asked)).bootstrap_ready).toBe(false);
  expect(await serverView(request, asked)).toEqual({ status: "approved", bootstrap_ready: false, bootstrap_expired: false });
  // Paid: nothing more is owed in this tab.
  expect(await page.evaluate(() => Object.keys(sessionStorage).filter(k => k.startsWith("loca-join-ack-")))).toEqual([]);
  await context.close();
});

test("a browser that cannot keep the seat does not acknowledge the key", async ({ browser, request }) => {
  const context = await browser.newContext();
  await context.addInitScript(() => {
    localStorage.setItem("loca-gs-seen", "1");
    const setItem = Storage.prototype.setItem;
    Storage.prototype.setItem = function (key, value) {
      if (this === window.localStorage && key === "loca-seat") throw new DOMException("full", "QuotaExceededError");
      return setItem.call(this, key, value);
    };
  });
  const page = await context.newPage();
  const acks = countAcks(page);
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("unauthenticated");
  const asked = await askAtTheDoor(page, "jr-unkept");
  await approveIt(request, asked.id);

  await signedInAs(page, "jr-unkept");
  await expect(page.locator("#joinRequestStatus")).toContainText("could not keep your key");
  await settle(page, 1500);
  expect(acks.sent).toBe(0);
  expect((await serverView(request, asked)).bootstrap_ready).toBe(true);
  await context.close();
});

test("another key for the same name does not acknowledge a key this browser did not keep", async ({ browser, request }) => {
  // The browser refuses to keep a seat that holds a membership key, and
  // keeps any other seat.
  const context = await browser.newContext();
  await context.addInitScript(() => {
    localStorage.setItem("loca-gs-seen", "1");
    const setItem = Storage.prototype.setItem;
    Storage.prototype.setItem = function (key, value) {
      if (this === window.localStorage && key === "loca-seat" && String(value).includes('"roomToken":"mb_')) {
        throw new DOMException("full", "QuotaExceededError");
      }
      return setItem.call(this, key, value);
    };
  });
  const page = await context.newPage();
  const acks = countAcks(page);
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("unauthenticated");
  const asked = await askAtTheDoor(page, "jr-twokeys");
  await approveIt(request, asked.id);
  await signedInAs(page, "jr-twokeys");
  await expect(page.locator("#joinRequestStatus")).toContainText("could not keep your key");
  expect(acks.sent).toBe(0);

  // A Master gives the same name an invite; the person signs in with it here.
  // That seat is kept, under the asking name, and still proves nothing about
  // the collected key.
  await provisionRooms(request, ["jr-two"]);
  const invited = await request.post("/rooms/jr-two/invites", { headers: MASTER, data: { name: "jr-twokeys" } });
  expect(invited.status(), await invited.text()).toBe(200);
  const { token } = await invited.json();
  await openSignInForm(page);
  await page.locator("#name").fill("jr-twokeys");
  await page.locator("#roomToken").fill(token);
  await page.evaluate(() => window.doConnect("jr-two"));
  await expect(page.locator("#curRoom")).toHaveText("jr-two");
  await signedInAs(page, "jr-twokeys");
  expect(await page.evaluate(() => JSON.parse(localStorage.getItem("loca-seat")).roomToken)).toBe(token);
  await settle(page, 1500);
  expect(acks.sent).toBe(0);

  // Nor after a reload, when the collected key is no longer there to compare.
  await page.reload();
  await signedInAs(page, "jr-twokeys");
  await settle(page, 1500);
  expect(acks.sent).toBe(0);
  expect((await serverView(request, asked)).bootstrap_ready).toBe(true);
  await context.close();
});

test("a key owed an acknowledgement is not acknowledged for the next person at the tab", async ({ browser, request }) => {
  const nextKey = await smasterKey(request, "jrnext");
  const { context, page } = await freshDoor(browser);
  const acks = countAcks(page);
  await page.route(/\/join-requests\/[^/]+\/ack$/, route => route.abort());
  const asked = await askAtTheDoor(page, "jr-first");
  await approveIt(request, asked.id);
  await signedInAs(page, "jr-first");
  await expect.poll(() => acks.sent).toBe(1);

  // The asker leaves; someone else signs in at this tab. Their seat being
  // taken must not send the asker's ACK.
  await page.unroute(/\/join-requests\/[^/]+\/ack$/);
  await switchTo(page, nextKey, "jrnext");
  await settle(page, 1500);
  expect(acks.sent).toBe(1);
  expect((await serverView(request, asked)).bootstrap_ready).toBe(true);
  await context.close();
});
