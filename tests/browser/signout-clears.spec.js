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
  // Each person in a browser of their own: no shared storage between them.
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.addInitScript(({ token, expiresAt, name }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name }));
  }, { token: info.session_token, expiresAt: info.expires_at, name });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  return page;
}

test("signing out without a reload leaves no key, witnessed birebir or Building data in the page", async ({ browser, request }) => {
  const PRIVATE = "so-private-line-7f3a";
  const sa = await openAs(browser, await rankSession(request, "so-sa"), "so-sa");
  await rankSession(request, "so-sb");
  const masterInfo = await rankSession(request, "MASTER");
  const sent = await request.post("/groups", { headers: { "x-session-token": masterInfo.session_token }, data: { name: "so-group" } });
  expect(sent.ok()).toBeTruthy();

  // A private conversation the Master is called in to read.
  await sa.evaluate(() => openBirebir("so-sb"));
  await expect.poll(() => sa.evaluate(() => isBirebir(state.room))).toBe(true);
  await expect.poll(() => sa.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  await sa.locator("#birebirBar [data-bb=call]").click();
  await expect(sa.locator("#birebirBar")).toContainText("The Master is reading");
  await sa.fill("#msg", PRIVATE);
  await sa.keyboard.press("Enter");
  await expect(sa.locator("#feed")).toContainText(PRIVATE);

  const master = await openAs(browser, masterInfo, "Master");
  await master.evaluate(() => doConnect(state.homeRoom));
  await expect(master.locator("#leaveBtn")).not.toHaveClass(/hidden/);
  await master.evaluate(() => switchTab("people"));
  await expect.poll(() => master.evaluate(() => state.profile?.building_role)).toBe("master");
  await expect(master.locator("#peopleList")).toContainText("so-sa");

  // The Master issues a Smaster key: the secret is on screen once...
  await master.locator("#btabKeys").click();
  await master.locator("#keyKind").selectOption("smaster");
  await master.locator("#keyForm button[type=submit]").click();
  await expect(master.locator("#keyIssued")).toContainText("copy it now");
  const key = (await master.locator("#keySecret").textContent()).trim();
  expect(key.length).toBeGreaterThan(8);
  // ...and reads the birebir it was called into.
  await master.locator("#btabPrivate").click();
  await expect(master.locator("#birebirOverview [data-bbread]")).toHaveCount(1);
  await master.locator("#birebirOverview [data-bbread]").click();
  await expect(master.locator("#bbWitness")).toContainText(PRIVATE);

  const held = () => master.evaluate(() => ({
    keyLedger: state.keyLedger.length,
    people: (state.people || []).length,
    ranks: (state.ranks || []).length,
    joinRequests: Array.isArray(state.joinRequests),
    birebirOverview: state.birebirOverview.length,
    groups: state.groups.length,
    groupsLoaded: state.groupsLoaded,
    profilesList: (state.profilesList || []).length,
    notif: state.notif.loaded,
    keyIssued: document.getElementById("keyIssued")?.textContent || "",
    bbWitness: document.getElementById("bbWitness")?.textContent || "",
  }));
  await expect.poll(() => master.evaluate(() => state.notif.loaded), { timeout: 15_000 }).toBe(true);
  const before = await held();
  expect(before.keyLedger).toBeGreaterThan(0);
  expect(before.ranks).toBeGreaterThan(0);
  expect(before.joinRequests).toBe(true);
  expect(before.birebirOverview).toBeGreaterThan(0);
  expect(before.groups).toBeGreaterThan(0);
  expect(before.profilesList).toBeGreaterThan(0);
  expect(before.keyIssued).toContain(key);
  expect(before.bbWitness).toContain(PRIVATE);

  // Sign out. The page is not reloaded.
  await master.evaluate(() => { window.__sameDocument = true; });
  await master.locator("#connGear").click();
  await master.locator("#leaveBtn").click();
  await master.locator("#leaveYes").click();
  await expect(master.locator("#doorline")).toContainText("you left loca.");
  await expect(master.locator("body")).toHaveClass(/locked/);
  expect(await master.evaluate(() => window.__sameDocument)).toBe(true);

  expect(await held()).toEqual({
    keyLedger: 0, people: 0, ranks: 0, joinRequests: false, birebirOverview: 0,
    groups: 0, groupsLoaded: false, profilesList: 0, notif: false, keyIssued: "", bbWitness: "",
  });
  const page = await master.evaluate(() => document.documentElement.outerHTML);
  expect(page).not.toContain(key);
  expect(page).not.toContain(PRIVATE);
  expect(await master.evaluate(() => state.notif.items.length)).toBe(0);
  for (const pane of ["#peopleList", "#groupsPane", "#keyLedger", "#birebirOverview"]) {
    await expect(master.locator(pane)).toBeEmpty();
  }

  // The next person at this tab signs in: the Building view is drawn afresh
  // from what the server gives them, with nothing carried over.
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  await master.locator("#masterEntryButton").click();
  await master.locator("#pairingCode").fill(pairing_code);
  await master.locator("#connectBtn").click();
  await expect(master.locator("body")).not.toHaveClass(/locked/);
  await master.evaluate(() => switchTab("people"));
  await expect(master.locator("#peopleList")).toBeVisible();
  await expect(master.locator("#peopleList")).toContainText("so-sa");
  await master.locator("#btabKeys").click();
  await expect(master.locator("#keyLedger")).toContainText("Key generator");
  await expect(master.locator("#keyIssued")).toBeHidden();
  await expect(master.locator("#keyIssued")).toBeEmpty();
  await master.locator("#btabPrivate").click();
  await expect(master.locator("#birebirOverview")).toContainText("so-sa");
  await expect(master.locator("#bbWitness")).toBeEmpty();
  await master.locator("#btabGroups").click();
  await expect(master.locator("#groupsPane")).toContainText("so-group");

  await master.context().close();
  await sa.context().close();
});

// ---- Answers still in flight at sign-out --------------------------------
// The clear above runs once. A Building request sent before it and answered
// after it must not bring the last person's data back for the next one.

// The Master at one tab with all a Building view holds: the key ledger, a
// group, a birebir it was called in to read, and its notifications.
async function masterAtWork(browser, request, tag) {
  const PRIVATE = `${tag}-private-line`;
  const sa = await openAs(browser, await rankSession(request, `${tag}-sa`), `${tag}-sa`);
  await rankSession(request, `${tag}-sb`);
  const masterInfo = await rankSession(request, "MASTER");
  const made = await request.post("/groups", { headers: { "x-session-token": masterInfo.session_token }, data: { name: `${tag}-old-group` } });
  expect(made.ok()).toBeTruthy();
  await sa.evaluate((peer) => openBirebir(peer), `${tag}-sb`);
  await expect.poll(() => sa.evaluate(() => isBirebir(state.room))).toBe(true);
  await expect.poll(() => sa.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  await sa.locator("#birebirBar [data-bb=call]").click();
  await expect(sa.locator("#birebirBar")).toContainText("The Master is reading");
  await sa.fill("#msg", PRIVATE);
  await sa.keyboard.press("Enter");
  await expect(sa.locator("#feed")).toContainText(PRIVATE);
  const room = await sa.evaluate(() => state.room);

  const master = await openAs(browser, masterInfo, "Master");
  await master.evaluate(() => doConnect(state.homeRoom));
  await expect(master.locator("#leaveBtn")).not.toHaveClass(/hidden/);
  await master.evaluate(() => switchTab("people"));
  await expect.poll(() => master.evaluate(() => state.profile?.building_role)).toBe("master");
  await master.locator("#btabPrivate").click();
  await expect(master.locator(`#birebirOverview [data-bbread="${room}"]`)).toHaveCount(1);
  await master.locator("#btabKeys").click();
  await expect(master.locator("#keyLedger")).toContainText("Key generator");
  await expect.poll(() => master.evaluate(() => state.groups.map(g => g.name))).toContain(`${tag}-old-group`);
  await expect.poll(() => master.evaluate(() => state.notif.loaded), { timeout: 15_000 }).toBe(true);
  // Read is in the Private pane, hidden behind Keys now: click it in the DOM.
  const readWitness = () => master.evaluate((r) => { document.querySelector(`#birebirOverview [data-bbread="${r}"]`).click(); }, room);
  return { master, sa, PRIVATE, readWitness };
}

// A notification only the old answer carries: it can come from nowhere else.
const oldNotice = (tag) => ({
  id: 9_000_001, title: `${tag}-old-notice`, body: "", from: "Master",
  audience: { all: true }, created_at: Date.now(), read: false,
});
const withOldNotice = (tag) => (page) => ({ ...page, notifications: [oldNotice(tag), ...(page.notifications || [])] });

// Hold point 1, at the network: the server answers the first matching request
// at once (still for the old session), but the page's fetch() does not
// resolve until the test releases it — after sign-out and the next sign-in.
function holdAtNetwork(page, pattern, method, edit) {
  const hold = {};
  hold.arrived = new Promise(resolve => { hold.seen = resolve; });
  hold.fulfilled = new Promise(resolve => { hold.done = resolve; });
  const released = new Promise(resolve => { hold.release = resolve; });
  let taken = false;
  page.route(pattern, async route => {
    if (taken || route.request().method() !== method) return route.fallback();
    taken = true;
    const response = await route.fetch();
    hold.body = await response.json();
    hold.seen();
    await released;
    await route.fulfill({ response, json: edit ? edit(hold.body) : hold.body });
    hold.done();
  });
  return hold;
}

// Hold point 2, at the body: fetch() has resolved with its headers, and the
// first r.json() on each matching path waits until the test opens the gate.
// This is the second await of every fetcher.
async function holdBodies(page, paths) {
  await page.evaluate((sources) => {
    const wanted = sources.map(s => new RegExp(s));
    const gate = window.__bodyGate = { arrived: [] };
    const open = new Promise(resolve => { gate.open = resolve; });
    const json = Response.prototype.json;
    Response.prototype.json = async function () {
      const url = new URL(this.url);
      const i = wanted.findIndex(re => re.test(url.pathname + url.search));
      if (i >= 0) { gate.arrived.push(wanted.splice(i, 1)[0].source); await open; }
      return json.call(this);
    };
  }, paths);
}

async function settle(page) {
  await page.evaluate(() => new Promise(resolve => setTimeout(resolve, 250)));
}

// The next person: a Smaster with a group of its own.
async function nextSmaster(request, name) {
  const { token } = await (await request.post("/smasters", { headers: MASTER, data: { name } })).json();
  const s = await (await request.post("/sessions", { headers: { "x-admin-token": token }, data: { name, kind: "user" } })).json();
  const made = await request.post("/groups", { headers: { "x-session-token": s.session_token }, data: { name: `${name}-own-group` } });
  expect(made.ok()).toBeTruthy();
  return token;
}

// The session ends one of two ways: the sign-out button, or the server
// answering 401 to the session (the /rooms read refreshRooms makes, answered
// 401 by page.route for the Master's token). In the 401 case a second /rooms
// read, sent while the door is shut, is held and answered 401 only after the
// next person has signed in: a late 401 that must not close their session.
async function endSession(page, end) {
  if (end === "sign-out") {
    await page.locator("#connGear").click();
    await page.locator("#leaveBtn").click();
    await page.locator("#leaveYes").click();
    await expect(page.locator("#doorline")).toContainText("you left loca.");
    return null;
  }
  const oldToken = await page.evaluate(() => state.session);
  const late = {};
  late.arrived = new Promise(resolve => { late.seen = resolve; });
  late.fulfilled = new Promise(resolve => { late.done = resolve; });
  const released = new Promise(resolve => { late.release = resolve; });
  let lateTaken = false;
  await page.route("**/rooms", async route => {
    if (route.request().method() !== "GET") return route.fallback();
    const token = route.request().headers()["x-session-token"];
    if (token === oldToken) return route.fulfill({ status: 401, body: "session ended" });
    if (token || lateTaken) return route.fallback();
    lateTaken = true;
    late.seen();
    await released;
    await route.fulfill({ status: 401, body: "no session" });
    late.done();
  });
  await page.evaluate(() => { refreshRooms(); });
  await expect(page.locator("body")).toHaveClass(/locked/, { timeout: 15_000 });
  await expect(page.locator("#doorline")).toContainText("your session ended");
  await page.evaluate(() => { refreshRooms(); });
  await late.arrived;
  return late;
}

async function endThenSignInAsSmaster(page, end, token, name) {
  const late = await endSession(page, end);
  await page.locator("#entryKind").selectOption("smaster");
  await page.locator("#roomToken").fill(token);
  await page.locator("#connectBtn").click();
  await expect(page.locator("body")).not.toHaveClass(/locked/);
  await expect.poll(() => page.evaluate(() => state.profile?.building_role)).toBe("smaster");
  await page.evaluate(() => switchTab("people"));
  await page.locator("#btabGroups").click();
  await expect(page.locator("#groupsPane")).toContainText(`${name}-own-group`);
  return late;
}

async function releaseLate(page, late) {
  if (!late) return;
  late.release();
  await late.fulfilled;
  await settle(page);
}

async function expectNothingCameBack(page, { tag, name, PRIVATE, secret }) {
  const now = await page.evaluate(() => ({
    role: state.profile?.building_role,
    keyLedger: state.keyLedger.length,
    birebirOverview: state.birebirOverview.length,
    groups: state.groups.map(g => g.name),
    notices: state.notif.items.map(x => x.title),
    keyLedgerPane: document.getElementById("keyLedger").innerHTML,
    overviewPane: document.getElementById("birebirOverview").innerHTML,
    groupsPane: document.getElementById("groupsPane").textContent,
    html: document.documentElement.outerHTML,
  }));
  expect(now.keyLedger).toBe(0);
  expect(now.keyLedgerPane).toBe("");
  expect(now.birebirOverview).toBe(0);
  expect(now.overviewPane).toBe("");
  expect(now.groups).not.toContain(`${tag}-old-group`);
  expect(now.notices).not.toContain(`${tag}-old-notice`);
  expect(now.html).not.toContain(PRIVATE);
  expect(now.html).not.toContain(`${tag}-old-group`);
  expect(now.html).not.toContain(`${tag}-old-notice`);
  if (secret) expect(now.html).not.toContain(secret);
  // ...and what the next person has of their own is intact.
  expect(now.role).toBe("smaster");
  expect(now.groups).toContain(`${name}-own-group`);
  expect(now.groupsPane).toContain(`${name}-own-group`);
  // Still signed in: no late answer closed the new session.
  expect(await page.evaluate(() => ({
    locked: document.body.classList.contains("locked"), auth: state.authStatus, session: !!state.session,
  }))).toEqual({ locked: false, auth: "authenticated", session: true });
  await expect.poll(() => page.evaluate(() => state.notif.loaded && state.notif.principal === state.session), { timeout: 15_000 }).toBe(true);
  expect(await page.evaluate(() => state.notif.items.map(x => x.title))).not.toContain(`${tag}-old-notice`);
}

for (const end of ["sign-out", "401"]) {
test(`Building answers held at the network across ${end} and the next sign-in bring nothing back`, async ({ browser, request }) => {
  test.setTimeout(120_000);
  const tag = `race-net-${end}`;
  const { master, sa, PRIVATE, readWitness } = await masterAtWork(browser, request, tag);

  const issued = holdAtNetwork(master, "**/keys", "POST");
  const ledger = holdAtNetwork(master, "**/keys", "GET");
  const overview = holdAtNetwork(master, "**/building/birebirs", "GET");
  const witness = holdAtNetwork(master, "**/building/birebirs/*/messages*", "GET");
  const groups = holdAtNetwork(master, "**/groups", "GET");
  const notices = holdAtNetwork(master, "**/notifications?*", "GET", withOldNotice(tag));

  await master.locator("#keyKind").selectOption("smaster");
  await master.locator("#keyForm button[type=submit]").click();
  await issued.arrived;
  await master.evaluate(() => { fetchKeyLedger(); });
  await ledger.arrived;
  await readWitness();
  await witness.arrived;
  await master.evaluate(() => { fetchBirebirOverview(); });
  await overview.arrived;
  await master.evaluate(() => { fetchGroups(); });
  await groups.arrived;
  await master.evaluate(() => { pollNotifications(); });
  await notices.arrived;
  const secret = issued.body.code;
  expect(secret.length).toBeGreaterThan(8);
  expect(ledger.body.length).toBeGreaterThan(0);
  expect(witness.body.messages.map(m => m.text)).toContain(PRIVATE);
  expect(groups.body.map(g => g.name)).toContain(`${tag}-old-group`);

  const name = `${tag}-next`;
  const token = await nextSmaster(request, name);
  const late = await endThenSignInAsSmaster(master, end, token, name);

  // Released in the order that would rebuild the most: the panes first, then
  // what writes into them.
  for (const hold of [overview, ledger, witness, issued, groups, notices]) {
    hold.release();
    await hold.fulfilled;
    await settle(master);
  }
  await releaseLate(master, late);
  await expectNothingCameBack(master, { tag, name, PRIVATE, secret });

  await master.context().close();
  await sa.context().close();
});
}

for (const end of ["sign-out", "401"]) {
test(`Building answers whose body is held across ${end} and the next sign-in bring nothing back`, async ({ browser, request }) => {
  test.setTimeout(120_000);
  const tag = `race-body-${end}`;
  const { master, sa, PRIVATE, readWitness } = await masterAtWork(browser, request, tag);

  // The old notifications carry a marker; the body read is what is held.
  let marking = true;
  await master.route("**/notifications?*", async route => {
    if (!marking) return route.fallback();
    const response = await route.fetch();
    await route.fulfill({ response, json: withOldNotice(tag)(await response.json()) });
  });
  const paths = ["^/keys$", "^/building/birebirs$", "^/building/birebirs/[^/]+/messages(\\?|$)", "^/groups$", "^/notifications\\?"];
  await holdBodies(master, paths);
  await readWitness();
  await master.evaluate(() => {
    fetchKeyLedger();
    fetchBirebirOverview();
    fetchGroups();
    pollNotifications();
  });
  await expect.poll(() => master.evaluate(() => window.__bodyGate.arrived.length), { timeout: 15_000 }).toBe(paths.length);
  marking = false;

  const name = `${tag}-next`;
  const token = await nextSmaster(request, name);
  const late = await endThenSignInAsSmaster(master, end, token, name);

  await master.evaluate(() => window.__bodyGate.open());
  await settle(master);
  await releaseLate(master, late);
  await expectNothingCameBack(master, { tag, name, PRIVATE });

  await master.context().close();
  await sa.context().close();
});
}
