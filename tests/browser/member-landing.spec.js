const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

// Where a page load lands. Care is the ranks' and caretakers' own loca
// (PRINCIPLES "İye"): a plain member cannot enter it, so it must never be
// sent there to sit at "connecting…" forever. A member lands in its last
// loca if it may still enter it, else its first seat, else the Lobby room,
// where everyone in the Building lands first (PRINCIPLES "Lobby").

const MASTER = { "x-admin-token": "MASTER" };

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
});

async function admit(request, name) {
  const admitted = await request.post("/members", { headers: MASTER, data: { name, kind: "user" } });
  expect(admitted.ok()).toBeTruthy();
  return (await admitted.json()).token;
}

async function invite(request, room, name) {
  const invited = await request.post(`/rooms/${room}/invites`, { headers: MASTER, data: { name } });
  expect(invited.ok(), await invited.text()).toBeTruthy();
}

async function signIn(page, kind, name, key) {
  await page.goto("/");
  await page.locator("#entryKind").selectOption(kind);
  await page.locator("#name").fill(name);
  await page.locator("#roomToken").fill(key);
  await page.locator("#connectBtn").click();
}

async function expectSeatedIn(page, room) {
  await expect.poll(() => page.evaluate(() => state.room)).toBe(room);
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  await expect(page.locator("#connDot")).toHaveAttribute("aria-label", "connected");
}

async function sidebarRooms(page) {
  return page.locator("#roomList .rname").allTextContents();
}

test("a member with an invite lands in its loca, not in Care, and again after a reload", async ({ page, request }) => {
  await provisionRooms(request, ["landing-own"]);
  const key = await admit(request, "landing-member");
  await invite(request, "landing-own", "landing-member");
  const home = await (await request.get("/health")).json();
  expect(home.loca_agent_room).toBe("iye");

  await signIn(page, "human", "landing-member", key);
  await expectSeatedIn(page, "landing-own");
  expect(await sidebarRooms(page)).toEqual(["landing-own"]);

  await page.reload();
  await expectSeatedIn(page, "landing-own");
  expect(await sidebarRooms(page)).not.toContain("iye");
});

test("a member lands in its last loca while it may enter it, else its first seat", async ({ page, request }) => {
  await provisionRooms(request, ["landing-first", "landing-last"]);
  const key = await admit(request, "landing-two");
  await invite(request, "landing-first", "landing-two");
  await invite(request, "landing-last", "landing-two");

  await signIn(page, "human", "landing-two", key);
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  await page.evaluate(() => doConnect("landing-last"));
  await expectSeatedIn(page, "landing-last");
  await page.reload();
  await expectSeatedIn(page, "landing-last");

  // Released from it: the remembered loca is no longer one it may enter.
  const released = await request.post("/rooms/landing-last/moderate", {
    headers: MASTER, data: { action: "release", name: "landing-two" },
  });
  expect(released.ok(), await released.text()).toBeTruthy();
  await page.reload();
  await expectSeatedIn(page, "landing-first");
});

test("a member with no seat lands in the Lobby room instead of connecting to Care", async ({ page, request }) => {
  await provisionRooms(request, ["landing-called"]);
  const key = await admit(request, "landing-lobby");
  const sockets = [];
  page.on("websocket", ws => sockets.push(new URL(ws.url()).searchParams.get("room")));

  for (const load of ["sign-in", "reload"]) {
    if (load === "sign-in") await signIn(page, "human", "landing-lobby", key);
    else await page.reload();
    await expectSeatedIn(page, "lobby");
    await expect(page.locator("#curRoom")).toHaveText("Lobby");
    await expect(page.locator("#lobbyRoomLink")).toHaveClass(/active/);
    // The Lobby room is not a loca in the list.
    expect(await sidebarRooms(page)).toEqual([]);
  }
  expect(sockets).not.toContain("iye");

  // Called in: the loca appears in the list, and the next load lands there.
  await invite(request, "landing-called", "landing-lobby");
  await page.evaluate(() => refreshRooms());
  await expect.poll(() => sidebarRooms(page)).toEqual(["landing-called"]);
  await page.reload();
  await expectSeatedIn(page, "landing-called");
});

test("a Smaster still lands in Care, the ranks' own loca", async ({ page, request }) => {
  const issued = await request.post("/smasters", { headers: MASTER, data: { name: "landing-smaster" } });
  expect(issued.ok()).toBeTruthy();
  const { token } = await issued.json();
  await signIn(page, "smaster", "landing-smaster", token);
  await expectSeatedIn(page, "iye");
  await page.reload();
  await expectSeatedIn(page, "iye");
});

test("the Master still lands in Care, the ranks' own loca", async ({ page, request }) => {
  const paired = await request.post("/pairings", { headers: MASTER, data: {} });
  const { pairing_code } = await paired.json();
  await page.goto("/");
  await page.locator("#masterEntryButton").click();
  await page.locator("#pairingCode").fill(pairing_code);
  await page.locator("#connectBtn").click();
  await expectSeatedIn(page, "iye");
  await page.reload();
  await expectSeatedIn(page, "iye");
});

// GET /rooms is how the page learns which doors open for this person. When it
// cannot be read, a member must not fall back to Care and knock at a door that
// never opens: it waits, reads the list again, and lands once it is known.
function roomsListOnly(url) { return new URL(url).pathname === "/rooms"; }

for (const [failure, fulfil] of [
  ["answers 500", route => route.fulfill({ status: 500, contentType: "application/json", body: "{\"error\":\"boom\"}" })],
  ["cannot be reached", route => route.abort("connectionfailed")],
  ["is not a list", route => route.fulfill({ status: 200, contentType: "application/json", body: "{\"rooms\":\"nope\"}" })],
]) {
  test(`a member is not sent to Care while the list of locas ${failure}, and lands once it is read`, async ({ page, request }) => {
    const slug = failure.replace(/[^a-z0-9]+/g, "-");
    const name = `landing-unread-${slug}`;
    await provisionRooms(request, [`landing-own-${slug}`]);
    const key = await admit(request, name);
    await invite(request, `landing-own-${slug}`, name);

    const sockets = [];
    page.on("websocket", ws => sockets.push(new URL(ws.url()).searchParams.get("room")));
    await page.route(roomsListOnly, fulfil);
    await signIn(page, "human", name, key);
    await expect(page.locator("#feed")).toContainText("could not be read");
    await expect(page.locator("#subStatus")).toHaveText("could not read your locas — trying again");
    await expect(page.locator("#curRoom")).toHaveText("no loca open");
    // Past the first retry (2 s): still no door tried, Care least of all.
    await page.waitForTimeout(3000);
    expect(await page.evaluate(() => ({ room: state.room, ws: state.ws }))).toEqual({ room: null, ws: null });
    expect(sockets).toEqual([]);

    await page.unroute(roomsListOnly);
    await expectSeatedIn(page, `landing-own-${slug}`);
    expect(sockets).not.toContain("iye");
  });
}

test("a member keeps its remembered loca across a load in which the list could not be read", async ({ page, request }) => {
  await provisionRooms(request, ["landing-kept-first", "landing-kept-last"]);
  const key = await admit(request, "landing-kept");
  await invite(request, "landing-kept-first", "landing-kept");
  await invite(request, "landing-kept-last", "landing-kept");
  await signIn(page, "human", "landing-kept", key);
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  await page.evaluate(() => doConnect("landing-kept-last"));
  await expectSeatedIn(page, "landing-kept-last");

  await page.route(roomsListOnly, route => route.fulfill({ status: 500, body: "" }));
  await page.reload();
  await expect(page.locator("#subStatus")).toHaveText("could not read your locas — trying again");
  expect(await page.evaluate(() => JSON.parse(localStorage.getItem("loca-seat")).room)).toBe("landing-kept-last");
  await page.unroute(roomsListOnly);
  await expectSeatedIn(page, "landing-kept-last");
});

test("a rank goes to Care even while the list of locas cannot be read: that door is known to open for it", async ({ page, request }) => {
  const issued = await request.post("/smasters", { headers: MASTER, data: { name: "landing-rank-unread" } });
  expect(issued.ok()).toBeTruthy();
  const { token } = await issued.json();
  await page.route(roomsListOnly, route => route.fulfill({ status: 500, body: "" }));
  await signIn(page, "smaster", "landing-rank-unread", token);
  // Seated, by the socket itself. The status line is not asserted here: the
  // sidebar's own read of the list fails too and reports that in the same
  // line ("cannot reach server"), whichever of the two finishes last.
  await expect.poll(() => page.evaluate(() => state.room)).toBe("iye");
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  await expect.poll(() => page.evaluate(() => state.members.some(m => m.name === "landing-rank-unread"))).toBe(true);
});

// Only Care is known to open for a rank without the list: a Smaster outside a
// group is refused at that group's locas. A rank's remembered loca is
// therefore not tried blind either; it waits for the list like anyone's.
test("a rank's remembered loca is not tried while the list of locas cannot be read", async ({ page, request }) => {
  await provisionRooms(request, ["landing-rank-kept"]);
  const issued = await request.post("/smasters", { headers: MASTER, data: { name: "landing-rank-kept-s" } });
  expect(issued.ok()).toBeTruthy();
  const { token } = await issued.json();
  await signIn(page, "smaster", "landing-rank-kept-s", token);
  await expectSeatedIn(page, "iye");
  await page.evaluate(() => doConnect("landing-rank-kept"));
  await expectSeatedIn(page, "landing-rank-kept");

  await page.route(roomsListOnly, route => route.fulfill({ status: 500, body: "" }));
  const sockets = [];
  page.on("websocket", ws => sockets.push(new URL(ws.url()).searchParams.get("room")));
  await page.reload();
  await expect(page.locator("#subStatus")).toHaveText("could not read your locas — trying again");
  await page.waitForTimeout(3000);
  expect(sockets).toEqual([]);
  expect(await page.evaluate(() => JSON.parse(localStorage.getItem("loca-seat")).room)).toBe("landing-rank-kept");
  await page.unroute(roomsListOnly);
  await expectSeatedIn(page, "landing-rank-kept");
});

// The list answers late, and meanwhile the person has opened a loca itself.
// The late answer is for a decision that has been replaced: it must not move
// the person.
test("a list of locas that answers late does not move a person out of the loca it opened meanwhile", async ({ page, request }) => {
  await provisionRooms(request, ["landing-late-a", "landing-late-b"]);
  const key = await admit(request, "landing-late");
  await invite(request, "landing-late-a", "landing-late");
  await invite(request, "landing-late-b", "landing-late");
  await signIn(page, "human", "landing-late", key);
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  await page.evaluate(() => doConnect("landing-late-a"));
  await expectSeatedIn(page, "landing-late-a");

  // Hold every read of the list from now on until the test lets it go.
  let release;
  const held = new Promise(resolve => { release = resolve; });
  let asked = 0;
  await page.route(roomsListOnly, async route => { asked++; await held; await route.continue(); });
  // A landing is asked for (a-loca), and while its list is out the person
  // opens b-loca itself.
  await page.evaluate(() => { land(state.buildingGen, "landing-late-a"); });
  await expect.poll(() => asked).toBeGreaterThan(0);
  await page.evaluate(() => joinRoom("landing-late-b"));
  await expectSeatedIn(page, "landing-late-b");
  release();
  await page.waitForTimeout(1500);
  expect(await page.evaluate(() => state.room)).toBe("landing-late-b");
  await page.unroute(roomsListOnly);
});

// Leaving a loca without being asked where to: the loca on screen is sealed,
// or a private conversation is closed. The page decides where to go the same
// way it does at sign-in, never Care by default, keeps the session it holds,
// and says where the person actually goes.

function socketRooms(page) {
  const rooms = [];
  page.on("websocket", ws => rooms.push(new URL(ws.url()).searchParams.get("room")));
  return rooms;
}

async function seal(request, room) {
  const archived = await request.put(`/rooms/${room}/settings`, { headers: MASTER, data: { archived: true } });
  expect(archived.ok(), await archived.text()).toBeTruthy();
  const sealed = await request.delete(`/rooms/${room}`, { headers: MASTER });
  expect(sealed.status()).toBe(204);
}

async function memberIn(page, request, name, seats) {
  const key = await admit(request, name);
  for (const room of seats) await invite(request, room, name);
  await signIn(page, "human", name, key);
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  return key;
}

test("a member whose loca is sealed moves to its other loca, is told so, and keeps its session", async ({ page, request }) => {
  await provisionRooms(request, ["leave-other", "leave-sealed"]);
  const sockets = socketRooms(page);
  await memberIn(page, request, "leave-two", ["leave-other", "leave-sealed"]);
  await page.evaluate(() => joinRoom("leave-sealed"));
  await expectSeatedIn(page, "leave-sealed");
  const session = await page.evaluate(() => state.session);

  // Closing (archiving) alone moves nobody: the loca is read-only, not gone.
  const archived = await request.put("/rooms/leave-sealed/settings", { headers: MASTER, data: { archived: true } });
  expect(archived.ok()).toBeTruthy();
  await page.waitForTimeout(1000);
  expect(await page.evaluate(() => state.room)).toBe("leave-sealed");

  expect((await request.delete("/rooms/leave-sealed", { headers: MASTER })).status()).toBe(204);
  await expect(page.locator("#feed")).toContainText("this loca was closed — moving you to leave-other");
  await expectSeatedIn(page, "leave-other");
  expect(await page.evaluate(() => state.session)).toBe(session);
  expect(sockets).not.toContain("iye");
});

test("a member whose only loca is sealed is told it goes to the Lobby, and lands in the Lobby room", async ({ page, request }) => {
  await provisionRooms(request, ["leave-only"]);
  const sockets = socketRooms(page);
  await memberIn(page, request, "leave-one", ["leave-only"]);
  await expectSeatedIn(page, "leave-only");

  await seal(request, "leave-only");
  // Told, then moved: the Lobby room's own feed replaces the notice.
  await expectSeatedIn(page, "lobby");
  await expect(page.locator("#curRoom")).toHaveText("Lobby");
  expect(sockets).not.toContain("iye");
});

test("a Smaster whose loca is sealed still moves to Care", async ({ page, request }) => {
  await provisionRooms(request, ["leave-rank-sealed"]);
  const issued = await request.post("/smasters", { headers: MASTER, data: { name: "leave-smaster" } });
  const { token } = await issued.json();
  await signIn(page, "smaster", "leave-smaster", token);
  await expectSeatedIn(page, "iye");
  await page.evaluate(() => joinRoom("leave-rank-sealed"));
  await expectSeatedIn(page, "leave-rank-sealed");

  await seal(request, "leave-rank-sealed");
  await expect(page.locator("#feed")).toContainText("this loca was closed — moving you to iye");
  await expectSeatedIn(page, "iye");
});

async function openDmWith(page, peer) {
  await page.evaluate(peer => openDm(peer), peer);
  await expect.poll(() => page.evaluate(() => isDm(state.room))).toBe(true);
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  return page.evaluate(() => state.room);
}

async function closeDm(page, room) {
  page.once("dialog", dialog => dialog.accept());
  await page.evaluate(room => deleteDm(room, dmOf(room).peer), room);
}

test("a member who closes a private conversation returns to the loca it was in", async ({ page, request }) => {
  await provisionRooms(request, ["leave-bb-first", "leave-bb-second"]);
  await admit(request, "leave-bb-peer");
  await invite(request, "leave-bb-second", "leave-bb-peer");
  const sockets = socketRooms(page);
  await memberIn(page, request, "leave-bb-two", ["leave-bb-first", "leave-bb-second"]);
  await page.evaluate(() => joinRoom("leave-bb-second"));
  await expectSeatedIn(page, "leave-bb-second");

  const room = await openDmWith(page, "leave-bb-peer");
  await closeDm(page, room);
  await expectSeatedIn(page, "leave-bb-second");
  expect(sockets).not.toContain("iye");
});

test("a loca the member opens itself while the page announces a move wins over that move", async ({ page, request }) => {
  await provisionRooms(request, ["leave-own-first", "leave-own-picked", "leave-own-sealed"]);
  await memberIn(page, request, "leave-own-move", ["leave-own-first", "leave-own-picked", "leave-own-sealed"]);
  await page.evaluate(() => joinRoom("leave-own-sealed"));
  await expectSeatedIn(page, "leave-own-sealed");

  await seal(request, "leave-own-sealed");
  await expect(page.locator("#feed")).toContainText("this loca was closed — moving you to leave-own-first");
  await page.evaluate(() => joinRoom("leave-own-picked"));
  await expectSeatedIn(page, "leave-own-picked");
  // Past the announced move (800 ms): the person's own choice stands.
  await page.waitForTimeout(1500);
  expect(await page.evaluate(() => state.room)).toBe("leave-own-picked");
});

test("a member with one seat who closes a private conversation returns to that loca", async ({ page, request }) => {
  await provisionRooms(request, ["leave-bb-own"]);
  await admit(request, "leave-bb-mate");
  await invite(request, "leave-bb-own", "leave-bb-mate");
  const sockets = socketRooms(page);
  await memberIn(page, request, "leave-bb-one", ["leave-bb-own"]);
  await expectSeatedIn(page, "leave-bb-own");

  const room = await openDmWith(page, "leave-bb-mate");
  await closeDm(page, room);
  await expectSeatedIn(page, "leave-bb-own");
  expect(sockets).not.toContain("iye");
});

test("a member whose private conversation the Master seals returns to the loca it was in, and is told so", async ({ page, request }) => {
  await provisionRooms(request, ["leave-seal-first", "leave-seal-second"]);
  await admit(request, "leave-seal-peer");
  await invite(request, "leave-seal-second", "leave-seal-peer");
  const sockets = socketRooms(page);
  await memberIn(page, request, "leave-seal-two", ["leave-seal-first", "leave-seal-second"]);
  await page.evaluate(() => joinRoom("leave-seal-second"));
  await expectSeatedIn(page, "leave-seal-second");
  const room = await openDmWith(page, "leave-seal-peer");

  const closed = await request.post(`/building/birebirs/${room}/close`, { headers: MASTER, data: { archived: true } });
  expect(closed.ok(), await closed.text()).toBeTruthy();
  const sealed = await request.post(`/building/birebirs/${room}/seal`, { headers: MASTER });
  expect(sealed.ok(), await sealed.text()).toBeTruthy();
  await expect(page.locator("#feed")).toContainText("this conversation was closed — moving you to leave-seal-second");
  await expectSeatedIn(page, "leave-seal-second");
  expect(sockets).not.toContain("iye");
});

test("a Smaster who closes a private conversation it opened from Care returns to Care", async ({ page, request }) => {
  await request.post("/smasters", { headers: MASTER, data: { name: "leave-bb-rank-peer" } });
  const issued = await request.post("/smasters", { headers: MASTER, data: { name: "leave-bb-rank" } });
  const { token } = await issued.json();
  await signIn(page, "smaster", "leave-bb-rank", token);
  await expectSeatedIn(page, "iye");

  const room = await openDmWith(page, "leave-bb-rank-peer");
  await closeDm(page, room);
  await expectSeatedIn(page, "iye");
});

test("the Master who seals the loca it is in moves to Care without taking a new session", async ({ page, request }) => {
  await provisionRooms(request, ["leave-master-sealed"]);
  const paired = await request.post("/pairings", { headers: MASTER, data: {} });
  const { pairing_code } = await paired.json();
  await page.goto("/");
  await page.locator("#masterEntryButton").click();
  await page.locator("#pairingCode").fill(pairing_code);
  await page.locator("#connectBtn").click();
  await expectSeatedIn(page, "iye");
  await page.evaluate(() => joinRoom("leave-master-sealed"));
  await expectSeatedIn(page, "leave-master-sealed");
  const session = await page.evaluate(() => state.session);

  const archived = await request.put("/rooms/leave-master-sealed/settings", { headers: MASTER, data: { archived: true } });
  expect(archived.ok()).toBeTruthy();
  page.once("dialog", dialog => dialog.accept("leave-master-sealed"));
  await page.evaluate(() => deleteRoom("leave-master-sealed"));
  await expectSeatedIn(page, "iye");
  expect(await page.evaluate(() => state.session)).toBe(session);
});

// The loca a person moved to last is where a reload lands: for a member, a
// Smaster and the Master alike (and on the desktop, where the seat lives in
// the keychain). Moving is a click in the sidebar, not a new sign-in.
async function openFromSidebar(page, room) {
  await page.locator("#roomList button.room", {
    has: page.locator(".rname", { hasText: new RegExp(`^${room}$`) }),
  }).click();
  await expectSeatedIn(page, room);
}

function rememberedRoom(page) {
  return page.evaluate(() => JSON.parse(localStorage.getItem("loca-seat") || "null")?.room);
}

test("a member who moved to another loca lands there after a reload", async ({ page, request }) => {
  await provisionRooms(request, ["last-a", "last-b"]);
  const key = await admit(request, "last-member");
  await invite(request, "last-a", "last-member");
  await invite(request, "last-b", "last-member");
  await signIn(page, "human", "last-member", key);
  await expectSeatedIn(page, "last-a");

  await openFromSidebar(page, "last-b");
  await page.reload();
  await expectSeatedIn(page, "last-b");
  await openFromSidebar(page, "last-a");
  await page.reload();
  await expectSeatedIn(page, "last-a");
});

test("a Smaster who moved to another loca lands there after a reload", async ({ page, request }) => {
  await provisionRooms(request, ["last-smaster-room"]);
  const issued = await request.post("/smasters", { headers: MASTER, data: { name: "last-smaster" } });
  expect(issued.ok()).toBeTruthy();
  await signIn(page, "smaster", "last-smaster", (await issued.json()).token);
  await expectSeatedIn(page, "iye");
  await openFromSidebar(page, "last-smaster-room");
  await page.reload();
  await expectSeatedIn(page, "last-smaster-room");
});

test("the Master who moved to another loca lands there after a reload", async ({ page, request }) => {
  await provisionRooms(request, ["last-master-room"]);
  const { pairing_code } = await (await request.post("/pairings", { headers: MASTER, data: {} })).json();
  await page.goto("/");
  await page.locator("#masterEntryButton").click();
  await page.locator("#pairingCode").fill(pairing_code);
  await page.locator("#connectBtn").click();
  await expectSeatedIn(page, "iye");
  await openFromSidebar(page, "last-master-room");
  await page.reload();
  await expectSeatedIn(page, "last-master-room");
});

for (const [how, end] of [
  ["released from", (request, room, name) => request.post(`/rooms/${room}/moderate`, {
    headers: MASTER, data: { action: "release", name },
  })],
  ["closed", (request, room) => request.put(`/rooms/${room}/settings`, {
    headers: MASTER, data: { archived: true },
  })],
]) {
  test(`a member ${how} its last loca lands in its first seat, and that loca is not kept`, async ({ page, request }) => {
    const slug = how.split(" ")[0];
    const [first, last, name] = [`last-${slug}-a`, `last-${slug}-b`, `last-${slug}`];
    await provisionRooms(request, [first, last]);
    const key = await admit(request, name);
    await invite(request, first, name);
    await invite(request, last, name);
    await signIn(page, "human", name, key);
    await expectSeatedIn(page, first);
    await openFromSidebar(page, last);
    await expect.poll(() => rememberedRoom(page)).toBe(last);

    const ended = await end(request, last, name);
    expect(ended.ok(), await ended.text()).toBeTruthy();
    await page.reload();
    await expectSeatedIn(page, first);
    await expect.poll(() => rememberedRoom(page)).toBe(first);
  });
}

// A closed loca is kept, read-only. Nobody is sent back into one while
// another loca is open; when every loca the person may enter is closed, the
// closed one is still where they are (they can read it), not the Lobby, which
// would say no loca has a seat for them.
test("a member whose only loca was closed still lands in it, not in the Lobby", async ({ page, request }) => {
  await provisionRooms(request, ["last-only-closed"]);
  const key = await admit(request, "last-only");
  await invite(request, "last-only-closed", "last-only");
  await signIn(page, "human", "last-only", key);
  await expectSeatedIn(page, "last-only-closed");
  const closed = await request.put("/rooms/last-only-closed/settings", { headers: MASTER, data: { archived: true } });
  expect(closed.ok(), await closed.text()).toBeTruthy();
  await page.reload();
  await expect.poll(() => page.evaluate(() => state.room)).toBe("last-only-closed");
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  await expect(page.locator("#curRoom")).not.toHaveText("Lobby");
  expect(await rememberedRoom(page)).toBe("last-only-closed");
});

test("a direct message is not remembered as the loca to land in", async ({ page, request }) => {
  await provisionRooms(request, ["last-before-dm"]);
  expect((await request.post("/smasters", { headers: MASTER, data: { name: "last-bb-b" } })).ok()).toBeTruthy();
  const issued = await request.post("/smasters", { headers: MASTER, data: { name: "last-bb-a" } });
  expect(issued.ok()).toBeTruthy();
  const { token } = await issued.json();
  await signIn(page, "smaster", "last-bb-a", token);
  await expectSeatedIn(page, "iye");
  await openFromSidebar(page, "last-before-dm");
  await page.evaluate(() => openDm("last-bb-b"));
  await expect.poll(() => page.evaluate(() => isDm(state.room))).toBe(true);
  expect(await rememberedRoom(page)).toBe("last-before-dm");
  await page.reload();
  await expectSeatedIn(page, "last-before-dm");
});
