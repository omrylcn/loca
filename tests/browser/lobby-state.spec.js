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
  // Every member is in the Lobby room now; the separate "who waits" list
  // is gone. People shows each member's connection and wake state.
  await expect(page.locator("#lobbyBox")).toHaveCount(0);
  await page.evaluate(() => switchTab("people"));
  const row = (name) => page.locator("#peopleList .prow").filter({ hasText: name }).locator(".pstatuses");
  await expect(row("offline-agent")).toHaveText("disconnectedunverified");
  await expect(row("waiting-agent")).toHaveText("connectedunverified");
  await expect(row("ready-agent")).toHaveText("connectedready");
  await expect(row("waiting-human")).toHaveText("connectedn/a");
  await expect(row("seated-agent")).toHaveText("connectedready");
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
  const late = page.locator("#peopleList .prow").filter({ hasText: "late-agent" }).locator(".pstatuses");
  await expect(late).toHaveText("disconnectedunverified");

  online = true;
  await page.evaluate(() => refreshPeopleRuntime());
  await expect(late).toHaveText("connectedunverified");
});

test("a delayed Building load cannot overwrite a newer online lobby snapshot", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await page.goto("/");
  await page.evaluate(() => {
    if (healthTimer) clearInterval(healthTimer);
    state.adminSession = true; state.session = "isolated-fixture"; state.tab = "people";
  });
  let residentReads = 0, joinReads = 0, release;
  const held = new Promise(resolve => { release = resolve; });
  await page.route("**/rooms", route => route.fulfill({ json: [] }));
  await page.route("**/admission-stock", route => route.fulfill({ json: { available: 0, total: 0 } }));
  await page.route("**/join-requests", async route => {
    if (++joinReads === 1) await held;
    await route.fulfill({ json: { pending: [] } });
  });
  await page.route("**/residents", route => route.fulfill({ json: [{
    name: "late-agent", kind: "agent", online: ++residentReads > 1, locas: [], runtime: null,
  }] }));
  await page.evaluate(() => { window.oldPeopleLoad = fetchPeople(); });
  await expect.poll(() => joinReads).toBe(1);
  await page.evaluate(() => refreshPeopleRuntime());
  await expect.poll(() => page.evaluate(() => state.lobby.length)).toBe(1);
  release();
  await page.evaluate(() => window.oldPeopleLoad);
  expect(await page.evaluate(() => state.lobby.length)).toBe(1);
  expect(await page.evaluate(() => state.people[0].online)).toBe(true);
});

test("a resident response from an ended admin session cannot repopulate the lobby", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await page.goto("/");
  await page.evaluate(() => {
    if (healthTimer) clearInterval(healthTimer);
    state.adminSession = true; state.session = "isolated-old-session";
  });
  let release;
  const held = new Promise(resolve => { release = resolve; });
  await page.route("**/residents", async route => {
    await held;
    await route.fulfill({ json: [{ name: "old-private-member", kind: "agent", online: true, locas: [] }] });
  });
  const requested = page.waitForRequest("**/residents");
  await page.evaluate(() => { window.oldLobbyLoad = fetchLobby(); });
  await requested;
  await page.evaluate(() => {
    state.adminSession = false; state.adminOpen = false; state.session = null;
    state.lobby = []; renderLobby();
  });
  release();
  await page.evaluate(() => window.oldLobbyLoad);
  expect(await page.evaluate(() => (state.lobby || []).map(p => p.name))).not.toContain("old-private-member");
});

test("People say what runs an agent, as its runtime reported", async ({ page, request }) => {
  const info = await adminSession(request);
  await page.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "operator" }));
  }, { token: info.session_token, expiresAt: info.expires_at });
  await page.route("**/residents", route => route.fulfill({
    status: 200,
    contentType: "application/json",
    body: JSON.stringify([
      { name: "codex-agent", kind: "agent", online: true, locas: [],
        runtime: { ready: true, profile: { runtime: "codex-v2", sandbox: "workspace-write", approval: "never" } } },
      { name: "claude-agent", kind: "agent", online: true, locas: [],
        runtime: { ready: true, profile: { runtime: "claude-monitor" } } },
      { name: "old-kit-agent", kind: "agent", online: true, locas: [], runtime: { ready: true } },
    ]),
  }));
  await page.goto("/");
  await page.evaluate(() => switchTab("people"));
  const runtimeOf = (name) => page.locator("#peopleList .prow").filter({ hasText: name }).locator(".pstatus.runtime");
  await expect(runtimeOf("codex-agent")).toHaveText("Codex · sandbox workspace-write · approvals never");
  await expect(runtimeOf("codex-agent")).toHaveClass(/warn/);
  await expect(runtimeOf("codex-agent")).toHaveAttribute("title", /does not stop to ask a person/);
  await expect(runtimeOf("claude-agent")).toHaveText("Claude Code");
  await expect(runtimeOf("claude-agent")).toHaveAttribute("title", /set in the runtime itself/);
  await expect(runtimeOf("old-kit-agent")).toHaveCount(0);
});
