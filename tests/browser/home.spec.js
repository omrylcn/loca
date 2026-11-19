const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

// Home (the Home and HomeLoca boards): the overview a person opens from the
// top of the sidebar. It is not a room: it opens no socket and has no
// composer, the loca that was open stays connected behind it, and leaving
// Home goes back there. Sign-in never lands on it (member-landing.spec.js
// keeps where a sign-in lands).

const MASTER = { "x-admin-token": "MASTER" };

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
});

async function admit(request, name, kind = "user") {
  const admitted = await request.post("/members", { headers: MASTER, data: { name, kind } });
  expect(admitted.ok(), await admitted.text()).toBeTruthy();
  return (await admitted.json()).token;
}

async function invite(request, room, name) {
  const invited = await request.post(`/rooms/${room}/invites`, { headers: MASTER, data: { name } });
  expect(invited.ok(), await invited.text()).toBeTruthy();
  return (await invited.json()).token;
}

test("Home shows each loca with what is new and who named you, and leaves back to the loca that was open", async ({ page, request }) => {
  const tag = Date.now().toString(36);
  const rooms = [`home-a-${tag}`, `home-b-${tag}`];
  const member = `home-ana-${tag}`, agent = `home-bot-${tag}`;
  await provisionRooms(request, rooms);
  const key = await admit(request, member);
  for (const room of rooms) await invite(request, room, member);
  // An agent seated in the second loca, set up before the member looks, so
  // that only what it says afterwards is new.
  await admit(request, agent, "agent");
  const agentInvite = await invite(request, rooms[1], agent);
  const session = await (await request.post("/sessions", {
    headers: { "x-room-token": agentInvite }, data: { name: agent, kind: "agent" },
  })).json();

  await page.goto("/");
  await page.locator("#entryKind").selectOption("human");
  await page.locator("#name").fill(member);
  await page.locator("#roomToken").fill(key);
  await page.locator("#connectBtn").click();
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  // Sign-in lands in a loca, never on Home.
  expect(rooms).toContain(await page.evaluate(() => state.room));
  await expect(page.locator("#homePanel")).toBeHidden();
  await expect(page.locator("#homeLink")).toBeVisible();
  // Sit in the first loca; the agent speaks in the other.
  const [landed, other] = rooms;
  await page.evaluate(room => joinRoom(room), landed);
  await expect.poll(() => page.evaluate(() => state.room)).toBe(landed);
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  // Both locas have been seen once, so what is said next is new.
  await expect.poll(() => page.evaluate(room => room in state.readCursors, other)).toBe(true);

  // The agent names the member in the other loca.
  const said = await request.post(`/rooms/${other}/messages`, {
    headers: { "x-session-token": session.session_token },
    data: { sender: agent, sender_type: "agent", text: `@${member} the plan is ready` },
  });
  expect(said.ok(), await said.text()).toBeTruthy();
  await expect.poll(() => page.evaluate(room => state.unread[room], other), { timeout: 20_000 }).toBe(1);

  // Home opens over the loca: no new socket, no composer, the loca still open.
  await page.evaluate(() => { window.__homeSocket = state.ws; });
  await page.locator("#homeLink").click();
  await expect(page.locator("#homePanel")).toBeVisible();
  await expect(page.locator("#homeLink")).toHaveClass(/active/);
  await expect(page.locator("#curRoom")).toHaveText("Home");
  await expect(page.locator("#composer")).toBeHidden();
  await expect(page.locator("#feed")).toBeHidden();
  await expect(page.locator(".main")).toHaveClass(/global/);
  expect(await page.evaluate(() => state.tab)).toBe("home");
  expect(await page.evaluate(() => state.ws === window.__homeSocket && state.ws.readyState)).toBe(1);
  expect(await page.evaluate(() => state.room)).toBe(landed);
  await expect(page.locator("#homeGreeting")).toContainText(member);

  // The cards: the loca that was open first, the other with its news.
  await expect(page.locator("#homeLocas .homecard").first()).toHaveAttribute("data-home-room", landed);
  await expect(page.locator(`#homeLocas [data-home-room="${landed}"] .homekicker`)).toHaveText("Continue");
  const card = page.locator(`#homeLocas [data-home-room="${other}"]`);
  await expect(card.locator(".homenew")).toHaveText("1 new");
  await expect(card.locator(".homementioned")).toHaveText("@ you");
  await expect(card.locator(".homelast")).toContainText("the plan is ready");
  await expect(page.locator(`#homeUnreadList [data-home-room="${other}"]`)).toContainText("names you");
  await expect(page.locator("#homeSub")).toContainText("1 new message");

  // Home again goes back to the loca that was open, as it was.
  await page.locator("#homeLink").click();
  await expect(page.locator("#homePanel")).toBeHidden();
  await expect(page.locator("#composer")).toBeVisible();
  expect(await page.evaluate(() => [state.tab, state.room])).toEqual(["chat", landed]);
  expect(await page.evaluate(() => state.ws === window.__homeSocket)).toBe(true);

  // A card opens its loca, in its chat.
  await page.locator("#homeLink").click();
  await page.locator(`#homeLocas [data-home-room="${other}"]`).click();
  await expect.poll(() => page.evaluate(() => state.room)).toBe(other);
  await expect(page.locator("#homePanel")).toBeHidden();
  await expect(page.locator("#feed")).toContainText("the plan is ready");
  expect(await page.evaluate(() => state.tab)).toBe("chat");
  await expect.poll(() => page.evaluate(room => state.unread[room], other)).toBe(0);

  // A reload lands in the loca, not on Home.
  await page.reload();
  await expect.poll(() => page.evaluate(() => state.room)).toBe(other);
  await expect(page.locator("#homePanel")).toBeHidden();
  expect(await page.evaluate(() => state.tab)).toBe("chat");
});

test("the Building view still opens beside Home, and leaving Home returns to it", async ({ page, request }) => {
  // A Master session in the browser, through the real pairing door, in Care.
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  await page.goto("/");
  await page.locator("#masterEntryButton").click();
  await page.locator("#pairingCode").fill(pairing_code);
  await page.evaluate(() => window.doConnect("iye"));
  await expect(page.locator("#whoami")).toContainText("MASTER");
  await expect.poll(() => page.evaluate(() => state.room)).toBe("iye");
  await expect(page.locator("#tabPeople")).toBeVisible();

  await page.locator("#homeLink").click();
  await expect(page.locator("#homePanel")).toBeVisible();
  await expect(page.locator(`#homeLocas [data-home-room="iye"] .homekicker`)).toHaveText("Continue");

  await page.locator("#tabPeople").click();
  await expect(page.locator("#peoplePanel")).toBeVisible();
  await expect(page.locator("#homePanel")).toBeHidden();
  await expect(page.locator("#curRoom")).toHaveText("Building");
  await expect(page.locator("#homeLink")).not.toHaveClass(/active/);

  await page.locator("#homeLink").click();
  await expect(page.locator("#homePanel")).toBeVisible();
  await page.locator("#homeLink").click();
  await expect(page.locator("#peoplePanel")).toBeVisible();
  expect(await page.evaluate(() => [state.tab, state.room])).toEqual(["people", "iye"]);
});
