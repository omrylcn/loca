// A person's full name and icon in the browser: display only. The full name
// leads where a person is shown, the @name always stands beside it, and an
// open client follows a change live.
const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

const ROOM = "card-room";
const MASTER = { "x-admin-token": "MASTER" };

async function signInAsMaster(page, request) {
  const paired = await request.post("/pairings?ttl_hours=1", { headers: MASTER });
  const { pairing_code: pairingCode } = await paired.json();
  await page.goto("/");
  await page.locator("#masterEntryButton").click();
  await page.locator("#pairingCode").fill(pairingCode);
  await page.evaluate(room => window.doConnect(room), ROOM);
  await expect(page.locator("#whoami")).toContainText("MASTER");
}

async function newPage(browser) {
  const context = await browser.newContext();
  await context.addInitScript(() => {
    try { localStorage.setItem("loca-gs-seen", "1"); } catch (e) {}
  });
  return { context, page: await context.newPage() };
}

// A member seated in ROOM through its own session, as an agent runtime is.
async function seatMember(request, name) {
  const admitted = await request.post("/members", { headers: MASTER, data: { name, kind: "agent" } });
  expect(admitted.ok()).toBeTruthy();
  const { token } = await admitted.json();
  expect((await request.post(`/rooms/${ROOM}/invites`, { headers: MASTER, data: { name } })).ok()).toBeTruthy();
  const session = await (await request.post("/sessions", {
    headers: { "x-room-token": token }, data: { name, kind: "agent" },
  })).json();
  const base = new URL(test.info().project.use.baseURL);
  const frames = [];
  const socket = new WebSocket(`ws://${base.host}/ws?room=${ROOM}&name=${name}&type=agent`,
    ["loca.v1", `loca.session.${session.session_token}`]);
  socket.addEventListener("message", event => frames.push(JSON.parse(event.data).t));
  await expect.poll(() => frames.includes("history")).toBe(true);
  return { name, session: session.session_token, socket };
}

async function setCard(request, who, data) {
  const response = await request.put("/profile/card", { headers: { "x-session-token": who.session }, data });
  expect(response.ok(), await response.text()).toBeTruthy();
}

const seat = (page, name) => page.locator("#onlineList .omem").filter({ has: page.locator(".oname", { hasText: `@${name}` }) });
const plainSeat = (page, name) => page.locator("#onlineList .omem .oname").filter({ hasText: name });

test.beforeEach(async ({ request }) => {
  await provisionRooms(request, [ROOM]);
});

test.afterEach(async ({ request }) => {
  await request.put("/profile/card", { headers: MASTER, data: { full_name: null, icon: null } });
});

test("a full name leads beside the @name, two people stay two, and open clients follow live", async ({ browser, request }) => {
  test.setTimeout(60_000);
  // Letters only: a full name holds no digits, and one test wears an @name.
  const tag = Date.now().toString(36).replace(/\d/g, d => "abcdefghij"[d]);
  const ada = await seatMember(request, `ada-${tag}`);
  const bo = await seatMember(request, `bo-${tag}`);
  const first = await newPage(browser);
  await signInAsMaster(first.page, request);
  const second = await newPage(browser);
  await signInAsMaster(second.page, request);
  for (const { page } of [first, second]) {
    await expect(plainSeat(page, bo.name)).toHaveText(bo.name);
  }

  // Bo writes a line before any card exists.
  expect((await request.post(`/rooms/${ROOM}/messages`, {
    headers: { "x-session-token": bo.session },
    data: { sender: bo.name, sender_type: "agent", text: `bo speaks ${tag}` },
  })).ok()).toBeTruthy();
  const line = page => page.locator("#feed .row", { hasText: `bo speaks ${tag}` });
  await expect(line(first.page).locator(".sender")).toHaveText(bo.name);
  await expect(line(first.page).locator(".sendercard")).toBeHidden();

  await setCard(request, ada, { full_name: "Ada Lovelace", icon: "leaf" });
  await setCard(request, bo, { full_name: "Ada Lovelace", icon: "moon" });
  for (const { page } of [first, second]) {
    // Two seats, one full name: each wears its own @name.
    await expect(seat(page, ada.name).locator(".oname")).toContainText(`Ada Lovelace @${ada.name}`);
    await expect(seat(page, bo.name).locator(".oname")).toContainText(`Ada Lovelace @${bo.name}`);
    await expect(seat(page, ada.name).locator("svg.picon")).toHaveCount(1);
    await expect(page.locator("#onlineList .omem .fullname", { hasText: "Ada Lovelace" })).toHaveCount(2);
  }
  // The line already in the feed: full name first, the @name beside it.
  const header = line(first.page);
  await expect(header.locator(".sendercard bdi.fullname")).toHaveText("Ada Lovelace");
  await expect(header.locator(".sendercard svg.picon")).toHaveCount(1);
  await expect(header.locator(".sender")).toHaveText(bo.name);
  expect(await header.locator(".sender").evaluate(el => getComputedStyle(el, "::before").content)).toBe('"@"');

  // Bo wears Ada's @name as a full name: still visibly Bo.
  await setCard(request, bo, { full_name: ada.name });
  for (const { page } of [first, second]) {
    await expect(seat(page, bo.name).locator(".oname")).toContainText(`${ada.name} @${bo.name}`);
    await expect(seat(page, ada.name).locator(".oname")).toContainText(`Ada Lovelace @${ada.name}`);
  }
  await expect(header.locator(".sender")).toHaveText(bo.name);

  // A rank word in another script earns no rank badge.
  await setCard(request, bo, { full_name: "Мaster" });
  await expect(seat(first.page, bo.name).locator(".oname")).toContainText(`Мaster @${bo.name}`);
  await expect(seat(first.page, bo.name).locator(".rankbadge")).toHaveCount(0);

  // Mention completion offers the @name; the full name helps find it.
  const complete = text => first.page.evaluate(value => {
    const input = document.getElementById("msg");
    input.value = value;
    input.selectionStart = input.selectionEnd = value.length;
    updateMentionPop();
  }, text);
  await complete("@lovel");
  await expect(first.page.locator("#mentionPop")).toContainText(`@${ada.name}`);
  await expect(first.page.locator("#mentionPop")).toContainText("Ada Lovelace");
  await complete("");

  // Cleared: exactly the plain name again, live.
  await setCard(request, bo, { full_name: null, icon: null });
  for (const { page } of [first, second]) {
    await expect(plainSeat(page, bo.name)).toHaveText(bo.name);
  }
  await expect(header.locator(".sendercard")).toBeHidden();

  ada.socket.close();
  bo.socket.close();
  await first.context.close();
  await second.context.close();
});

test("one's own full name and icon are set from the profile menu, by keyboard too", async ({ browser, request }) => {
  const { context, page } = await newPage(browser);
  await signInAsMaster(page, request);
  await page.locator("#whoami .profilemenu > summary").click();
  const fullName = page.locator("#profileFullName");
  await expect(fullName).toBeVisible();

  // A refused value says why and changes nothing.
  await fullName.fill("Master");
  await page.locator("#profileCardSave").click();
  await expect(page.locator("#profileCardNote")).toContainText("rank or title");
  expect((await (await request.get("/profile", { headers: MASTER })).json()).principal.full_name).toBeUndefined();

  await fullName.fill("Ömer Yalçın");
  await page.locator('#profileCard input[data-profile-icon][value=""]').focus();
  await page.keyboard.press("ArrowRight");
  await expect(page.locator('#profileCard input[data-profile-icon][value="dot"]')).toBeChecked();
  await fullName.focus();
  await page.keyboard.press("Enter");
  await expect(page.locator("#profileCardNote")).toHaveText("Saved.");
  const profile = await (await request.get("/profile", { headers: MASTER })).json();
  expect(profile.principal.full_name).toBe("Ömer Yalçın");
  expect(profile.principal.icon).toBe("dot");
  await expect(page.locator("#whoami .profileidentity b")).toContainText("Ömer Yalçın @");
  // A reload keeps it.
  await page.reload();
  await expect(page.locator("#whoami .profileidentity b")).toContainText("Ömer Yalçın @");
  await context.close();
});

test("what a card carries is shown as text, never as markup or an unknown icon", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await page.goto("/");
  await page.evaluate(() => {
    window.__ran = false;
    state.members = [{
      name: "probe-agent", type: "agent",
      full_name: '<img src=x onerror="window.__ran=true">', icon: "<svg onload=alert(1)>",
    }];
    renderMembers();
  });
  const name = page.locator("#onlineList .omem .oname");
  await expect(name).toContainText('<img src=x onerror="window.__ran=true"> @probe-agent');
  await expect(page.locator("#onlineList img")).toHaveCount(0);
  await expect(page.locator("#onlineList svg.picon")).toHaveCount(0);
  expect(await page.evaluate(() => window.__ran)).toBe(false);
});

// A line wears a card only when the server says who verifiably wrote it.
test("a line nobody verified keeps its plain name when the real person arrives; theirs wear the card", async ({ browser, request }) => {
  test.setTimeout(60_000);
  const tag = Date.now().toString(36).replace(/\d/g, d => "abcdefghij"[d]);
  const name = `alice-${tag}`;
  // The shared building key's session names no principal: its "alice" is a label.
  const shared = await (await request.post("/sessions", {
    headers: { "x-room-token": "building" }, data: { name, kind: "user" },
  })).json();
  const posted = await request.post(`/rooms/${ROOM}/messages`, {
    headers: { "x-session-token": shared.session_token },
    data: { sender: name, sender_type: "user", text: `unverified ${tag}` },
  });
  expect(posted.ok(), await posted.text()).toBeTruthy();
  expect((await posted.json()).sender_principal).toBeUndefined();
  expect((await request.delete("/sessions", { headers: { "x-session-token": shared.session_token } })).ok()).toBeTruthy();

  // The real member of that name arrives with a full name and icon, and speaks.
  const alice = await seatMember(request, name);
  await setCard(request, alice, { full_name: "Alice Real", icon: "star" });
  const hers = await request.post(`/rooms/${ROOM}/messages`, {
    headers: { "x-session-token": alice.session },
    data: { sender: name, sender_type: "agent", text: `verified ${tag}` },
  });
  expect((await hers.json()).sender_principal).toMatch(/^pr_/);

  const { context, page } = await newPage(browser);
  await signInAsMaster(page, request);
  const line = text => page.locator("#feed .row", { hasText: text });
  const check = async () => {
    await expect(line(`verified ${tag}`).filter({ hasNotText: "unverified" }).locator(".sendercard bdi.fullname")).toHaveText("Alice Real");
    await expect(line(`verified ${tag}`).filter({ hasNotText: "unverified" }).locator(".sendercard svg.picon")).toHaveCount(1);
    const old = line(`unverified ${tag}`);
    await expect(old.locator(".sender")).toHaveText(name);
    await expect(old.locator(".sendercard")).toBeHidden();
    await expect(old.locator(".sendercard svg, .sendercard bdi")).toHaveCount(0);
    expect(await old.locator(".sender").evaluate(el => getComputedStyle(el, "::before").content)).toBe("none");
  };
  await check();
  await page.reload();
  await check();

  // Cleared, Alice's own line shows the plain name too, live.
  await setCard(request, alice, { full_name: null, icon: null });
  await expect(line(`verified ${tag}`).filter({ hasNotText: "unverified" }).locator(".sendercard")).toBeHidden();
  alice.socket.close();
  await context.close();
});

test("a writer who was revoked shows the plain name; two writers with one full name stay distinguishable", async ({ browser, request }) => {
  test.setTimeout(60_000);
  const tag = Date.now().toString(36).replace(/\d/g, d => "abcdefghij"[d]);
  const ada = await seatMember(request, `ada-${tag}`);
  const bo = await seatMember(request, `bo-${tag}`);
  for (const who of [ada, bo]) {
    await setCard(request, who, { full_name: "Ada Lovelace", icon: "leaf" });
    expect((await request.post(`/rooms/${ROOM}/messages`, {
      headers: { "x-session-token": who.session },
      data: { sender: who.name, sender_type: "agent", text: `said by ${who.name}` },
    })).ok()).toBeTruthy();
  }
  const { context, page } = await newPage(browser);
  await signInAsMaster(page, request);
  const header = who => page.locator("#feed .row", { hasText: `said by ${who.name}` });
  for (const who of [ada, bo]) {
    await expect(header(who).locator(".sendercard bdi.fullname")).toHaveText("Ada Lovelace");
    await expect(header(who).locator(".sender")).toHaveText(who.name);
  }

  // Bo's membership is revoked: his line keeps its words and @name, no card.
  const members = await (await request.get("/members", { headers: MASTER })).json();
  const record = members.find(m => m.name === bo.name);
  expect((await request.delete(`/members/${record.id}`, { headers: MASTER })).ok()).toBeTruthy();
  await page.reload();
  await expect(header(bo).locator(".sender")).toHaveText(bo.name);
  await expect(header(bo).locator(".sendercard")).toBeHidden();
  await expect(header(ada).locator(".sendercard bdi.fullname")).toHaveText("Ada Lovelace");
  ada.socket.close();
  bo.socket.close();
  await context.close();
});

test("the key ledger shows the name a key was issued for, never the card of who wears that name now", async ({ browser, request }) => {
  // A membership is revoked and its name admitted again as another person,
  // who sets a full name and an icon. The ledger still lists the first
  // person's revoked key under that name: it must not wear the newcomer's card.
  const name = `ledger-${Date.now().toString(36)}`;
  const first = await request.post("/members", { headers: MASTER, data: { name, kind: "agent" } });
  expect(first.ok()).toBeTruthy();
  const listed = await (await request.get("/members", { headers: MASTER })).json();
  const id = listed.find(member => member.name === name).id;
  expect((await request.delete(`/members/${id}`, { headers: MASTER })).ok()).toBeTruthy();
  const second = await seatMember(request, name);
  await setCard(request, second, { full_name: "Newcomer Person", icon: "star" });

  const { context, page } = await newPage(browser);
  await signInAsMaster(page, request);
  await page.locator("#tabPeople").click();
  await page.locator("#btabKeys").click();
  const rows = page.locator("#keyLedger .keyrow").filter({ has: page.locator(".keywho", { hasText: name }) });
  await expect.poll(() => rows.count()).toBeGreaterThan(1);
  await expect(page.locator("#keyLedger")).not.toContainText("Newcomer Person");
  await expect(page.locator("#keyLedger .keywho .fullname")).toHaveCount(0);
  await expect(page.locator("#keyLedger .keywho .picon")).toHaveCount(0);
  // The newcomer does wear the card where the server verified the seat.
  await expect(seat(page, name)).toContainText("Newcomer Person");
  second.socket.close();
  await context.close();
});
