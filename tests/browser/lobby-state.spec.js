const { test, expect } = require("@playwright/test");

async function adminSession(request) {
  const pairing = await request.post("/pairings?ttl_hours=1", {
    headers: { "x-admin-token": "MASTER" },
  });
  const { pairing_code: pairingCode } = await pairing.json();
  const session = await request.post("/sessions", {
    headers: { "x-pairing-code": pairingCode },
    data: { name: "operator", kind: "user" },
  });
  return session.json();
}

test("lobby is a live location and shares connection and wake vocabulary", async ({ page, request }) => {
  const info = await adminSession(request);
  await page.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({
      token, expiresAt, name: "operator",
    }));
  }, { token: info.session_token, expiresAt: info.expires_at });
  await page.route("**/residents", route => route.fulfill({
    status: 200,
    contentType: "application/json",
    body: JSON.stringify([
      { name: "offline-agent", kind: "agent", online: false, locas: [], runtime: null },
      { name: "waiting-agent", kind: "agent", online: true, locas: [], runtime: null },
      { name: "ready-agent", kind: "agent", online: true, locas: [], runtime: { ready: true } },
      { name: "waiting-human", kind: "user", online: true, locas: [], runtime: null },
      { name: "seated-agent", kind: "agent", online: true, locas: ["iye"], runtime: { ready: true } },
    ]),
  }));

  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate(() => fetchLobby());
  await expect(page.locator("#lobbyCount")).toHaveText("3");
  await expect(page.locator("#lobbyList")).not.toContainText("offline-agent");
  await expect(page.locator("#lobbyList")).not.toContainText("seated-agent");
  await expect(page.locator("#lobbyList .omem").filter({ hasText: "waiting-agent" }))
    .toContainText("waitingconnectedunverified");
  await expect(page.locator("#lobbyList .omem").filter({ hasText: "ready-agent" }))
    .toContainText("waitingconnectedready");
  await expect(page.locator("#lobbyList .omem").filter({ hasText: "waiting-human" }))
    .toContainText("waitingconnectedn/a");
});

test("building resident refresh updates the live lobby without a reload", async ({ page, request }) => {
  const info = await adminSession(request);
  await page.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({
      token, expiresAt, name: "operator",
    }));
  }, { token: info.session_token, expiresAt: info.expires_at });

  let online = false;
  await page.route("**/residents", route => route.fulfill({
    status: 200,
    contentType: "application/json",
    body: JSON.stringify([
      { name: "late-agent", kind: "agent", online, locas: [], runtime: null },
    ]),
  }));

  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate(() => switchTab("people"));
  await expect(page.locator("#lobbyCount")).toHaveText("0");

  online = true;
  await page.evaluate(() => refreshPeopleRuntime());
  await expect(page.locator("#lobbyCount")).toHaveText("1");
  await expect(page.locator("#lobbyList .omem").filter({ hasText: "late-agent" }))
    .toContainText("waitingconnectedunverified");
});
