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
