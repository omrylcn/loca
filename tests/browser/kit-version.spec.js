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
  // The sidebar Lobby list too.
  await master.evaluate(() => fetchLobby());
  await expect(master.locator("#lobbyList .omem", { hasText: "kv-lobby" }).locator(".kitver")).toHaveText(`kit ${OLD_KIT}`);

  // Gone with the connection.
  agent.socket.close();
  lobby.socket.close();
  await expect.poll(async () => {
    await master.evaluate(() => fetchPeople());
    return master.locator("#peopleList .kitver").count();
  }).toBe(0);
  for (const page of [master, member]) await page.close();
});
