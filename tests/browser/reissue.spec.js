const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

// "Re-send invitation": where a call answers `already`, the Master can replace
// the member's invite with a fresh one. Synthetic names only.
const MASTER = { "x-admin-token": "MASTER" };
const ROOM = "reissue-room";

async function masterSession(request) {
  const paired = await request.post("/pairings?ttl_hours=1", { headers: MASTER });
  const { pairing_code } = await paired.json();
  const signedIn = await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name: "reissue-boss", kind: "user" },
  });
  expect(signedIn.ok()).toBe(true);
  return signedIn.json();
}

async function openRoom(page, request) {
  await provisionRooms(request, [ROOM]);
  const info = await masterSession(request);
  await page.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "reissue-boss" }));
  }, { token: info.session_token, expiresAt: info.expires_at });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate((room) => joinRoom(room), ROOM);
  await expect.poll(() => page.evaluate(() => state.room)).toBe(ROOM);
  // The call list lives in the This Loca view.
  await page.locator("#sideLocaTab").click();
  await expect(page.locator("#callBtn")).toBeVisible();
}

async function admitted(request, name) {
  const r = await request.post("/members", { headers: MASTER, data: { name, kind: "agent" } });
  expect(r.ok()).toBe(true);
}

const sysLines = (page) => page.locator("#feed .sysline");
const reissueButton = (page) => page.locator("#callList [data-call-reissue]");

/// Calls answer `already` and the reissue answers `reissue` (a route handler).
async function mockCall(page, reissue) {
  const bodies = [];
  await page.route(`**/rooms/${ROOM}/call`, async (route) => {
    const body = JSON.parse(route.request().postData() || "{}");
    bodies.push(body);
    if (body.reissue === true) return reissue(route, body);
    return route.fulfill({ json: { already: true, name: body.name } });
  });
  return bodies;
}

test("a seated member's invitation is re-sent against the real server", async ({ page, request }) => {
  const name = "reissue-agent";
  await provisionRooms(request, [ROOM]);
  await admitted(request, name);
  await openRoom(page, request);

  // The dialog was opened while the member was free; someone seated them in
  // the meantime, so the call answers `already`.
  await page.locator("#callBtn").click();
  const row = page.locator("#callDialog .callcand").filter({ hasText: name }).getByRole("checkbox");
  await expect(row).toBeVisible();
  const called = await request.post(`/rooms/${ROOM}/call`, { headers: MASTER, data: { name } });
  expect(called.status()).toBe(201);
  const old = (await called.json()).token;
  expect((await request.get(`/rooms/${ROOM}/messages`, { headers: { "x-room-token": old } })).status()).toBe(200);
  await row.check();
  await page.locator("#callSubmit").click();

  await expect(sysLines(page).filter({ hasText: `${name} already has a seat in this loca` })).toHaveCount(1);
  const resend = page.locator("#callDialog [data-call-dialog-reissue]");
  await expect(resend).toHaveText("Re-send invitation");
  await resend.click();
  await expect(sysLines(page).filter({ hasText: `fresh invitation sent to ${name}` })).toHaveCount(1);
  await expect(page.locator("#callDialog .callresult").filter({ hasText: name })).toContainText("fresh invitation sent");
  // The page never shows the invite itself.
  expect(await page.locator("body").innerText()).not.toContain("dv_");

  expect((await request.get(`/rooms/${ROOM}/messages`, { headers: { "x-room-token": old } })).status()).toBe(401);
  const invites = await (await request.get(`/rooms/${ROOM}/invites`, { headers: MASTER })).json();
  expect(invites.filter(d => d.name === name)).toHaveLength(1);
});

test("an older server that ignores the flag is reported, not claimed as re-sent", async ({ page, request }) => {
  await openRoom(page, request);
  const bodies = await mockCall(page, (route, body) =>
    route.fulfill({ json: { already: true, name: body.name } }));
  await page.evaluate(() => callIn("old-server-agent"));
  await reissueButton(page).click();
  await expect(sysLines(page).filter({ hasText: "this server cannot re-send invitations yet; old-server-agent's invitation was not changed" })).toHaveCount(1);
  await expect(sysLines(page).filter({ hasText: "fresh invitation sent" })).toHaveCount(0);
  expect(bodies).toEqual([{ name: "old-server-agent" }, { name: "old-server-agent", reissue: true }]);
});

test("a refused re-send shows the server's sentence", async ({ page, request }) => {
  await openRoom(page, request);
  await mockCall(page, (route) => route.fulfill({
    status: 403, contentType: "text/plain",
    body: "cannot end a davet issued by a higher or unknown authority",
  }));
  await page.evaluate(() => callIn("ranked-agent"));
  await reissueButton(page).click();
  await expect(sysLines(page).filter({ hasText: "could not re-send ranked-agent's invitation: cannot end a davet issued by a higher or unknown authority" })).toHaveCount(1);
});

test("nothing is written once the Building generation has moved on", async ({ page, request }) => {
  await openRoom(page, request);
  let release;
  const held = new Promise(resolve => { release = resolve; });
  await mockCall(page, async (route) => {
    await held;
    await route.fulfill({ status: 201, json: { name: "late-agent", loca: ROOM, token: "dv_synthetic" } })
      .catch(() => {});
  });
  await page.evaluate(() => callIn("late-agent"));
  await expect(reissueButton(page)).toBeVisible();
  const before = await sysLines(page).count();

  // The generation moves on while the answer is still out, without the
  // request being cancelled: the late answer must write nothing.
  const sent = page.waitForRequest(r => r.url().endsWith(`/rooms/${ROOM}/call`)
    && (r.postData() || "").includes("reissue"));
  await reissueButton(page).click();
  await sent;
  await page.evaluate(() => { state.buildingGen++; });
  release();
  await page.waitForTimeout(300);
  expect(await sysLines(page).count()).toBe(before);
  await expect(reissueButton(page)).toBeVisible();

  // The page's own sign-out path cancels the request; the cancellation is
  // not reported either.
  let releaseAgain;
  const heldAgain = new Promise(resolve => { releaseAgain = resolve; });
  await page.unrouteAll({ behavior: "ignoreErrors" });
  await mockCall(page, async (route) => {
    await heldAgain;
    await route.fulfill({ status: 201, json: { name: "late-agent", loca: ROOM, token: "dv_synthetic" } })
      .catch(() => {});
  });
  const sentAgain = page.waitForRequest(r => r.url().endsWith(`/rooms/${ROOM}/call`)
    && (r.postData() || "").includes("reissue"));
  await reissueButton(page).click();
  await sentAgain;
  await page.evaluate(() => moveBuildingOn());
  releaseAgain();
  await page.waitForTimeout(300);
  expect(await sysLines(page).count()).toBe(before);
});
