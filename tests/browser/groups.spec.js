const { test, expect } = require("@playwright/test");

const MASTER = { "x-admin-token": "MASTER" };

async function rankSession(request, name) {
  if (name === "MASTER") {
    const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
    return (await request.post("/sessions", { headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" } })).json();
  }
  const { token } = await (await request.post("/smasters", { headers: MASTER, data: { name } })).json();
  return (await request.post("/sessions", { headers: { "x-admin-token": token }, data: { name, kind: "user" } })).json();
}

async function openAs(browser, info, name) {
  // Each person in a browser of their own: no shared storage between them.
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.addInitScript(({ token, expiresAt, name }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name }));
  }, { token: info.session_token, expiresAt: info.expires_at, name });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  return page;
}

async function openGroupsTab(page) {
  await page.evaluate(() => switchTab("people"));
  await expect(page.locator("#btabGroups")).toBeVisible();
  await page.locator("#btabGroups").click();
  await expect(page.locator("#groupsPane")).toBeVisible();
}

/// The names of the locas listed right under a group's sidebar header.
async function sidebarGroup(page, group) {
  return page.evaluate((group) => {
    const heads = [...document.querySelectorAll("#roomList .rgrouphead")];
    const head = heads.find(h => h.querySelector(".rgroupname")?.textContent === group);
    if (!head) return null;
    const rooms = [];
    for (let el = head.nextElementSibling; el && el.classList.contains("roomitem"); el = el.nextElementSibling) {
      rooms.push(el.querySelector(".rname")?.textContent);
    }
    return rooms;
  }, group);
}

test("a group and its loca are made from the UI; members see it under the group, an outsider Smaster never", async ({ browser, request }) => {
  const founder = await openAs(browser, await rankSession(request, "grp-founder"), "grp-founder");
  const member = await openAs(browser, await rankSession(request, "grp-member"), "grp-member");
  const outsider = await openAs(browser, await rankSession(request, "grp-outsider"), "grp-outsider");

  // Found the group in the Groups tab.
  await openGroupsTab(founder);
  await founder.locator("#groupName").fill("grp-team");
  await founder.locator("#groupForm button[type=submit]").click();
  const card = founder.locator(".grpcard", { hasText: "grp-team" });
  await expect(card).toBeVisible();
  await expect(card.locator(".grpowner")).toContainText("founded by grp-founder");
  // The founder is its first member and cannot be removed.
  await expect(card.locator(".grpmember", { hasText: "grp-founder" })).toContainText("founder");
  await expect(card.locator(".grpmember", { hasText: "grp-founder" }).locator("[data-group-remove]")).toHaveCount(0);
  // Nobody adds themselves: the founder is never offered in the picker.
  const options = await card.locator("select[data-group-add-select] option").allTextContents();
  expect(options.some(o => o.startsWith("grp-founder"))).toBe(false);

  // Add the member.
  const memberId = await founder.evaluate(() => state.profilesList.find(p => p.display_name === "grp-member").id);
  await card.locator("select[data-group-add-select]").selectOption(memberId);
  await card.locator("[data-group-add]").click();
  await expect(card.locator(".grpmember", { hasText: "grp-member" })).toBeVisible();

  // Open a loca in the group from +.
  await founder.locator("#newRoomName").fill("grp-loca");
  await founder.locator("#newRoomBtn").click();
  await expect(founder.locator("#extCreate")).toBeVisible();
  await expect(founder.locator("#extCreateGroupField")).toBeVisible();
  const groupId = await founder.evaluate(() => state.groups.find(g => g.name === "grp-team").id);
  await founder.locator("#extCreateGroup").selectOption(groupId);
  await expect(founder.locator("#extCreateGroupNote")).toContainText("grp-team");
  await founder.locator("#extCreateSubmit").click();
  await expect(founder.locator("#extCreate")).toBeHidden();
  await expect.poll(() => founder.evaluate(() => state.room)).toBe("grp-loca");
  await founder.evaluate(() => refreshRooms());
  await expect.poll(() => sidebarGroup(founder, "grp-team")).toEqual(["grp-loca"]);

  // A member sees it under the group's header.
  await member.evaluate(() => refreshRooms());
  await expect.poll(() => sidebarGroup(member, "grp-team")).toEqual(["grp-loca"]);
  await member.locator("#roomList .room", { hasText: "grp-loca" }).click();
  await expect.poll(() => member.evaluate(() => state.ws?.readyState)).toBe(1);

  // An outsider Smaster, in another browser, sees nothing at all.
  await outsider.evaluate(() => refreshRooms());
  await expect(outsider.locator("#roomList")).not.toContainText("grp-loca");
  await expect(outsider.locator("#roomList")).not.toContainText("grp-team");
  expect(await outsider.evaluate(() => state.rooms.map(r => r.room))).not.toContain("grp-loca");
  const peek = await outsider.evaluate(async () => (await fetch("/rooms/grp-loca/messages", { headers: adminHeaders({}) })).status);
  expect(peek).toBe(404);
  await openGroupsTab(outsider);
  await expect(outsider.locator("#groupsPane")).not.toContainText("grp-team");

  await founder.context().close();
  await member.context().close();
  await outsider.context().close();
});

test("the Groups tab manages members, and the People tab shows and filters by group", async ({ browser, request }) => {
  const founder = await openAs(browser, await rankSession(request, "grp-owner"), "grp-owner");
  const temp = await openAs(browser, await rankSession(request, "grp-temp"), "grp-temp");
  const created = await founder.evaluate(async () => {
    const r = await fetch("/groups", { method: "POST", headers: adminHeaders({ "content-type": "application/json" }), body: JSON.stringify({ name: "grp-ops" }) });
    return r.json();
  });
  const room = await founder.evaluate(async (group) => {
    const r = await fetch("/rooms", { method: "POST", headers: adminHeaders({ "content-type": "application/json" }), body: JSON.stringify({ name: "grp-ops-room", group }) });
    return r.status;
  }, created.id);
  expect(room).toBe(201);

  await openGroupsTab(founder);
  const card = founder.locator(".grpcard", { hasText: "grp-ops" });
  await expect(card.locator(".grplocas")).toContainText("grp-ops-room");
  const tempId = await founder.evaluate(() => state.profilesList.find(p => p.display_name === "grp-temp").id);
  await card.locator("select[data-group-add-select]").selectOption(tempId);
  await card.locator("[data-group-add]").click();
  await expect(card.locator(".grpmember", { hasText: "grp-temp" })).toBeVisible();
  await temp.evaluate(() => refreshRooms());
  await expect.poll(() => sidebarGroup(temp, "grp-ops")).toEqual(["grp-ops-room"]);

  // The People tab: a group column, a filter, and a person's card.
  await founder.locator("#btabPeople").click();
  const row = founder.locator("#peopleList .prow", { hasText: "grp-temp" });
  await expect(row.locator(".pgroups")).toContainText("grp-ops");
  await founder.locator("#peopleList [data-group-filter]", { hasText: "grp-ops" }).click();
  await expect(founder.locator("#peopleList .prow", { hasText: "grp-temp" })).toBeVisible();
  await expect(founder.locator("#peopleList .prow.rankrow", { hasText: "grp-founder" })).toHaveCount(0);
  await founder.locator("#peopleList [data-group-filter=all]").click();
  await row.locator("[data-person-card]").click();
  const personCard = founder.locator("#personCard");
  await expect(personCard).toBeVisible();
  await expect(personCard.locator("#personCardBody")).toContainText("grp-ops");
  await personCard.locator("#personCardClose").click();
  await expect(personCard).toBeHidden();

  // Remove from the Groups tab: the loca leaves their sidebar.
  await founder.locator("#btabGroups").click();
  founder.once("dialog", dialog => dialog.accept());
  await card.locator(".grpmember", { hasText: "grp-temp" }).locator("[data-group-remove]").click();
  await expect(card.locator(".grpmember", { hasText: "grp-temp" })).toHaveCount(0);
  await temp.evaluate(() => refreshRooms());
  await expect.poll(() => temp.evaluate(() => state.rooms.map(r => r.room))).not.toContain("grp-ops-room");
  await expect(temp.locator("#roomList")).not.toContainText("grp-ops");

  await founder.context().close();
  await temp.context().close();
});

test("the Master moves a default loca into a group from This Loca, after seeing who loses access", async ({ browser, request }) => {
  const MASTER = { "x-admin-token": "MASTER" };
  const { provisionRooms } = require("./support/building");
  await provisionRooms(request, ["gm-room"]);
  await request.post("/members", { headers: MASTER, data: { name: "gm-outsider", kind: "agent" } });
  await request.post("/rooms/gm-room/invites", { headers: MASTER, data: { name: "gm-outsider" } });
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const m = await (await request.post("/sessions", { headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" } })).json();
  const group = await (await request.post("/groups", { headers: { "x-session-token": m.session_token }, data: { name: "gm-backend" } })).json();
  const page = await browser.newPage({ viewport: { width: 1280, height: 860 } });
  await page.addInitScript(({ t, e }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token: t, expiresAt: e, name: "Master" }));
  }, { t: m.session_token, e: m.expires_at });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate(() => joinRoom("gm-room"));
  await page.evaluate(() => setSidebarView("loca"));
  const summary = page.locator("#locaSummary");
  await expect(summary).toContainText("Default");
  await summary.locator("[data-loca-group-change]").click();
  await summary.locator("[data-loca-group-target]").selectOption(group.id);
  await expect(summary.locator(".groupmove")).toContainText("1 will lose access:");
  await expect(summary.locator(".groupmove")).toContainText("gm-outsider");
  await page.screenshot({ path: test.info().outputPath("group-move-1280.png") });
  // Cancel changes nothing.
  await summary.locator("[data-loca-group-cancel]").click();
  await expect(summary).toContainText("Default");
  // Move for real.
  await summary.locator("[data-loca-group-change]").click();
  await summary.locator("[data-loca-group-target]").selectOption(group.id);
  await expect(summary.locator("[data-loca-group-confirm]")).toBeEnabled();
  await summary.locator("[data-loca-group-confirm]").click();
  await expect(summary).toContainText("gm-backend");
  await page.evaluate(() => setSidebarView("building"));
  await expect(page.locator("#roomList")).toContainText("gm-backend");
  const outsider = await (await request.get("/rooms/gm-room/invites", { headers: MASTER })).json();
  expect(outsider.map(i => i.name)).not.toContain("gm-outsider");
  await page.close();
});
