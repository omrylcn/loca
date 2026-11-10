const { test, expect } = require("@playwright/test");

const MASTER = { "x-admin-token": "MASTER" };

async function adminSession(request, key, name) {
  if (key === "MASTER") {
    const paired = await request.post("/pairings?ttl_hours=1", { headers: MASTER });
    const { pairing_code } = await paired.json();
    const signedIn = await request.post("/sessions", {
      headers: { "x-pairing-code": pairing_code }, data: { name, kind: "user" },
    });
    expect(signedIn.ok()).toBe(true);
    return signedIn.json();
  }
  const signedIn = await request.post("/sessions", {
    headers: { "x-admin-token": key }, data: { name, kind: "user" },
  });
  expect(signedIn.ok()).toBe(true);
  return signedIn.json();
}

async function openAs(page, info, name) {
  await page.addInitScript(({ token, expiresAt, name }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name }));
  }, { token: info.session_token, expiresAt: info.expires_at, name });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
}

test("Building lists the Master and every Smaster by name, with the rank beside it", async ({ page, request }) => {
  const created = await request.post("/smasters", { headers: MASTER, data: { name: "ui-rank-listed" } });
  expect(created.ok()).toBe(true);
  // A title is not a name.
  const refused = await request.post("/smasters", { headers: MASTER, data: { name: "Smaster" } });
  expect(refused.status()).toBe(400);

  const info = await adminSession(request, "MASTER", "operator");
  await openAs(page, info, "operator");
  await page.evaluate(() => switchTab("people"));
  await expect.poll(() => page.evaluate(() => state.profile?.building_role)).toBe("master");
  const rows = page.locator("#peopleList .prow.rankrow");
  await expect(rows.filter({ hasText: "ui-rank-listed" })).toHaveCount(1);
  await expect(rows.filter({ hasText: "ui-rank-listed" }).locator(".rankbadge")).toHaveText("Smaster");
  await expect(rows.locator(".rankbadge.rank-master")).toHaveCount(1);
  await expect(rows.filter({ hasText: "ui-rank-listed" })).toContainText("not in a loca");
});

test("a Smaster renames its own profile; titles and taken names are refused", async ({ page, request }) => {
  const created = await request.post("/smasters", { headers: MASTER, data: { name: "ui-rank-before" } });
  const { token } = await created.json();
  const info = await adminSession(request, token, "ignored-body-name");
  await openAs(page, info, "ui-rank-before");
  await page.evaluate(() => switchTab("people"));
  await expect.poll(() => page.evaluate(() => state.profile?.building_role)).toBe("smaster");

  await page.locator("#whoami summary").first().click();
  const input = page.locator("#profileNameInput");
  await expect(input).toHaveValue("ui-rank-before");
  await expect(page.locator("#whoami .profileconnected .rankbadge")).toHaveText("Smaster");

  await input.fill("master");
  await page.locator("#profileRename").click();
  await expect(page.locator("#profileRenameNote")).toContainText("title");

  await input.fill("ui-rank-after");
  await page.locator("#profileRename").click();
  await expect(page.locator("#profileRenameNote")).toHaveText("Renamed to ui-rank-after.");
  await expect.poll(() => page.evaluate(() => state.name)).toBe("ui-rank-after");
  await expect(page.locator("#whoami .profileconnected b").first()).toHaveText("ui-rank-after");

  // Same key, same rank, new name — the Building now lists the new name.
  const ranks = await (await request.get("/building/ranks", { headers: MASTER })).json();
  expect(ranks.some(r => r.name === "ui-rank-after" && r.role === "smaster")).toBe(true);
  expect(ranks.some(r => r.name === "ui-rank-before")).toBe(false);
});

test("loca roster shows rank badges and offers no moderation the server would refuse", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await page.goto("/");
  await page.evaluate(() => {
    state.room = "rank-room";
    state.name = "deputy";
    state.adminSession = true;
    state.profile = { building_role: "smaster" };
    state.members = [
      { name: "omrylcn", type: "user", role: "master" },
      { name: "deputy", type: "user", role: "smaster" },
      { name: "selin", type: "user", role: "smaster" },
      { name: "worker", type: "agent" },
    ];
    renderMembers();
  });
  const row = name => page.locator("#onlineList .omem").filter({ hasText: name });
  await expect(row("omrylcn").locator(".rankbadge")).toHaveText("Master");
  await expect(row("selin").locator(".rankbadge")).toHaveText("Smaster");
  await expect(row("worker").locator(".rankbadge")).toHaveCount(0);
  // A Smaster sees no mute/kick/ban on the Master or a peer Smaster, but does on a member.
  await expect(row("omrylcn").locator("[data-mod]")).toHaveCount(0);
  await expect(row("selin").locator("[data-mod]")).toHaveCount(0);
  await expect(row("worker").locator('[data-mod="kick"]')).toHaveCount(1);
  // The Master may moderate a Smaster.
  await page.evaluate(() => { state.profile.building_role = "master"; state.name = "omrylcn"; renderMembers(); });
  await expect(row("selin").locator('[data-mod="kick"]')).toHaveCount(1);
  await expect(row("omrylcn").locator("[data-mod]")).toHaveCount(0);
});

test("Building header counts what the directory shows, and a phone never scrolls sideways", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await page.setViewportSize({ width: 390, height: 800 });
  await page.goto("/");
  // Open the Building view without a live server fetch, then render fixtures.
  await page.evaluate(() => { state.adminSession = false; switchTab("people"); });
  await page.evaluate(() => {
    state.adminSession = true;
    state.profile = { building_role: "master" };
    state.ranks = [{ name: "omrylcn", role: "master", locas: ["fa-backend"] }, { name: "ayse", role: "smaster", locas: [] }];
    state.people = [
      { name: "claude-api", kind: "agent", online: true, locas: ["fa-backend"], membership_id: "mbid_a", runtime: { ready: true } },
      { name: "a-very-long-agent-name-that-should-not-push-the-page", kind: "agent", online: false, locas: [], membership_id: "mbid_b" },
      { name: "selin", kind: "user", online: true, locas: [], membership_id: "mbid_c" },
    ];
    state.joinRequests = [];
    renderPeople();
  });
  await expect(page.locator("#subStatus")).toHaveText("2 ranks · 3 members · 2 online · 1 in lobby");
  await expect(page.locator("#peopleList .locachip").first()).toHaveText("#fa-backend");
  await expect(page.locator("[data-revoke-member]").first()).toHaveText("Revoke");
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
  expect(overflow).toBeLessThanOrEqual(0);
});

test("a rename carries the rank's past messages, live in every open feed and after reload", async ({ page, browser, request }) => {
  const { provisionRooms } = require("./support/building");
  await provisionRooms(request, ["rename-room"]);
  const { token } = await (await request.post("/smasters", { headers: MASTER, data: { name: "ui-said-before" } })).json();
  const info = await adminSession(request, token, "x");
  await page.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    if (!sessionStorage.getItem("init")) {
      sessionStorage.setItem("init", "1");
      localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "ui-said-before" }));
      localStorage.setItem("loca-seated-rooms", JSON.stringify(["rename-room"]));
    }
  }, { token: info.session_token, expiresAt: info.expires_at });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate(() => joinRoom("rename-room"));
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  await page.fill("#msg", "said under the old name");
  await page.keyboard.press("Enter");
  await expect(page.locator("#feed")).toContainText("said under the old name");

  // Someone else watches the same loca in another browser.
  const other = await browser.newPage();
  const watcher = await adminSession(request, "MASTER", "w");
  await other.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "Master" }));
  }, { token: watcher.session_token, expiresAt: watcher.expires_at });
  await other.goto("/");
  await expect.poll(() => other.evaluate(() => state.authStatus)).toBe("authenticated");
  await other.evaluate(() => joinRoom("rename-room"));
  const otherSender = other.locator("#feed .row", { hasText: "said under the old name" }).locator(".sender");
  await expect(otherSender).toHaveText("ui-said-before");

  await page.locator("#whoami summary").first().click();
  await page.locator("#profileNameInput").fill("ui-said-after");
  await page.locator("#profileRename").click();
  await expect(page.locator("#profileRenameNote")).toHaveText("Renamed to ui-said-after.");

  // The other feed relabels without a reload; ours too, and the name sticks.
  await expect(otherSender).toHaveText("ui-said-after");
  const mine = page.locator("#feed .row", { hasText: "said under the old name" }).locator(".sender");
  await expect(mine).toHaveText("ui-said-after");
  expect(await page.evaluate(() => JSON.parse(localStorage.getItem("loca-admin-session")).name)).toBe("ui-said-after");
  await expect(page.locator("#name")).toHaveValue("ui-said-after");
  await page.reload();
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate(() => joinRoom("rename-room"));
  await expect(mine).toHaveText("ui-said-after");
  await other.close();
});
