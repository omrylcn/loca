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
  const page = await browser.newPage();
  await page.addInitScript(({ token, expiresAt, name }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name }));
  }, { token: info.session_token, expiresAt: info.expires_at, name });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  return page;
}

test("a birebir is seen by its two only; the Master sees that it exists and reads only when called in", async ({ browser, request }) => {
  const sa = await openAs(browser, await rankSession(request, "bb-sa"), "bb-sa");
  const sb = await openAs(browser, await rankSession(request, "bb-sb"), "bb-sb");
  const other = await openAs(browser, await rankSession(request, "bb-other"), "bb-other");
  const master = await openAs(browser, await rankSession(request, "MASTER"), "Master");

  // sa opens a birebir with sb: it lands under Messages, named by the peer.
  await sa.evaluate(() => openBirebir("bb-sb"));
  await expect.poll(() => sa.evaluate(() => isBirebir(state.room))).toBe(true);
  const room = await sa.evaluate(() => state.room);
  await expect(sa.locator("#curRoom")).toHaveText("bb-sb");
  await expect(sa.locator("#birebirBar")).toContainText("Only you and bb-sb see this");
  await expect(sa.locator("#birebirBar")).toContainText("Not encrypted");
  await expect(sa.locator(".tabs")).toBeHidden();
  await expect(sa.locator("#seatToggle")).toBeHidden();
  await expect(sa.locator("#roomList")).not.toContainText(room);
  await expect(sa.locator("#msg")).toHaveAttribute("placeholder", /say it to bb-sb/);
  await expect.poll(() => sa.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  await sa.fill("#msg", "before the call");
  await sa.keyboard.press("Enter");
  await expect(sa.locator("#feed")).toContainText("before the call");
  // Every word is to the other side, so no "@peer" prefix clutters the line.
  await expect(sa.locator("#feed .row", { hasText: "before the call" })).not.toContainText("@bb-sb");

  // sb finds it under Messages and reads it.
  await sb.evaluate(() => refreshRooms());
  await expect(sb.locator("#dmList")).toContainText("bb-sa");
  await sb.locator("#dmList .room.dm", { hasText: "bb-sa" }).click();
  await expect(sb.locator("#feed")).toContainText("before the call");

  // Another rank sees nothing — no Messages entry, no loca, no door.
  await other.evaluate(() => refreshRooms());
  await expect(other.locator("#dmList")).not.toContainText("bb-s");
  expect(await other.evaluate(() => state.rooms.map(r => r.room))).not.toContain(room);
  const peek = await other.evaluate(async (room) => (await fetch(`/rooms/${room}/messages`, { headers: adminHeaders({}) })).status, room);
  expect(peek).toBe(404);

  // The Master's Building view: existence only, no Read.
  await master.evaluate(() => switchTab("people"));
  await master.locator("#btabPrivate").click();
  const overview = master.locator("#birebirOverview");
  await expect(overview).toContainText("bb-sa");
  await expect(overview).toContainText("bb-sb");
  await expect(overview).not.toContainText("before the call");
  await expect(overview.locator("[data-bbread]")).toHaveCount(0);

  // sb calls the Master in; both sides see it.
  await sb.locator("#birebirBar [data-bb=call]").click();
  await expect(sb.locator("#birebirBar")).toContainText("The Master is reading");
  await expect(sa.locator("#birebirBar")).toContainText("The Master is reading");
  await expect(sa.locator("#birebirBar [data-bb=dismiss]")).toHaveCount(0);
  await sa.fill("#msg", "after the call");
  await sa.keyboard.press("Enter");
  await expect(sb.locator("#feed")).toContainText("after the call");

  // The Master reads from the call on; earlier history stays closed.
  await master.evaluate(() => fetchBirebirOverview());
  await overview.locator("[data-bbread]").click();
  await expect(master.locator("#bbWitness")).toContainText("after the call");
  await expect(master.locator("#bbWitness")).not.toContainText("before the call");

  // Both allow history; then it opens.
  await sa.locator("#birebirBar [data-bb=history]").click();
  await sb.locator("#birebirBar [data-bb=history]").click();
  await expect(sb.locator("#birebirBar")).toContainText("everything, from the start");
  await overview.locator("[data-bbread]").click();
  await expect(master.locator("#bbWitness")).toContainText("before the call");

  // The caller sends the Master out.
  await sb.locator("#birebirBar [data-bb=dismiss]").click();
  await expect(sb.locator("#birebirBar")).toContainText("Only you and bb-sa see this");
  for (const p of [sa, sb, other, master]) await p.close();
});

test("New message: a Smaster writes to another Smaster without sharing a loca", async ({ browser, request }) => {
  const sa = await openAs(browser, await rankSession(request, "nm-sa"), "nm-sa");
  await rankSession(request, "nm-sb");
  await sa.locator("#dmNew").click();
  await expect(sa.locator("#dmPicker")).toBeVisible();
  await sa.locator("#dmSearch").fill("nm-s");
  await expect(sa.locator("#dmCandidates")).toContainText("nm-sb");
  await expect(sa.locator("#dmCandidates")).not.toContainText("nm-sa");
  await sa.screenshot({ path: test.info().outputPath("dm-picker-1280.png") });
  await sa.locator("#dmCandidates [data-dm-pick='nm-sb']").click();
  await expect.poll(() => sa.evaluate(() => isBirebir(state.room))).toBe(true);
  await expect(sa.locator("#curRoom")).toHaveText("nm-sb");
  await expect(sa.locator("#dmPicker")).toBeHidden();
  await sa.close();
});

test("a Messages row menu is visible and Close asks yes/no, then removes it from my list", async ({ browser, request }) => {
  const sa = await openAs(browser, await rankSession(request, "cl-sa"), "cl-sa");
  await rankSession(request, "cl-sb");
  await sa.evaluate(() => openBirebir("cl-sb"));
  await expect(sa.locator("#dmList")).toContainText("cl-sb");
  const openMenu = async () => {
    const trigger = sa.locator("#dmList .roompreftrigger").first();
    await expect(trigger).toBeVisible();
    await trigger.click();
    await expect(sa.locator("#dmList [data-dm-delete]")).toBeVisible();
  };
  await openMenu();
  const close = sa.locator("#dmList [data-dm-delete]");
  await expect(close).toHaveText("Close");
  // The menu is really on screen: the point under its center is the menu.
  const visible = await sa.evaluate(() => {
    const m = document.querySelector("#dmList .roomprefs[open] .roomprefmenu");
    const r = m.getBoundingClientRect();
    return m.contains(document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2));
  });
  expect(visible).toBe(true);
  // "No" keeps it.
  sa.once("dialog", d => d.dismiss());
  await close.click();
  await expect(sa.locator("#dmList")).toContainText("cl-sb");
  // "Yes" closes it from my Messages and leaves the conversation.
  await openMenu();
  sa.once("dialog", d => d.accept());
  await sa.locator("#dmList [data-dm-delete]").click();
  await expect(sa.locator("#dmList")).not.toContainText("cl-sb");
  await expect.poll(() => sa.evaluate(() => isBirebir(state.room))).toBe(false);
  await sa.close();
});

test("a closed conversation returns with a new word and stays after it is read", async ({ browser, request }) => {
  const sa = await openAs(browser, await rankSession(request, "rt-sa"), "rt-sa");
  const sb = await openAs(browser, await rankSession(request, "rt-sb"), "rt-sb");
  const say = async (page, text) => {
    await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
    await page.fill("#msg", text);
    await page.keyboard.press("Enter");
    await expect(page.locator("#feed")).toContainText(text);
  };
  const closeFromList = async () => {
    await sa.locator("#dmList .roompreftrigger").first().click();
    sa.once("dialog", d => d.accept());
    await sa.locator("#dmList [data-dm-delete]").click();
    await expect(sa.locator("#dmList")).not.toContainText("rt-sb");
  };
  await sa.evaluate(() => openBirebir("rt-sb"));
  await expect.poll(() => sa.evaluate(() => isBirebir(state.room))).toBe(true);
  const room = await sa.evaluate(() => state.room);
  await say(sa, "first word");
  await sb.evaluate(() => openBirebir("rt-sa"));
  await expect.poll(() => sb.evaluate(() => state.room)).toBe(room);

  // Close it: gone from my Messages, and I am back in a loca.
  await closeFromList();
  await expect.poll(() => sa.evaluate(() => isBirebir(state.room))).toBe(false);
  expect(await sa.evaluate(room => loadRoomPreferences().hidden.includes(room), room)).toBe(true);

  // The other side writes: the conversation comes back, unread.
  await say(sb, "are you there");
  await sa.evaluate(() => refreshRooms());
  await expect(sa.locator("#dmList .unreadbadge")).toHaveText("1");
  await expect(sa.locator("#dmList")).toContainText("rt-sb");

  // Reading it does not take it away again; it was un-hidden for good.
  await sa.locator("#dmList button.dm").first().click();
  await expect.poll(() => sa.evaluate(() => state.room)).toBe(room);
  await expect(sa.locator("#feed")).toContainText("are you there");
  await expect(sa.locator("#dmList .unreadbadge")).toHaveCount(0);
  await expect(sa.locator("#dmList")).toContainText("rt-sb");
  expect(await sa.evaluate(room => loadRoomPreferences().hidden.includes(room), room)).toBe(false);
  await sa.evaluate(() => refreshRooms());
  await expect(sa.locator("#dmList")).toContainText("rt-sb");
  await sa.reload();
  await expect.poll(() => sa.evaluate(() => state.authStatus)).toBe("authenticated");
  await expect(sa.locator("#dmList")).toContainText("rt-sb");

  // Closing answers for what was already waiting: a word that arrived before
  // the close does not bring it back, only one that comes after.
  await sa.evaluate(() => joinRoom(state.homeRoom));
  await expect.poll(() => sa.evaluate(() => isBirebir(state.room))).toBe(false);
  await say(sb, "one more thing");
  await sa.evaluate(() => refreshRooms());
  await expect(sa.locator("#dmList .unreadbadge")).toHaveText("1");
  await closeFromList();
  await sa.evaluate(() => refreshRooms());
  await expect(sa.locator("#dmList")).not.toContainText("rt-sb");
  await say(sb, "and another");
  await sa.evaluate(() => refreshRooms());
  await expect(sa.locator("#dmList")).toContainText("rt-sb");
  await sa.close();
  await sb.close();
});

test("an unread count read before the close does not bring a closed conversation back", async ({ browser, request }) => {
  const sa = await openAs(browser, await rankSession(request, "hr-sa"), "hr-sa");
  const sb = await openAs(browser, await rankSession(request, "hr-sb"), "hr-sb");
  const say = async (page, text) => {
    await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
    await page.fill("#msg", text);
    await page.keyboard.press("Enter");
    await expect(page.locator("#feed")).toContainText(text);
  };
  // A refresh that is certainly this one, not a health tick already running.
  const refreshed = () => sa.evaluate(async () => {
    while (roomsRefreshing) await new Promise(r => setTimeout(r, 20));
    await refreshRooms();
  });
  const hidden = () => sa.evaluate(room => loadRoomPreferences().hidden.includes(room), room);
  await sa.evaluate(() => openBirebir("hr-sb"));
  await expect.poll(() => sa.evaluate(() => isBirebir(state.room))).toBe(true);
  const room = await sa.evaluate(() => state.room);
  await say(sa, "first word");
  await sb.evaluate(() => openBirebir("hr-sa"));
  await expect.poll(() => sb.evaluate(() => state.room)).toBe(room);

  // sa is in a loca, with everything in the conversation counted: no unread
  // count for it is asked again until something new is said there.
  await sa.evaluate(() => joinRoom(state.homeRoom));
  await expect.poll(() => sa.evaluate(() => isBirebir(state.room))).toBe(false);
  await refreshed();
  expect(await sa.evaluate(room => Number(state.unread[room] || 0), room)).toBe(0);

  // Barrier: the first unread count for the conversation is held, with its
  // answer already taken from the server, until the test lets it go. It is
  // set before sb writes, so whichever refresh asks first is the one held.
  const tailRead = url => decodeURIComponent(url.pathname) === `/rooms/${room}/messages` && url.searchParams.has("since");
  let arrived, release, held = false;
  const atBarrier = new Promise(r => { arrived = r; });
  const released = new Promise(r => { release = r; });
  await sa.route(tailRead, async route => {
    if (held) return route.fallback();
    held = true;
    const answer = await route.fetch();
    arrived(await answer.json());
    await released;
    await route.fulfill({ response: answer });
  });
  await say(sb, "are you there");
  await sa.evaluate(() => { refreshRooms(); });
  const counted = await atBarrier;
  expect(counted.map(m => m.text)).toContain("are you there");

  // sa closes the conversation while that count is on its way.
  await sa.locator("#dmList .roompreftrigger").first().click();
  sa.once("dialog", d => d.accept());
  await sa.locator("#dmList [data-dm-delete]").click();
  await expect(sa.locator("#dmList")).not.toContainText("hr-sb");
  expect(await hidden()).toBe(true);

  // The old count lands; the refresh that asked for it finishes.
  release();
  await expect.poll(() => sa.evaluate(() => roomsRefreshing)).toBe(false);
  await sa.unroute(tailRead);
  await expect(sa.locator("#dmList")).not.toContainText("hr-sb");
  expect(await hidden()).toBe(true);
  await refreshed();
  await expect(sa.locator("#dmList")).not.toContainText("hr-sb");
  expect(await hidden()).toBe(true);
  // Across a reload: the stored preference still hides it...
  const principal = await sa.evaluate(() => state.principalId);
  await sa.reload();
  await expect.poll(() => sa.evaluate(() => state.authStatus)).toBe("authenticated");
  const stored = await sa.evaluate(principal => Object.keys(localStorage)
    .filter(key => key.startsWith("loca-room-preferences:") && key.endsWith(`:${principal}`))
    .map(key => JSON.parse(localStorage.getItem(key)).hidden), principal);
  expect(stored).toEqual([[room]]);
  // ...and the list does, once the page knows whose preferences to read. (A
  // page restored without a remembered seat learns that when a loca is
  // entered; until then it shows every conversation. Not this change's.)
  await sa.evaluate(() => joinRoom(state.homeRoom));
  await expect.poll(() => sa.evaluate(() => state.principalId)).toBe(principal);
  await refreshed();
  expect(await sa.evaluate(room => state.birebirs.some(b => b.room === room), room)).toBe(true);
  await expect(sa.locator("#dmList")).not.toContainText("hr-sb");
  expect(await hidden()).toBe(true);

  // A word said after the close brings it back, and it stays once read.
  await say(sb, "a new word");
  await refreshed();
  await expect(sa.locator("#dmList .unreadbadge")).toHaveText("1");
  await expect(sa.locator("#dmList")).toContainText("hr-sb");
  await sa.locator("#dmList button.dm").first().click();
  await expect.poll(() => sa.evaluate(() => state.room)).toBe(room);
  await expect(sa.locator("#feed")).toContainText("a new word");
  await expect(sa.locator("#dmList .unreadbadge")).toHaveCount(0);
  await expect(sa.locator("#dmList")).toContainText("hr-sb");
  expect(await hidden()).toBe(false);
  await refreshed();
  await expect(sa.locator("#dmList")).toContainText("hr-sb");
  await sa.close();
  await sb.close();
});
