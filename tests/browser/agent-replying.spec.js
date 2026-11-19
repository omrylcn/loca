const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

// An agent's runtime says it is replying (`POST /rooms/:id/typing`, its own
// loca session): the room shows "<agent> is typing…", and the agent's message
// replaces it at once instead of a few seconds later.

const MASTER = { "x-admin-token": "MASTER" };
const ROOM = "agent-replying";

test("an agent that is replying shows as typing until its message arrives", async ({ browser, request }) => {
  await provisionRooms(request, [ROOM]);
  const base = new URL(test.info().project.use.baseURL);

  expect((await request.post("/members", { headers: MASTER, data: { name: "rp-agent", kind: "agent" } })).ok()).toBeTruthy();
  const { token: invite } = await (await request.post(`/rooms/${ROOM}/invites`, { headers: MASTER, data: { name: "rp-agent" } })).json();
  const { session_token: session } = await (await request.post("/sessions", {
    headers: { "x-room-token": invite }, data: { name: "rp-agent", kind: "agent", loca: ROOM },
  })).json();
  const agent = new WebSocket(`ws://${base.host}/ws?room=${ROOM}&name=rp-agent&type=agent`,
    ["loca.v1", `loca.session.${session}`]);
  const seated = new Promise(resolve => agent.addEventListener("message", event => {
    if (JSON.parse(event.data).t === "history") resolve();
  }));
  await seated;

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
  await page.evaluate(room => joinRoom(room), ROOM);
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);

  const said = await request.post(`/rooms/${ROOM}/typing`, {
    headers: { "x-session-token": session }, data: { on: true },
  });
  expect(said.status(), await said.text()).toBe(204);
  await expect(page.locator("#typing")).toHaveText("rp-agent is typing…");

  const posted = await request.post(`/rooms/${ROOM}/messages`, {
    headers: { "x-session-token": session },
    data: { sender: "rp-agent", sender_type: "agent", text: "here is the answer" },
  });
  expect(posted.ok(), await posted.text()).toBeTruthy();
  await expect(page.locator("#feed")).toContainText("here is the answer");
  // Well before the four-second expiry: the reply ended it.
  await expect(page.locator("#typing")).toHaveText("", { timeout: 1500 });

  agent.close();
  await page.close();
});
