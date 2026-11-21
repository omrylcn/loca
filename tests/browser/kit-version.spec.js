const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

// The kit version a connected agent sent (`kit_version`, docs/protocol.md
// "Kit version") is shown to the Master, a Smaster and the loca's Operator
// as "kit X", with a plain mark when older than this server; a browser seat
// shows nothing, and an ordinary member sees no new noise.

const MASTER = { "x-admin-token": "MASTER" };
const ROOM = "kit-version";
// Older than any server this suite runs.
const OLD_KIT = "0.1.0";

async function masterPage(browser, request) {
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const info = await (await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" },
  })).json();
  const page = await browser.newPage();
  await page.addInitScript(({ token, expiresAt, room }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "" }));
    localStorage.setItem("loca-seated-rooms", JSON.stringify([room]));
  }, { token: info.session_token, expiresAt: info.expires_at, room: ROOM });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await enter(page);
  return page;
}

async function memberPage(browser, request, name) {
  const { token } = await (await request.post("/members", { headers: MASTER, data: { name, kind: "user" } })).json();
  const invited = await request.post(`/rooms/${ROOM}/invites`, { headers: MASTER, data: { name } });
  expect(invited.ok(), await invited.text()).toBeTruthy();
  const page = await browser.newPage();
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await page.goto("/");
  await page.locator("#entryKind").selectOption("human");
  await page.locator("#name").fill(name);
  await page.locator("#roomToken").fill(token);
  await page.locator("#connectBtn").click();
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await enter(page);
  return page;
}

async function enter(page) {
  await page.evaluate(room => joinRoom(room), ROOM);
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
}

// An agent runtime the way the kit connects: its own session, a loca socket
// (or the Lobby socket) with `kit_version` in the query.
function open(url, protocols) {
  const socket = new WebSocket(url, protocols);
  const frames = [];
  socket.addEventListener("message", event => frames.push(JSON.parse(event.data).t));
  return { socket, frames };
}

test("the Master sees an agent's older kit; a browser seat and a member see nothing new", async ({ browser, request }) => {
  await provisionRooms(request, [ROOM]);
  const health = await (await request.get("/health")).json();
  const base = new URL(test.info().project.use.baseURL);

  // A seated agent that sent an older kit version.
  expect((await request.post("/members", { headers: MASTER, data: { name: "kv-agent", kind: "agent" } })).ok()).toBeTruthy();
  const { token: invite } = await (await request.post(`/rooms/${ROOM}/invites`, { headers: MASTER, data: { name: "kv-agent" } })).json();
  const session = await (await request.post("/sessions", {
    headers: { "x-room-token": invite }, data: { name: "kv-agent", kind: "agent", loca: ROOM },
  })).json();
  const agent = open(`ws://${base.host}/ws?room=${ROOM}&name=kv-agent&type=agent&kit_version=${OLD_KIT}`,
    ["loca.v1", `loca.session.${session.session_token}`]);
  await expect.poll(() => agent.frames.includes("history")).toBe(true);
  // An agent waiting in the Lobby, seated nowhere, with the same kit.
  const { token: lobbyKey } = await (await request.post("/members", { headers: MASTER, data: { name: "kv-lobby", kind: "agent" } })).json();
  const lobby = open(`ws://${base.host}/lobby/ws?kit_version=${OLD_KIT}`, ["loca.v1", `loca.membership.${lobbyKey}`]);
  await expect.poll(() => lobby.frames.includes("lobby_ready")).toBe(true);

  const master = await masterPage(browser, request);
  const member = await memberPage(browser, request, "kv-person");
  const seat = (page, name) => page.locator("#onlineList .omem", { hasText: name });

  // The roster: the agent's seat wears the version and the mark.
  await expect(seat(master, "kv-agent").locator(".kitver")).toHaveText(`kit ${OLD_KIT}`);
  await expect(seat(master, "kv-agent").locator(".kitold")).toHaveText(`older than this server (${health.version})`);
  // A browser seat carries no version.
  await expect(seat(master, "kv-person")).toHaveCount(1);
  await expect(seat(master, "kv-person").locator(".kitver, .kitold")).toHaveCount(0);
  // An ordinary member sees the agent seated, without the version.
  await expect(seat(member, "kv-agent")).toHaveCount(1);
  await expect(member.locator("#onlineList .kitver, #onlineList .kitold")).toHaveCount(0);

  // The Building list: the seated agent and the one waiting in the Lobby.
  await master.evaluate(() => switchTab("people"));
  for (const name of ["kv-agent", "kv-lobby"]) {
    const row = master.locator("#peopleList .prow", { hasText: name });
    await expect(row.locator(".kitver")).toHaveText(`kit ${OLD_KIT}`);
    await expect(row.locator(".kitold")).toHaveText(`older than this server (${health.version})`);
  }
  await expect(master.locator("#peopleList .prow", { hasText: "kv-person" }).locator(".kitver")).toHaveCount(0);

  // Gone with the connection.
  agent.socket.close();
  lobby.socket.close();
  await expect.poll(async () => {
    await master.evaluate(() => fetchPeople());
    return master.locator("#peopleList .kitver").count();
  }).toBe(0);
  for (const page of [master, member]) await page.close();
});

// The two other viewers the rule names: the loca's Operator sees the version
// on that loca's roster (not the Building list, which an Operator does not
// read), and a Smaster sees it on both, like the Master.
test("the loca's Operator sees an agent's kit on the roster; a Smaster on the roster and the Building list", async ({ browser, request }) => {
  const room = "kit-version-viewers";
  await provisionRooms(request, [room]);
  const base = new URL(test.info().project.use.baseURL);
  expect((await request.post("/members", { headers: MASTER, data: { name: "kvv-agent", kind: "agent" } })).ok()).toBeTruthy();
  const { token: invite } = await (await request.post(`/rooms/${room}/invites`, { headers: MASTER, data: { name: "kvv-agent" } })).json();
  const session = await (await request.post("/sessions", {
    headers: { "x-room-token": invite }, data: { name: "kvv-agent", kind: "agent", loca: room },
  })).json();
  const agent = open(`ws://${base.host}/ws?room=${room}&name=kvv-agent&type=agent&kit_version=${OLD_KIT}`,
    ["loca.v1", `loca.session.${session.session_token}`]);
  await expect.poll(() => agent.frames.includes("history")).toBe(true);

  const join = async (page) => {
    await page.evaluate(room => joinRoom(room), room);
    await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  };
  const seat = (page) => page.locator("#onlineList .omem", { hasText: "kvv-agent" });

  // The Operator: a human member of this loca, appointed.
  const { token: key } = await (await request.post("/members", { headers: MASTER, data: { name: "kvv-op", kind: "user" } })).json();
  const { token: opInvite } = await (await request.post(`/rooms/${room}/invites`, { headers: MASTER, data: { name: "kvv-op" } })).json();
  const opSession = await (await request.post("/sessions", { headers: { "x-room-token": opInvite }, data: { name: "kvv-op", kind: "user" } })).json();
  const opProfile = await (await request.get(`/profile?room=${room}`, { headers: { "x-session-token": opSession.session_token } })).json();
  const appointed = await request.post(`/rooms/${room}/operators`, { headers: MASTER, data: { principal_id: opProfile.principal.id } });
  expect(appointed.ok(), await appointed.text()).toBeTruthy();
  const operator = await browser.newPage();
  await operator.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await operator.goto("/");
  await operator.locator("#entryKind").selectOption("human");
  await operator.locator("#name").fill("kvv-op");
  await operator.locator("#roomToken").fill(key);
  await operator.locator("#connectBtn").click();
  await expect.poll(() => operator.evaluate(() => state.authStatus)).toBe("authenticated");
  await join(operator);
  await expect.poll(() => operator.evaluate(() => isLocaOperator() && !isAdmin())).toBe(true);
  await expect(seat(operator).locator(".kitver")).toHaveText(`kit ${OLD_KIT}`);
  await expect(seat(operator).locator(".kitold")).toHaveCount(1);

  // The Smaster.
  const { token: deputyKey } = await (await request.post("/smasters", { headers: MASTER, data: { name: "kvv-deputy" } })).json();
  const deputy = await (await request.post("/sessions", {
    headers: { "x-admin-token": deputyKey }, data: { name: "kvv-deputy", kind: "user" },
  })).json();
  const smaster = await browser.newPage();
  await smaster.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "kvv-deputy" }));
  }, { token: deputy.session_token, expiresAt: deputy.expires_at });
  await smaster.goto("/");
  await expect.poll(() => smaster.evaluate(() => state.profile?.building_role)).toBe("smaster");
  await join(smaster);
  await expect(seat(smaster).locator(".kitver")).toHaveText(`kit ${OLD_KIT}`);
  await smaster.evaluate(() => switchTab("people"));
  await expect(smaster.locator("#peopleList .prow", { hasText: "kvv-agent" }).locator(".kitver")).toHaveText(`kit ${OLD_KIT}`);

  agent.socket.close();
  for (const page of [operator, smaster]) await page.close();
});
