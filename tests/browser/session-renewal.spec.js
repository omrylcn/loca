const { test, expect } = require("@playwright/test");
const { MASTER, settle, openSignInForm } = require("./support/switch");
const { provisionRooms } = require("./support/building");

// A signed-in page whose session ends on the server (it expired, or it was
// ended) while the page still holds a key that can take another: an invite, a
// membership key, the sandbox room key. The page takes a new session with
// that key by itself and carries on: the socket reopens, the loca stays open,
// what was typed stays, and there is no door. The tests end the session with
// DELETE /sessions from outside, which is what expiry looks like to the page.

const ROOM = "renew-room";

async function door(browser) {
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  const dialogs = [];
  page.on("dialog", dialog => { dialogs.push(dialog.message()); dialog.dismiss().catch(() => {}); });
  const errors = [];
  page.on("pageerror", error => errors.push(String(error)));
  const exchanges = [];
  page.on("request", sent => {
    if (sent.method() === "POST" && new URL(sent.url()).pathname === "/sessions") exchanges.push(Date.now());
  });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("unauthenticated");
  return { context, page, dialogs, errors, exchanges };
}

async function signInHuman(page, name, key, room) {
  await openSignInForm(page);
  await page.locator("#name").fill(name);
  await page.locator("#roomToken").fill(key);
  await page.evaluate(r => { doConnect(r); }, room);
  await expect(page.locator("body")).not.toHaveClass(/locked/);
  await expect.poll(() => page.evaluate(() => state.room)).toBe(room);
  await expect.poll(() => page.evaluate(() => !!state.session && state.ws && state.ws.readyState), { timeout: 15_000 }).toBe(1);
}

// Record whether the door is ever shown from now on.
async function watchDoor(page) {
  await page.evaluate(() => {
    window.__doorShown = false;
    new MutationObserver(() => {
      if (document.body.classList.contains("locked")) window.__doorShown = true;
    }).observe(document.body, { attributes: true, attributeFilter: ["class"] });
  });
}

async function whoami(request, token) {
  return (await request.get("/whoami", { headers: { "x-session-token": token } })).status();
}

const KEYS = {
  invite: async (request, name, room = ROOM) => {
    await request.post("/members", { headers: MASTER, data: { name, kind: "user" } });
    const invited = await request.post(`/rooms/${room}/invites`, { headers: MASTER, data: { name, kind: "user" } });
    return (await invited.json()).token;
  },
  membership: async (request, name) => {
    const member = await (await request.post("/members", { headers: MASTER, data: { name, kind: "user" } })).json();
    expect((await request.post(`/rooms/${ROOM}/call`, { headers: MASTER, data: { name } })).ok()).toBeTruthy();
    return member.token;
  },
  "sandbox room key": async () => "building",
};

for (const [index, kind] of Object.keys(KEYS).entries()) {
test(`a ${kind} sign-in whose session ended on the server takes a new one and carries on`, async ({ browser, request }) => {
  test.setTimeout(90_000);
  await provisionRooms(request, [ROOM]);
  const name = `renew${index}-person`;
  const key = await KEYS[kind](request, name);
  const { context, page, dialogs, errors, exchanges } = await door(browser);
  await signInHuman(page, name, key, ROOM);
  const s1 = await page.evaluate(() => state.session);
  const DRAFT = `renew${index}-unsent-draft-31c9`;
  await page.fill("#msg", DRAFT);
  await watchDoor(page);
  const before = exchanges.length;

  // The renewal's exchange is held, long enough for both triggers to have
  // seen the session gone: the 5 s poll and the socket the server closes.
  const held = {};
  held.arrived = new Promise(resolve => { held.seen = resolve; });
  const released = new Promise(resolve => { held.release = resolve; });
  await page.route("**/sessions", async route => {
    if (route.request().method() !== "POST" || held.taken) return route.fallback();
    held.taken = true;
    held.seen();
    await released;
    await route.fallback();
  });
  const refusedReads = [];
  page.on("response", response => {
    if (new URL(response.url()).pathname === "/rooms" && response.status() === 401) refusedReads.push(Date.now());
  });

  // The session ends on the server.
  expect((await request.delete("/sessions", { headers: { "x-session-token": s1 } })).status()).toBe(204);
  await held.arrived;
  // Both triggers fire while the exchange is out: the socket is closed by the
  // server and retried, and the poll reads /rooms and is refused.
  await expect.poll(() => page.evaluate(() => !state.ws || state.ws.readyState !== 1), { timeout: 20_000 }).toBe(true);
  await expect.poll(() => refusedReads.length, { timeout: 15_000 }).toBeGreaterThan(0);
  await settle(page, 3000);
  expect(exchanges.length - before).toBe(1);
  held.release();

  // A new session, accepted by the server; the socket open; the loca and the
  // draft where they were; no door; and that one exchange only.
  await expect.poll(() => page.evaluate(() => state.session), { timeout: 15_000 }).not.toBe(s1);
  await expect.poll(() => page.evaluate(() => !!state.session && state.ws && state.ws.readyState), { timeout: 15_000 }).toBe(1);
  const s2 = await page.evaluate(() => state.session);
  expect(await whoami(request, s2)).toBe(200);
  expect(await whoami(request, s1)).toBe(401);
  await settle(page, 6000);   // past the next poll and a reconnect
  expect(await page.evaluate(() => state.session)).toBe(s2);
  expect(exchanges.length - before).toBe(1);
  const now = await page.evaluate(() => ({
    room: state.room, draft: document.getElementById("msg").value, auth: state.authStatus,
    locked: document.body.classList.contains("locked"), doorShown: window.__doorShown, ws: state.ws && state.ws.readyState,
  }));
  expect(now).toEqual({ room: ROOM, draft: DRAFT, auth: "authenticated", locked: false, doorShown: false, ws: 1 });
  expect(dialogs).toEqual([]);
  expect(errors).toEqual([]);
  await context.close();
});
}

// The key itself is refused when the page tries it again: the door closes
// once, says so truthfully, and the page does not keep trying.
for (const [index, end] of ["suspended", "revoked"].entries()) {
test(`a member ${end} while signed in is locked out once, with a true sentence, and no exchange loop`, async ({ browser, request }) => {
  test.setTimeout(90_000);
  await provisionRooms(request, [ROOM]);
  const name = `renewx${index}-person`;
  const key = await KEYS.membership(request, name);
  const { context, page, dialogs, errors, exchanges } = await door(browser);
  await signInHuman(page, name, key, ROOM);
  const before = exchanges.length;

  const residents = await (await request.get("/residents", { headers: MASTER })).json();
  const id = residents.find(r => r.name === name)?.membership_id;
  expect(id).toBeTruthy();
  const ended = end === "suspended"
    ? await request.post(`/members/${encodeURIComponent(id)}/suspension`, { headers: MASTER, data: { suspended: true } })
    : await request.delete(`/members/${encodeURIComponent(id)}`, { headers: MASTER });
  expect(ended.ok()).toBeTruthy();

  await expect(page.locator("body")).toHaveClass(/locked/, { timeout: 30_000 });
  await expect(page.locator("#doorline")).toHaveText("that key does not open this table.");
  await settle(page, 12_000);   // two polls and several reconnect periods
  expect(exchanges.length - before).toBeLessThanOrEqual(1);
  expect(await page.evaluate(() => document.body.classList.contains("locked"))).toBe(true);
  expect(dialogs).toEqual([]);
  expect(errors).toEqual([]);
  await context.close();
});
}

// A Master session taken with a one-use pairing code cannot be renewed: the
// door asks again, saying the session ended, and the spent code is not tried.
test("a Master pairing session that ended asks at the door again, without trying the spent code", async ({ browser, request }) => {
  test.setTimeout(60_000);
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const { context, page, dialogs, errors, exchanges } = await door(browser);
  await page.evaluate(code => {
    setEntryPath("master");
    document.getElementById("pairingCode").value = code;
  }, pairing_code);
  await page.locator("#connectBtn").click();
  await expect.poll(() => page.evaluate(() => state.profile?.building_role)).toBe("master");
  const master = await page.evaluate(() => state.session);
  const before = exchanges.length;

  expect((await request.delete("/sessions", { headers: { "x-session-token": master } })).status()).toBe(204);
  await expect(page.locator("body")).toHaveClass(/locked/, { timeout: 30_000 });
  await expect(page.locator("#doorline")).toContainText("your session ended");
  await settle(page, 6000);
  expect(exchanges.length - before).toBe(0);
  expect(dialogs).toEqual([]);
  expect(errors).toEqual([]);
  await context.close();
});

// A renewal belongs to the person it was started for. The first person's is
// held indefinitely; the next person signs in at the tab, loses their own
// session, and is renewed on their own while the first is still held. When
// the first comes back, whatever it is, nothing of it reaches the next
// person, and a session it was granted is ended.
for (const outcome of ["success", "refusal", "network error"]) {
test(`a renewal still out for the last person is not joined by the next one (it ends in ${outcome})`, async ({ browser, request }) => {
  test.setTimeout(120_000);
  const tag = `renewp${["success", "refusal", "network error"].indexOf(outcome)}`;
  const room = `${tag}-room`;
  await provisionRooms(request, [room]);
  const keyA = await KEYS.invite(request, `${tag}-a`, room);
  const keyB = await KEYS.invite(request, `${tag}-b`, room);
  const { context, page, dialogs, errors } = await door(browser);
  const exchanges = { a: 0, b: 0 };
  const heldA = {};
  heldA.arrived = new Promise(resolve => { heldA.seen = resolve; });
  const releasedA = new Promise(resolve => { heldA.release = resolve; });
  let holdA = false;
  await page.route("**/sessions", async route => {
    const sent = route.request();
    if (sent.method() !== "POST") return route.fallback();
    const key = sent.headers()["x-room-token"];
    if (key === keyB) { exchanges.b++; return route.fallback(); }
    if (key !== keyA) return route.fallback();
    exchanges.a++;
    if (!holdA) return route.fallback();
    if (outcome === "success") {
      const response = await route.fetch();
      const body = await response.json();
      heldA.token = body.session_token;
      heldA.seen();
      await releasedA;
      return route.fulfill({ response, json: body }).catch(() => {});
    }
    heldA.seen();
    await releasedA;
    if (outcome === "refusal") return route.fulfill({ status: 401, body: "that key opens nothing" }).catch(() => {});
    return route.abort("connectionreset").catch(() => {});
  });

  // A signs in; A's session ends; A's renewal is held.
  await signInHuman(page, `${tag}-a`, keyA, room);
  const a1 = await page.evaluate(() => state.session);
  holdA = true;
  expect((await request.delete("/sessions", { headers: { "x-session-token": a1 } })).status()).toBe(204);
  await heldA.arrived;

  // B signs in over it, and is in.
  await signInHuman(page, `${tag}-b`, keyB, room);
  const b1 = await page.evaluate(() => state.session);
  expect(b1).toBeTruthy();
  expect(await whoami(request, b1)).toBe(200);
  await watchDoor(page);

  // B's session ends too; B is renewed on their own while A's is still held.
  expect((await request.delete("/sessions", { headers: { "x-session-token": b1 } })).status()).toBe(204);
  await expect.poll(() => page.evaluate(() => state.session), { timeout: 30_000 }).not.toBe(b1);
  await expect.poll(() => page.evaluate(() => !!state.session && state.ws && state.ws.readyState), { timeout: 20_000 }).toBe(1);
  const b2 = await page.evaluate(() => state.session);
  expect(await whoami(request, b2)).toBe(200);
  expect(exchanges.a).toBe(2);   // A's sign-in and A's held renewal
  expect(exchanges.b).toBe(2);   // B's sign-in and B's own renewal

  // Now A's comes back.
  heldA.release(outcome);
  await settle(page, 6000);
  const now = await page.evaluate(() => ({
    session: state.session, name: state.name, room: state.room, auth: state.authStatus,
    locked: document.body.classList.contains("locked"), doorShown: window.__doorShown, ws: state.ws && state.ws.readyState,
  }));
  expect(now).toEqual({ session: b2, name: `${tag}-b`, room, auth: "authenticated", locked: false, doorShown: false, ws: 1 });
  expect(await whoami(request, b2)).toBe(200);
  if (outcome === "success") {
    await expect.poll(() => whoami(request, heldA.token), { timeout: 10_000 }).toBe(401);
  }
  expect(exchanges.a).toBe(2);
  expect(exchanges.b).toBe(2);
  expect(dialogs).toEqual([]);
  expect(errors).toEqual([]);
  await context.close();
});
}

// A renewal the network leaves without an answer is tried again on its own,
// at the poll's pace, until the network answers.
test("a renewal that failed at the network is tried again by itself and the page carries on", async ({ browser, request }) => {
  test.setTimeout(90_000);
  const room = "renewn-room";
  await provisionRooms(request, [room]);
  const name = "renewn-person";
  const key = await KEYS.invite(request, name, room);
  const { context, page, dialogs, errors } = await door(browser);
  await signInHuman(page, name, key, room);
  const s1 = await page.evaluate(() => state.session);
  const DRAFT = "renewn-unsent-draft-77ab";
  await page.fill("#msg", DRAFT);
  await watchDoor(page);
  const attempts = [];
  await page.route("**/sessions", async route => {
    if (route.request().method() !== "POST") return route.fallback();
    attempts.push(Date.now());
    if (attempts.length === 1) return route.abort("connectionreset");
    return route.fallback();
  });

  expect((await request.delete("/sessions", { headers: { "x-session-token": s1 } })).status()).toBe(204);
  await expect.poll(() => attempts.length, { timeout: 20_000 }).toBeGreaterThan(0);
  // The network is back from the second attempt on: the session and the
  // socket come back by themselves.
  await expect.poll(() => page.evaluate(() => !!state.session && state.ws && state.ws.readyState), { timeout: 30_000 }).toBe(1);
  const s2 = await page.evaluate(() => state.session);
  expect(await whoami(request, s2)).toBe(200);
  await settle(page, 6000);
  expect(attempts.length).toBe(2);
  const now = await page.evaluate(() => ({
    session: state.session, room: state.room, draft: document.getElementById("msg").value,
    locked: document.body.classList.contains("locked"), doorShown: window.__doorShown, ws: state.ws && state.ws.readyState,
  }));
  expect(now).toEqual({ session: s2, room, draft: DRAFT, locked: false, doorShown: false, ws: 1 });
  expect(dialogs).toEqual([]);
  expect(errors).toEqual([]);
  await context.close();
});

test("while the network stays down, the renewal is tried at most once per poll and the page says it is reconnecting", async ({ browser, request }) => {
  test.setTimeout(90_000);
  const room = "renewd-room";
  await provisionRooms(request, [room]);
  const name = "renewd-person";
  const key = await KEYS.invite(request, name, room);
  const { context, page, dialogs, errors } = await door(browser);
  await signInHuman(page, name, key, room);
  const s1 = await page.evaluate(() => state.session);
  await watchDoor(page);
  const doorline = await page.locator("#doorline").textContent();
  const attempts = [];
  let down = true;
  await page.route("**/sessions", async route => {
    if (route.request().method() !== "POST") return route.fallback();
    attempts.push(Date.now());
    return down ? route.abort("connectionreset") : route.fallback();
  });

  expect((await request.delete("/sessions", { headers: { "x-session-token": s1 } })).status()).toBe(204);
  await expect.poll(() => attempts.length, { timeout: 20_000 }).toBeGreaterThan(0);
  await settle(page, 22_000);   // several polls with the network down
  // It keeps trying, but no faster than the 5 s poll.
  expect(attempts.length).toBeGreaterThanOrEqual(3);
  expect(attempts.length).toBeLessThanOrEqual(6);
  for (let i = 1; i < attempts.length; i++) expect(attempts[i] - attempts[i - 1]).toBeGreaterThan(3500);
  const during = await page.evaluate(() => ({
    locked: document.body.classList.contains("locked"), doorShown: window.__doorShown,
    status: document.getElementById("connDot").getAttribute("aria-label"),
    doorline: document.getElementById("doorline").textContent,
  }));
  expect(during.locked).toBe(false);
  expect(during.doorShown).toBe(false);
  expect(during.doorline).toBe(doorline);
  expect(during.status).toMatch(/reconnecting|pulling your chair back|connecting/);

  // The network comes back: so do the session and the socket.
  down = false;
  await expect.poll(() => page.evaluate(() => !!state.session && state.ws && state.ws.readyState), { timeout: 20_000 }).toBe(1);
  expect(await whoami(request, await page.evaluate(() => state.session))).toBe(200);
  expect(dialogs).toEqual([]);
  expect(errors).toEqual([]);
  await context.close();
});
