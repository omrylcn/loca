const { test, expect } = require("@playwright/test");
const { MASTER } = require("./support/switch");
const { provisionRooms } = require("./support/building");

// The Messages section of the sidebar, walked as its two people use it: a
// private conversation (a direct message in the code) is conversation only, its
// people wear their cards, and nothing on screen says "loca" or "birebir"
// about it.

// A member at a loca with the others named; returns its key.
async function member(request, name, room, kind = "user") {
  const r = await request.post("/members", { headers: MASTER, data: { name, kind } });
  expect(r.status(), await r.text()).toBe(201);
  const { token } = await r.json();
  if (room) expect((await request.post(`/rooms/${room}/invites`, { headers: MASTER, data: { name } })).ok()).toBeTruthy();
  return token;
}

async function sessionOf(request, name, key) {
  const r = await request.post("/sessions", { headers: { "x-room-token": key }, data: { name, kind: "user" } });
  expect(r.ok(), await r.text()).toBeTruthy();
  return { "x-session-token": (await r.json()).session_token };
}

async function openAsMember(browser, name, key) {
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await page.goto("/");
  await page.locator("#name").fill(name);
  await page.locator("#roomToken").fill(key);
  await page.locator("#connectBtn").click();
  await expect(page.locator("body")).not.toHaveClass(/locked/);
  await expect.poll(() => page.evaluate(() => state.principalId)).toBeTruthy();
  return page;
}

// A refresh that is certainly this one, not a health tick already running.
async function refreshed(page) {
  await page.evaluate(async () => {
    while (roomsRefreshing) await new Promise(r => setTimeout(r, 20));
    await refreshRooms();
  });
}

async function openConversation(page, peer) {
  await page.evaluate(peer => openDm(peer), peer);
  await expect.poll(() => page.evaluate(() => isDm(state.room))).toBe(true);
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  return page.evaluate(() => state.room);
}

test("the first word of a conversation just opened counts as unread for the other side", async ({ browser, request }) => {
  const room = "ms-unread-table";
  await provisionRooms(request, [room]);
  const aKey = await member(request, "ms-un-a", room);
  const bKey = await member(request, "ms-un-b", room);
  const b = await openAsMember(browser, "ms-un-b", bKey);
  // b's page knows its rooms already, and refreshes only when told to.
  await b.evaluate(() => { clearInterval(healthTimer); });
  await refreshed(b);
  await expect(b.locator("#dmSection")).toHaveClass(/empty/);

  // a opens the conversation and says the first word between two of b's refreshes.
  const a = await sessionOf(request, "ms-un-a", aKey);
  const opened = await (await request.post("/birebirs", { headers: a, data: { name: "ms-un-b" } })).json();
  const conversation = opened.birebir.room;
  expect((await request.post(`/rooms/${conversation}/messages`, {
    headers: a, data: { sender: "ms-un-a", sender_type: "user", text: "the very first word" },
  })).ok()).toBeTruthy();

  await refreshed(b);
  await expect(b.locator("#dmList")).toContainText("ms-un-a");
  await expect(b.locator("#dmList .unreadbadge")).toHaveText("1");
  await b.locator("#dmList button.dm").first().click();
  await expect(b.locator("#feed")).toContainText("the very first word");
  await expect(b.locator("#dmList .unreadbadge")).toHaveCount(0);
  await b.context().close();
});

test("inside a conversation nothing presents it as a loca", async ({ browser, request }) => {
  const room = "ms-noloca-table";
  await provisionRooms(request, [room]);
  const aKey = await member(request, "ms-nl-a", room);
  await member(request, "ms-nl-b", room);
  const a = await openAsMember(browser, "ms-nl-a", aKey);
  const conversation = await openConversation(a, "ms-nl-b");
  // No This Loca view: a conversation has no operator, lead, role or id to show.
  await expect(a.locator("#sideLocaTab")).toBeDisabled();
  await expect(a.locator("#sideLocaTab")).toHaveAttribute("aria-selected", "false");
  await a.evaluate(() => setSidebarView("loca"));
  await expect(a.locator("#sideLocaView")).toBeHidden();
  await expect(a.locator("#locaSummary")).not.toContainText(conversation);
  // The header says whether the other person is here, not a head count at a table.
  await expect(a.locator("#subStatus")).toHaveText("ms-nl-b is not here now");
  await expect(a.locator("#feed")).toContainText("nothing said yet");
  await expect(a.locator("#feed")).not.toContainText("table");
  // A loca is a loca again after it.
  await a.evaluate(room => joinRoom(room), room);
  await expect(a.locator("#sideLocaTab")).toBeEnabled();
  await expect(a.locator("#subStatus")).toContainText("at the table");
  await a.context().close();
});

test("@ completion in a conversation offers the other person only", async ({ browser, request }) => {
  const room = "ms-mention-table";
  await provisionRooms(request, [room]);
  const aKey = await member(request, "ms-mn-a", room);
  await member(request, "ms-mn-b", room);
  const a = await openAsMember(browser, "ms-mn-a", aKey);
  await openConversation(a, "ms-mn-b");
  // No @all (it cannot widen the conversation), no loca agent (nothing in a
  // conversation is carried to it).
  await a.locator("#msg").click();
  await a.keyboard.type("@");
  await expect(a.locator("#mentionPop")).toBeVisible();
  await expect(a.locator("#mentionPop")).toContainText("@ms-mn-b");
  await expect(a.locator("#mentionPop")).not.toContainText("@all");
  await expect(a.locator("#mentionPop")).not.toContainText("@loca-care");
  await a.keyboard.press("Escape");
  // In a loca, everyone is offered again.
  await a.evaluate(room => joinRoom(room), room);
  await expect.poll(() => a.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  await a.fill("#msg", "");
  await a.locator("#msg").click();
  await a.keyboard.type("@");
  await expect(a.locator("#mentionPop")).toContainText("@all");
  await a.context().close();
});

test("the other person's full name and icon show in Messages, the picker and on their lines", async ({ browser, request }) => {
  const room = "ms-card-table";
  await provisionRooms(request, [room]);
  const aKey = await member(request, "ms-card-a", room);
  const bKey = await member(request, "ms-card-b", room);
  const b = await sessionOf(request, "ms-card-b", bKey);
  expect((await request.put("/profile/card", { headers: b, data: { full_name: "Bora Kayaçelik", icon: "star" } })).ok()).toBeTruthy();
  const a = await openAsMember(browser, "ms-card-a", aKey);

  // The picker shows the card and finds the person by a word of the full name.
  await a.locator("#dmNew").click();
  await a.locator("#dmSearch").fill("kayacel");
  await expect(a.locator("#dmCandidates [data-dm-pick]")).toHaveCount(1);
  await expect(a.locator("#dmCandidates [data-dm-pick='ms-card-b']")).toContainText("Bora Kayaçelik");
  await expect(a.locator("#dmCandidates [data-dm-pick='ms-card-b'] svg.picon")).toHaveCount(1);
  await a.locator("#dmCandidates [data-dm-pick='ms-card-b']").click();
  await expect.poll(() => a.evaluate(() => isDm(state.room))).toBe(true);
  const conversation = await a.evaluate(() => state.room);
  await expect(a.locator("#dmList .room.dm")).toContainText("Bora Kayaçelik");
  await expect(a.locator("#dmList .room.dm")).toContainText("@ms-card-b");

  // b is not seated in a's page, and a is a plain member with no Building
  // lists: b's line still wears b's card.
  expect((await request.post(`/rooms/${conversation}/messages`, {
    headers: b, data: { sender: "ms-card-b", sender_type: "user", text: "a word from b" },
  })).ok()).toBeTruthy();
  const line = a.locator("#feed .row", { hasText: "a word from b" });
  await expect(line.locator(".sendercard")).toContainText("Bora Kayaçelik");
  await a.context().close();
});

test("a conversation whose other side left the building is read-only before anyone types", async ({ browser, request }) => {
  const room = "ms-gone-table";
  await provisionRooms(request, [room]);
  const aKey = await member(request, "ms-gone-a", room);
  await member(request, "ms-gone-b", room);
  const a = await openAsMember(browser, "ms-gone-a", aKey);
  await openConversation(a, "ms-gone-b");
  await expect(a.locator("#msg")).toBeEnabled();
  const people = await (await request.get("/members", { headers: MASTER })).json();
  const gone = people.find(p => p.name === "ms-gone-b");
  expect((await request.delete(`/members/${gone.id}`, { headers: MASTER })).ok()).toBeTruthy();
  await a.evaluate(() => fetchDms());
  await expect(a.locator("#dmBar")).toContainText("Read-only");
  await expect(a.locator("#msg")).toBeDisabled();
  await expect(a.locator("#sendBtn")).toBeDisabled();
  await expect(a.locator("#msg")).toHaveAttribute("placeholder", /no longer a member/);
  await a.context().close();
});

test("both sides see who called the Master in", async ({ browser, request }) => {
  const room = "ms-caller-table";
  await provisionRooms(request, [room]);
  const aKey = await member(request, "ms-cw-a", room);
  const bKey = await member(request, "ms-cw-b", room);
  const a = await openAsMember(browser, "ms-cw-a", aKey);
  const conversation = await openConversation(a, "ms-cw-b");

  // b calls the Master in: a's bar names who did.
  const b = await sessionOf(request, "ms-cw-b", bKey);
  expect((await request.post(`/birebirs/${conversation}/master`, { headers: b, data: { action: "call" } })).ok()).toBeTruthy();
  await expect(a.locator("#dmBar")).toContainText("ms-cw-b called the Master in");
  await a.locator("#dmBar [data-bb=history]").click();
  await expect(a.locator("#dmBar")).toContainText("you allowed earlier history");
  // b, who called, sends the Master out: the suite's server is shared, and the
  // Master's overview elsewhere counts the conversations it was called into.
  expect((await request.post(`/birebirs/${conversation}/master`, { headers: b, data: { action: "dismiss" } })).ok()).toBeTruthy();
  await expect(a.locator("#dmBar")).not.toContainText("called the Master in");
  await a.context().close();
});

test("a conversation the Master closed says so in its own words", async ({ browser, request }) => {
  const room = "ms-closed-table";
  await provisionRooms(request, [room]);
  const aKey = await member(request, "ms-cl-a", room);
  await member(request, "ms-cl-b", room);
  const a = await openAsMember(browser, "ms-cl-a", aKey);
  const conversation = await openConversation(a, "ms-cl-b");
  // The Master closes it: read-only, said in the conversation's own words.
  expect((await request.post(`/building/birebirs/${conversation}/close`, { headers: MASTER, data: { archived: true } })).ok()).toBeTruthy();
  await expect(a.locator("#msg")).toBeDisabled();
  await expect(a.locator("#dmBar")).toContainText("Closed by the Master");
  await expect(a.locator("#msg")).toHaveAttribute("placeholder", "the Master closed this conversation — read-only");
  await a.context().close();
});

test("no refusal or hint calls a private conversation a direct message", async ({ browser, request }) => {
  const room = "ms-word-table";
  await provisionRooms(request, [room]);
  const aKey = await member(request, "ms-word-a", room);
  const bKey = await member(request, "ms-word-b", room);
  await member(request, "ms-word-lone");
  const a = await openAsMember(browser, "ms-word-a", aKey);
  await a.evaluate(room => joinRoom(room), room);
  await expect.poll(() => a.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  // Nobody at a table with the lone member: the refusal is said plainly.
  await a.evaluate(() => openDm("ms-word-lone"));
  await expect(a.locator("#feed")).toContainText("could not open a conversation with ms-word-lone");
  await expect(a.locator("#feed")).not.toContainText("birebir");
  // The message button beside a person at the table.
  const b = await openAsMember(browser, "ms-word-b", bKey);
  await b.evaluate(room => joinRoom(room), room);
  await a.evaluate(() => setSidebarView("loca"));
  const dm = a.locator("#onlineList .dmbtn").first();
  await expect(dm).toHaveAttribute("data-tip-detail", /private conversation/);
  for (const page of [a, b]) await page.context().close();
});

async function masterPage(browser, request) {
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const m = await (await request.post("/sessions", { headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" } })).json();
  const page = await (await browser.newContext()).newPage();
  await page.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "Master" }));
  }, { token: m.session_token, expiresAt: m.expires_at });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  return { page, m };
}

test("the new-message picker is used with the keyboard", async ({ browser, request }) => {
  for (const name of ["ms-kb-zulu", "ms-kb-alfa", "ms-kb-mike"]) await member(request, name);
  const { page } = await masterPage(browser, request);
  await page.locator("#dmNew").click();
  await expect(page.locator("#dmSearch")).toBeFocused();
  await page.keyboard.type("ms-kb-");
  await expect(page.locator("#dmCandidates [data-dm-pick]")).toHaveCount(3);
  await page.keyboard.press("ArrowDown");
  await expect(page.locator("#dmCandidates [data-dm-pick]").nth(0)).toBeFocused();
  await page.keyboard.press("ArrowDown");
  await expect(page.locator("#dmCandidates [data-dm-pick]").nth(1)).toBeFocused();
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("ArrowUp");
  await expect(page.locator("#dmSearch")).toBeFocused();
  await page.keyboard.press("ArrowDown");
  // Escape from a person closes the picker, back on its button.
  await page.keyboard.press("Escape");
  await expect(page.locator("#dmPicker")).toBeHidden();
  await expect(page.locator("#dmNew")).toBeFocused();
  await page.context().close();
});

test("conversations where nothing was said yet are listed by name", async ({ browser, request }) => {
  for (const name of ["ms-qo-zulu", "ms-qo-alfa", "ms-qo-mike"]) await member(request, name);
  const { page, m } = await masterPage(browser, request);
  // Three conversations, nothing said in any: listed by name, not by chance.
  for (const name of ["ms-qo-zulu", "ms-qo-alfa", "ms-qo-mike"]) {
    expect((await request.post("/birebirs", { headers: { "x-session-token": m.session_token }, data: { name } })).ok()).toBeTruthy();
  }
  await refreshed(page);
  const names = await page.locator("#dmList [data-dm-peer]").evaluateAll(rows => rows.map(row => row.dataset.dmPeer));
  expect(names.filter(n => n.startsWith("ms-qo-"))).toEqual(["ms-qo-alfa", "ms-qo-mike", "ms-qo-zulu"]);
  await page.context().close();
});

test("signing out leaves no private words in the page", async ({ browser, request }) => {
  const room = "ms-out-table";
  await provisionRooms(request, [room]);
  const aKey = await member(request, "ms-out-a", room);
  await member(request, "ms-out-b", room);
  const a = await openAsMember(browser, "ms-out-a", aKey);
  await openConversation(a, "ms-out-b");
  await a.fill("#msg", "only for the two of us");
  await a.keyboard.press("Enter");
  await expect(a.locator("#feed")).toContainText("only for the two of us");
  await expect(async () => {
    if (!(await a.locator("#whoami details.profilemenu").evaluate(menu => menu.open))) {
      await a.locator("#whoami .profilemenu > summary").click({ timeout: 2000 });
    }
    await a.locator("#leaveBtn").click({ timeout: 2000 });
    await a.locator("#leaveYes").click({ timeout: 2000 });
  }).toPass({ timeout: 15_000 });
  await expect(a.locator("#doorline")).toContainText("you signed out.");
  expect(await a.evaluate(() => document.getElementById("feed").textContent)).not.toContain("only for the two of us");
  expect(await a.evaluate(() => state.msgs.length)).toBe(0);
  await a.context().close();
});

// What a person wrote or read must not outlive their time at the page: not in
// the feed, not in memory. A session that renews is the same person carrying on.
async function wordsLeft(page) {
  return page.evaluate(() => ({
    feed: document.getElementById("feed").textContent,
    kept: state.msgs.length,
  }));
}

test("a session the server ends for good leaves no private words; one that renews keeps the conversation", async ({ browser, request }) => {
  const room = "ms-end-table";
  await provisionRooms(request, [room]);
  const aKey = await member(request, "ms-end-a", room);
  await member(request, "ms-end-b", room);
  const a = await openAsMember(browser, "ms-end-a", aKey);
  await a.evaluate(() => { clearInterval(healthTimer); });
  await openConversation(a, "ms-end-b");
  await a.fill("#msg", "kept while I am still me");
  await a.keyboard.press("Enter");
  await expect(a.locator("#feed")).toContainText("kept while I am still me");

  // The session is ended, the key still opens the door: the page renews and
  // the conversation is where it was.
  const first = await a.evaluate(() => state.session);
  expect((await request.delete("/sessions", { headers: { "x-session-token": first } })).status()).toBe(204);
  await refreshed(a);
  await expect.poll(() => a.evaluate(() => state.session)).not.toBe(first);
  await expect(a.locator("body")).not.toHaveClass(/locked/);
  await expect(a.locator("#feed")).toContainText("kept while I am still me");

  // The membership itself ends: the key is refused, and nothing said stays.
  const listed = await (await request.get("/members", { headers: MASTER })).json();
  const id = listed.find(m => m.name === "ms-end-a").id;
  expect((await request.delete(`/members/${id}`, { headers: MASTER })).ok()).toBeTruthy();
  await refreshed(a);
  await expect(a.locator("body")).toHaveClass(/locked/);
  const left = await wordsLeft(a);
  expect(left.feed).not.toContain("kept while I am still me");
  expect(left.kept).toBe(0);
  await a.context().close();
});

test("a Master session that ended leaves none of what was on screen", async ({ browser, request }) => {
  const room = "ms-end-master";
  await provisionRooms(request, [room]);
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await page.goto("/");
  await page.locator("#masterEntryButton").click();
  await page.locator("#pairingCode").fill(pairing_code);
  await page.evaluate(room => window.doConnect(room), room);
  await expect(page.locator("#whoami")).toContainText("MASTER");
  await page.evaluate(() => { clearInterval(healthTimer); });
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  // Something said in the loca the Master is looking at.
  const key = await member(request, "ms-end-speaker", room);
  const speaker = await sessionOf(request, "ms-end-speaker", key);
  expect((await request.post(`/rooms/${room}/messages`, {
    headers: speaker, data: { sender: "ms-end-speaker", sender_type: "user", text: "said in front of the Master" },
  })).ok()).toBeTruthy();
  await expect(page.locator("#feed")).toContainText("said in front of the Master");

  const session = await page.evaluate(() => state.session);
  expect((await request.delete("/sessions", { headers: { "x-session-token": session } })).status()).toBe(204);
  await refreshed(page);
  await expect(page.locator("body")).toHaveClass(/locked/);
  const left = await wordsLeft(page);
  expect(left.feed).not.toContain("said in front of the Master");
  expect(left.kept).toBe(0);
  await context.close();
});
