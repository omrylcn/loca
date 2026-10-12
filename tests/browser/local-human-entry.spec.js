const { test, expect } = require("@playwright/test");

test("appointed human Operator: profile, real HTTP permissions and visible controls agree", async ({ page, request }) => {
  const root = { "x-admin-token": "MASTER" };
  const name = "appointed-human";
  const admitted = await request.post("/members", { headers: root, data: { name, kind: "user" } });
  expect(admitted.ok()).toBeTruthy();
  const invitation = await request.post("/rooms/operator-test/invites", { headers: root, data: { name } });
  expect(invitation.ok()).toBeTruthy();
  const { token } = await invitation.json();
  const session = await request.post("/sessions", { headers: { "x-room-token": token }, data: { name, kind: "user" } });
  expect(session.ok()).toBeTruthy();
  const { session_token } = await session.json();
  const headers = { "x-session-token": session_token };
  const profile = await (await request.get("/profile?room=operator-test", { headers })).json();
  const appointment = await request.post("/rooms/operator-test/operators", { headers: root, data: { principal_id: profile.principal.id } });
  expect(appointment.ok()).toBeTruthy();
  const appointed = await (await request.get("/profile?room=operator-test", { headers })).json();
  expect(appointed.loca.roles).toContain("operator");
  await page.goto("/");
  await page.locator("#name").fill(name);
  await page.locator("#roomToken").fill(token);
  await page.evaluate(() => doConnect("operator-test"));
  await expect(page.locator("#leadSelect")).toBeVisible();
  const lead = await request.post("/rooms/operator-test/lead", { headers, data: { lead: name } });
  expect(lead.ok()).toBeTruthy();
  const live = await request.put("/rooms/operator-test/settings", { headers, data: { live: true } });
  expect(live.ok()).toBeTruthy();
  await expect(page.locator("#liveToggle")).toBeVisible();
  await expect(page.locator("#adminToggle")).toBeVisible();
  await page.evaluate(() => pollHealth());
  await expect(page.locator("#adminToggle")).toBeVisible();
  const governance = await request.put("/rooms/operator-test/settings", { headers, data: { operators: [name] } });
  expect(governance.status()).toBe(403);
  const other = await request.put("/rooms/not-operated/settings", { headers, data: { live: true } });
  expect(other.status()).toBe(401);
  await page.reload();
  await expect(page.locator("#leadSelect")).toBeVisible();
  await expect(page.locator("#adminbar")).toBeHidden();
});

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
});

test("browser join requests identify a human, without claiming Building authority", async ({ page }) => {
  await page.goto("/");
  await expect(page.locator("#connBox")).toBeVisible();
  await page.locator("#name").fill("human-browser");
  const sent = page.waitForRequest(r => r.url().endsWith("/join-requests") && r.method() === "POST");
  await page.locator("#joinRequestBtn").click();
  const body = (await sent).postDataJSON();
  expect(body).toEqual({ name: "human-browser", kind: "user" });
});

test("human request approval preserves type through membership and browser session", async ({ page, request }) => {
  const root = { "x-admin-token": "MASTER" };
  await request.post("/admission-stock", { headers: root, data: { count: 1 } });
  await page.goto("/");
  await page.locator("#name").fill("human-approved");
  const sent = page.waitForResponse(r => r.url().endsWith("/join-requests") && r.request().method() === "POST");
  await page.locator("#joinRequestBtn").click();
  const { request_id } = await (await sent).json();
  const pending = await (await request.get("/join-requests", { headers: root })).json();
  expect(pending.pending.find(r => r.id === request_id).kind).toBe("user");
  const approved = await request.post("/join-requests/" + request_id + "/approve", { headers: root });
  expect(approved.ok()).toBeTruthy();
  await expect(page.locator("#whoami")).toContainText("human-approved");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  const identity = await page.evaluate(async () => {
    const r = await fetch("/profile", { headers: adminHeaders({}) });
    return r.json();
  });
  expect(identity.principal.kind).toBe("user");
  expect(identity.building_role).toBe("member");
});

test("refresh keeps restored identity connection settings closed", async ({ page, request }) => {
  const response = await request.post("/pairings?ttl_hours=1", { headers: { "x-admin-token": "MASTER" } });
  const { pairing_code } = await response.json();
  await page.goto("/");
  await page.locator("#pairingCode").fill(pairing_code);
  await page.evaluate(() => doConnect("iye"));
  await expect(page.locator("#connBox")).toBeHidden();
  await page.reload();
  await expect(page.locator("#whoami")).toContainText("MASTER");
  await expect(page.locator("#connBox")).toBeHidden();
  await expect(page.locator("#adminbar")).toBeHidden();
});
