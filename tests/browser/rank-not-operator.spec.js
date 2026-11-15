const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

// Operator is a seat a person is appointed to in a loca; the Master holds the
// operator's authority everywhere by rank, never by a silent appointment
// (PRINCIPLES). The page shows the rank where the authority comes from rank,
// and "operator" only for the person appointed to that seat here.

const MASTER = { "x-admin-token": "MASTER" };

async function rankSession(request, name) {
  if (name === "MASTER") {
    const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
    return (await request.post("/sessions", { headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" } })).json();
  }
  const { token } = await (await request.post("/smasters", { headers: MASTER, data: { name } })).json();
  return (await request.post("/sessions", { headers: { "x-admin-token": token }, data: { name, kind: "user" } })).json();
}

// A rank's page, seated (not watching) in `room`.
async function rankPage(browser, request, who, room) {
  const info = await rankSession(request, who);
  const page = await browser.newPage();
  await page.addInitScript(({ token, expiresAt, room }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "" }));
    localStorage.setItem("loca-seated-rooms", JSON.stringify([room]));
  }, { token: info.session_token, expiresAt: info.expires_at, room });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await enter(page, room);
  return page;
}

// A member appointed Operator of `room`.
async function appointedOperatorPage(browser, request, name, room) {
  const admitted = await request.post("/members", { headers: MASTER, data: { name, kind: "user" } });
  const { token } = await admitted.json();
  const invited = await request.post(`/rooms/${room}/invites`, { headers: MASTER, data: { name } });
  expect(invited.ok(), await invited.text()).toBeTruthy();
  const profile = await (await request.get("/profile", { headers: { "x-admin-token": token } })).json();
  const appointed = await request.post(`/rooms/${room}/operators`, {
    headers: MASTER, data: { principal_id: profile.principal.id },
  });
  expect(appointed.ok(), await appointed.text()).toBeTruthy();
  const page = await browser.newPage();
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await page.goto("/");
  await page.locator("#entryKind").selectOption("human");
  await page.locator("#name").fill(name);
  await page.locator("#roomToken").fill(token);
  await page.locator("#connectBtn").click();
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await enter(page, room);
  return page;
}

async function enter(page, room) {
  await page.evaluate(room => joinRoom(room), room);
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
}

async function say(page, text) {
  await page.fill("#msg", text);
  await page.keyboard.press("Enter");
  await expect(page.locator("#feed .row", { hasText: text })).toHaveCount(1);
}

function line(page, text) { return page.locator("#feed .row", { hasText: text }).locator(".roletags"); }

test("chat lines wear the writer's rank; only the appointed Operator wears operator", async ({ browser, request }) => {
  await provisionRooms(request, ["rank-tags"]);
  // The Master founded it, so the Master is its appointed Operator until the
  // seat is given to someone else.
  const operator = await appointedOperatorPage(browser, request, "rank-tags-op", "rank-tags");
  const master = await rankPage(browser, request, "MASTER", "rank-tags");
  const smaster = await rankPage(browser, request, "rank-tags-smaster", "rank-tags");

  await say(master, "a line by the Master");
  await say(smaster, "a line by the Smaster");
  await say(operator, "a line by the Operator");

  for (const viewer of [operator, master, smaster]) {
    await expect(line(viewer, "a line by the Master").locator(".optag")).toHaveCount(0);
    await expect(line(viewer, "a line by the Master").locator(".rankbadge")).toHaveText("Master");
    await expect(line(viewer, "a line by the Smaster").locator(".optag")).toHaveCount(0);
    await expect(line(viewer, "a line by the Smaster").locator(".rankbadge")).toHaveText("Smaster");
    await expect(line(viewer, "a line by the Operator").locator(".optag")).toHaveText("operator");
    await expect(line(viewer, "a line by the Operator").locator(".rankbadge")).toHaveCount(0);
  }
  for (const page of [operator, master, smaster]) await page.close();
});

test("a Master the server reports as the appointed Operator wears both its rank and operator", async ({ browser, request }) => {
  await provisionRooms(request, ["rank-tags-founded"]);
  const master = await rankPage(browser, request, "MASTER", "rank-tags-founded");
  await expect.poll(() => master.evaluate(() => state.profile?.loca?.operator_source)).toBe("appointed");
  await say(master, "the founder speaks");
  await expect(line(master, "the founder speaks").locator(".rankbadge")).toHaveText("Master");
  await expect(line(master, "the founder speaks").locator(".optag")).toHaveText("operator");
  await master.close();
});

test("the Master's own role in a loca it was not appointed to reads MASTER, not OPERATOR", async ({ browser, request }) => {
  await provisionRooms(request, ["rank-role"]);
  const operator = await appointedOperatorPage(browser, request, "rank-role-op", "rank-role");
  const master = await rankPage(browser, request, "MASTER", "rank-role");
  await expect.poll(() => master.evaluate(() => state.profile?.loca?.operator_source)).toBe("master");

  // This Loca: the Operator seat names the appointed person; the Master's own
  // role is its rank.
  await master.evaluate(() => setSidebarView("loca"));
  const summary = master.locator("#locaSummary");
  await expect(summary.locator("dt", { hasText: "Operator" }).locator("xpath=..")).toContainText("rank-role-op");
  await expect(summary.locator("dt", { hasText: "Your role" }).locator("xpath=../dd")).toHaveText("MASTER");
  // The account menu: the same.
  const loca = master.locator("#whoami .profileroles span", { hasText: "This Loca" });
  await expect(loca.locator(".loca-role")).toHaveText("MASTER");
  await expect(loca).not.toContainText(/operator/i);

  // The appointed Operator still reads OPERATOR, appointed.
  await operator.evaluate(() => setSidebarView("loca"));
  await expect(operator.locator("#locaSummary dt", { hasText: "Your role" }).locator("xpath=../dd")).toHaveText("OPERATOR");
  const opLoca = operator.locator("#whoami .profileroles span", { hasText: "This Loca" });
  await expect(opLoca.locator(".loca-role")).toHaveText("OPERATOR");
  await expect(opLoca).toContainText("appointed");
  for (const page of [operator, master]) await page.close();
});

// A rank belongs to a person, not to a name. A line keeps the writer the
// server recorded; if someone else wears that name later (the Master, a
// Smaster in the roster, the appointed Operator), the old line must not gain
// their title.
test("a line does not gain the rank or seat of someone else who now wears its writer's name", async ({ browser, request }) => {
  await provisionRooms(request, ["rank-name-reuse"]);
  const operator = await appointedOperatorPage(browser, request, "rank-reuse-op", "rank-name-reuse");
  const admitted = await request.post("/members", { headers: MASTER, data: { name: "rank-reuse-plain", kind: "user" } });
  const { token } = await admitted.json();
  const invited = await request.post("/rooms/rank-name-reuse/invites", { headers: MASTER, data: { name: "rank-reuse-plain" } });
  expect(invited.ok(), await invited.text()).toBeTruthy();
  const plain = await browser.newPage();
  await plain.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await plain.goto("/");
  await plain.locator("#entryKind").selectOption("human");
  await plain.locator("#name").fill("rank-reuse-plain");
  await plain.locator("#roomToken").fill(token);
  await plain.locator("#connectBtn").click();
  await expect.poll(() => plain.evaluate(() => state.authStatus)).toBe("authenticated");
  await enter(plain, "rank-name-reuse");
  await say(plain, "a line by a plain member");

  const tags = line(operator, "a line by a plain member");
  await expect(tags).toHaveAttribute("data-writer", /^pr_/);
  await expect(tags.locator(".rankbadge, .optag")).toHaveCount(0);
  // The same name, another principal: as the Master, as a Smaster seated
  // here, and as the appointed Operator.
  for (const wornBy of ["master", "smaster", "operator"]) {
    await operator.evaluate(wornBy => {
      const other = "pr_someone_else";
      const op = JSON.parse(JSON.stringify(state.locaOperator));
      if (wornBy === "master") op.building_master = { ...op.building_master, principal_id: other, display_name: "rank-reuse-plain" };
      if (wornBy === "operator") op.appointed = { ...op.appointed, principal_id: other, display_name: "rank-reuse-plain" };
      state.locaOperator = op;
      if (wornBy === "smaster") state.members = [...state.members.filter(m => m.name !== "rank-reuse-plain"),
        { name: "rank-reuse-plain", type: "user", role: "smaster", principal_id: other }];
      refreshSenderTags();
    }, wornBy);
    await expect(tags.locator(".rankbadge, .optag"), `name now worn by the ${wornBy}`).toHaveCount(0);
  }

  // A line the server recorded no writer for wears no rank and no seat,
  // whoever's name it carries: here the real Master's and the real appointed
  // Operator's names, drawn by the page's own addMsg.
  // (A fresh load first: the state above was redrawn by hand.)
  await operator.reload();
  await expect.poll(() => operator.evaluate(() => state.authStatus)).toBe("authenticated");
  await enter(operator, "rank-name-reuse");
  await expect.poll(() => operator.evaluate(() => state.locaOperator?.appointed?.display_name)).toBe("rank-reuse-op");
  const real = await operator.evaluate(() => ({
    master: state.locaOperator.building_master.display_name, operator: state.locaOperator.appointed.display_name,
  }));
  await operator.evaluate(({ master, operator }) => {
    const base = { room: state.room, sender_type: "user", ts: Date.now() };
    addMsg({ ...base, id: 990000001, sender: master, text: "unverified line under the Master's name" });
    addMsg({ ...base, id: 990000002, sender: operator, text: "unverified line under the Operator's name" });
    refreshSenderTags();
  }, real);
  for (const text of ["unverified line under the Master's name", "unverified line under the Operator's name"]) {
    await expect(operator.locator("#feed .row", { hasText: text })).toHaveCount(1);
    await expect(line(operator, text).locator(".rankbadge, .optag"), text).toHaveCount(0);
  }
  for (const page of [operator, plain]) await page.close();
});
