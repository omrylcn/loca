const { test, expect } = require("@playwright/test");
const { MASTER, settle, openSignInForm, signOut } = require("./support/switch");
const { provisionRooms } = require("./support/building");

// One identity, one seat (PRINCIPLES: a new connection with the same identity
// takes over the old seat, last writer wins). Two browser connections of one
// human under one name share the seat; the server evicts when the same
// identity arrives under another name or kind. In practice that is a key of
// kind "agent" open in two tabs, or in a tab while the agent's own runtime
// holds it. The older connection receives `evicted` and is closed. The tab
// steps down, says why, and comes back only when asked: reconnecting by itself
// would take the seat back, and the two would take it from each other in turn.

async function door(browser, route) {
  const context = await browser.newContext();
  if (route) await context.routeWebSocket(/\/ws\?/, route);
  const page = await context.newPage();
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  const errors = [];
  page.on("pageerror", error => errors.push(String(error)));
  const sockets = [];   // every loca socket the page opens
  page.on("websocket", ws => { if (new URL(ws.url()).pathname === "/ws") sockets.push(ws.url()); });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("unauthenticated");
  return { context, page, errors, sockets };
}

async function signIn(page, name, key, room) {
  await openSignInForm(page);
  await page.locator("#name").fill(name);
  await page.locator("#roomToken").fill(key);
  await page.evaluate(r => { doConnect(r); }, room);
  await expect(page.locator("body")).not.toHaveClass(/locked/);
  await expect.poll(() => page.evaluate(r => state.room === r && !!state.session && state.ws && state.ws.readyState, room), { timeout: 15_000 }).toBe(1);
}

async function invite(request, name, kind, room) {
  expect((await request.post("/members", { headers: MASTER, data: { name, kind } })).ok()).toBeTruthy();
  const invited = await request.post(`/rooms/${room}/invites`, { headers: MASTER, data: { name, kind } });
  expect(invited.ok()).toBeTruthy();
  return (await invited.json()).token;
}

const open = page => page.evaluate(() => !!state.ws && state.ws.readyState === 1);
const notice = name => `Another tab or device is now in this loca under this identity, as ${name}. `
  + "This tab stepped out so the two do not keep taking the seat from each other.";

async function expectSteppedOut(page, name) {
  await expect(page.locator("#evictedBar")).toBeVisible({ timeout: 10_000 });
  await expect(page.locator("#evictedText")).toHaveText(notice(name));
  await expect(page.locator("#subStatus")).toHaveText("open in another tab");
  expect(await open(page)).toBe(false);
}

test("an agent key open in two tabs: the older tab steps out and says so, the newer keeps its seat", async ({ browser, request }) => {
  test.setTimeout(90_000);
  const room = "evict-two-tabs";
  const name = "evict1-agent";
  await provisionRooms(request, [room]);
  const key = await invite(request, name, "agent", room);
  const first = await door(browser);
  await signIn(first.page, name, key, room);
  const second = await door(browser);
  await signIn(second.page, name, key, room);
  const firstBefore = first.sockets.length;
  const secondBefore = second.sockets.length;

  // Long enough for several of the old 1.5 s reconnects.
  await settle(second.page, 12_000);
  expect(first.sockets.length - firstBefore).toBe(0);
  expect(second.sockets.length - secondBefore).toBe(0);
  expect(await open(second.page)).toBe(true);
  await expect(second.page.locator("#evictedBar")).toBeHidden();
  await expectSteppedOut(first.page, name);
  expect(first.errors).toEqual([]);
  expect(second.errors).toEqual([]);
  await first.context.close();
  await second.context.close();
});

test("an agent runtime holding the same key keeps its seat; the tab steps out", async ({ browser, request }) => {
  test.setTimeout(90_000);
  const room = "evict-runtime";
  const name = "evict2-agent";
  await provisionRooms(request, [room]);
  const key = await invite(request, name, "agent", room);
  const tab = await door(browser);
  await signIn(tab.page, name, key, room);
  const before = tab.sockets.length;

  // The agent's runtime: a session taken with the same key, a loca socket.
  const session = await (await request.post("/sessions", { headers: { "x-room-token": key }, data: { name, kind: "agent" } })).json();
  const base = new URL(test.info().project.use.baseURL);
  const frames = [];
  const runtime = new WebSocket(`ws://${base.host}/ws?room=${room}&name=${name}&type=agent`, ["loca.v1", `loca.session.${session.session_token}`]);
  const closed = new Promise(resolve => runtime.addEventListener("close", resolve));
  runtime.addEventListener("message", event => frames.push(JSON.parse(event.data).t));
  await expect.poll(() => frames.includes("history")).toBe(true);

  // Long enough for several of the old 1.5 s reconnects.
  await settle(tab.page, 10_000);
  expect(frames).not.toContain("evicted");
  expect(runtime.readyState).toBe(WebSocket.OPEN);
  expect(tab.sockets.length - before).toBe(0);
  await expectSteppedOut(tab.page, name);
  runtime.close();
  await closed;
  expect(tab.errors).toEqual([]);
  await tab.context.close();
});

test("'Use it here' brings the first tab back and the other steps out, once, without a loop", async ({ browser, request }) => {
  test.setTimeout(90_000);
  const room = "evict-use-here";
  const name = "evict3-agent";
  await provisionRooms(request, [room]);
  const key = await invite(request, name, "agent", room);
  const first = await door(browser);
  await signIn(first.page, name, key, room);
  const second = await door(browser);
  await signIn(second.page, name, key, room);
  await expectSteppedOut(first.page, name);
  const firstBefore = first.sockets.length;
  const secondBefore = second.sockets.length;

  await first.page.locator("#evictedUse").click();
  await expect.poll(() => open(first.page), { timeout: 10_000 }).toBe(true);
  await expect(first.page.locator("#evictedBar")).toBeHidden();
  await expectSteppedOut(second.page, name);

  await settle(first.page, 10_000);
  expect(first.sockets.length - firstBefore).toBe(1);
  expect(second.sockets.length - secondBefore).toBe(0);
  expect(await open(first.page)).toBe(true);
  await expect(first.page.locator("#evictedBar")).toBeHidden();
  await expect(first.page.locator("#subStatus")).not.toHaveText("open in another tab");
  await expectSteppedOut(second.page, name);
  expect(first.errors).toEqual([]);
  expect(second.errors).toEqual([]);
  await first.context.close();
  await second.context.close();
});

test("one person in two tabs shares the seat, and a lost connection still reconnects by itself", async ({ browser, request }) => {
  test.setTimeout(90_000);
  const room = "evict-lost";
  const name = "evict4-person";
  await provisionRooms(request, [room]);
  const key = await invite(request, name, "user", room);
  // Holds the page side of every loca socket, so the test can drop one as a
  // network would, without the server evicting anything.
  const routes = [];
  const first = await door(browser, ws => {
    const server = ws.connectToServer();
    routes.push(ws);
    ws.onMessage(message => server.send(message));
    server.onMessage(message => ws.send(message));
  });
  await signIn(first.page, name, key, room);
  const second = await door(browser);
  await signIn(second.page, name, key, room);
  await settle(second.page, 4000);
  expect(await open(first.page)).toBe(true);
  expect(await open(second.page)).toBe(true);
  const before = first.sockets.length;

  await routes[routes.length - 1].close({ code: 1011, reason: "gone" });
  await expect.poll(() => first.sockets.length - before, { timeout: 10_000 }).toBe(1);
  await expect.poll(() => open(first.page), { timeout: 10_000 }).toBe(true);
  await settle(first.page, 3000);
  expect(first.sockets.length - before).toBe(1);
  expect(await open(second.page)).toBe(true);
  for (const tab of [first, second]) {
    await expect(tab.page.locator("#evictedBar")).toBeHidden();
    expect(tab.errors).toEqual([]);
  }
  await first.context.close();
  await second.context.close();
});

test("a reconnect scheduled for a lost socket does not bring a stepped-out tab back", async ({ browser, request }) => {
  test.setTimeout(90_000);
  const room = "evict-stale-timer";
  const name = "evict6-agent";
  await provisionRooms(request, [room]);
  const key = await invite(request, name, "agent", room);
  const routes = [];
  const tab = await door(browser, ws => {
    const server = ws.connectToServer();
    routes.push(ws);
    ws.onMessage(message => server.send(message));
    server.onMessage(message => ws.send(message));
  });
  await signIn(tab.page, name, key, room);

  // The first socket is lost as a network loses one: the page schedules its
  // reconnect. Before that fires, a second socket is opened another way
  // (what a session renewal or choosing the loca again does).
  await routes[routes.length - 1].close({ code: 1011, reason: "gone" });
  await tab.page.evaluate(() => openWs());
  await expect.poll(() => open(tab.page), { timeout: 10_000 }).toBe(true);

  // The agent's runtime takes the seat: the second socket is evicted and
  // the tab steps out.
  const session = await (await request.post("/sessions", { headers: { "x-room-token": key }, data: { name, kind: "agent" } })).json();
  const base = new URL(test.info().project.use.baseURL);
  const frames = [];
  const runtime = new WebSocket(`ws://${base.host}/ws?room=${room}&name=${name}&type=agent`, ["loca.v1", `loca.session.${session.session_token}`]);
  const closed = new Promise(resolve => runtime.addEventListener("close", resolve));
  runtime.addEventListener("message", event => frames.push(JSON.parse(event.data).t));
  await expect.poll(() => frames.includes("history")).toBe(true);
  await expectSteppedOut(tab.page, name);
  const before = tab.sockets.length;

  // The first socket's reconnect was due 1.5 s after it was lost. Nothing
  // opens a third socket: the tab stays out until someone asks.
  await settle(tab.page, 5000);
  expect(tab.sockets.length - before).toBe(0);
  await expectSteppedOut(tab.page, name);
  expect(frames).not.toContain("evicted");
  expect(runtime.readyState).toBe(WebSocket.OPEN);
  runtime.close();
  await closed;
  expect(tab.errors).toEqual([]);
  await tab.context.close();
});

test("a session that ends while the tab is stepped out is renewed without taking the seat back", async ({ browser, request }) => {
  test.setTimeout(90_000);
  const room = "evict-renew";
  const name = "evict5-agent";
  await provisionRooms(request, [room]);
  const key = await invite(request, name, "agent", room);
  const first = await door(browser);
  await signIn(first.page, name, key, room);
  const second = await door(browser);
  await signIn(second.page, name, key, room);
  await expectSteppedOut(first.page, name);
  const firstBefore = first.sockets.length;
  const secondBefore = second.sockets.length;

  const ended = await first.page.evaluate(() => state.session);
  expect((await request.delete("/sessions", { headers: { "x-session-token": ended } })).status()).toBe(204);
  await expect.poll(() => first.page.evaluate(() => state.session), { timeout: 20_000 }).not.toBe(ended);
  await settle(first.page, 4000);
  expect(first.sockets.length - firstBefore).toBe(0);
  expect(second.sockets.length - secondBefore).toBe(0);
  expect(await open(second.page)).toBe(true);
  await expectSteppedOut(first.page, name);
  await first.context.close();
  await second.context.close();
});

// The eviction is held back on its way to onFrame until the person it was for
// has signed out (and, in one case, someone else has signed in at the tab);
// then it is let through. It writes nothing for whoever is there now.
for (const after of ["sign-out", "another sign-in"]) {
test(`an eviction that lands after ${after} at the tab writes nothing`, async ({ browser, request }) => {
  test.setTimeout(90_000);
  const tag = after === "sign-out" ? "evict6" : "evict7";
  const room = `${tag}-room`;
  const name = `${tag}-agent`;
  await provisionRooms(request, [room]);
  const key = await invite(request, name, "agent", room);
  const next = await invite(request, `${tag}-next`, "user", room);
  const first = await door(browser);
  await signIn(first.page, name, key, room);
  await first.page.evaluate(() => {
    const deliver = onFrame;
    window.__held = [];
    window.onFrame = (f, from) => (f.t === "evicted" ? window.__held.push([f, from]) : deliver(f, from));
    window.__release = () => { for (const [f, from] of window.__held.splice(0)) deliver(f, from); };
  });
  const second = await door(browser);
  await signIn(second.page, name, key, room);
  await expect.poll(() => first.page.evaluate(() => window.__held.length)).toBe(1);
  await second.context.close();

  await signOut(first.page);
  if (after === "another sign-in") await signIn(first.page, `${tag}-next`, next, room);
  // A reconnect the first person's closed socket had already scheduled may
  // still reopen the socket for whoever is here now; let it.
  await settle(first.page, 4000);
  const status = await first.page.locator("#subStatus").textContent();
  const feed = await first.page.locator("#feed").innerHTML();
  const sockets = first.sockets.length;

  await first.page.evaluate(() => window.__release());
  await settle(first.page, 4000);
  await expect(first.page.locator("#evictedBar")).toBeHidden();
  expect(await first.page.locator("#subStatus").textContent()).toBe(status);
  expect(await first.page.locator("#feed").innerHTML()).toBe(feed);
  expect(first.sockets.length).toBe(sockets);
  if (after === "another sign-in") expect(await open(first.page)).toBe(true);
  // The button left over from the last person does nothing either.
  await first.page.evaluate(() => typeof useItHere === "function" && useItHere());
  await settle(first.page, 2000);
  expect(first.sockets.length).toBe(sockets);
  expect(first.errors).toEqual([]);
  await first.context.close();
});
}
