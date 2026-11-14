const { test, expect } = require("@playwright/test");

// A session being taken or renewed belongs to the person it was started for.
// These tests hold that one `/sessions` answer while that person leaves and
// someone else signs in at the same tab, then let it go: as a success, as a
// refusal and as a broken connection. Whatever it is, the new person's
// identity stays theirs and the last person's unsent message is not sent, or
// put back into the composer, for them.

const MASTER = { "x-admin-token": "MASTER" };

async function smasterKey(request, name) {
  const { token } = await (await request.post("/smasters", { headers: MASTER, data: { name } })).json();
  return token;
}

async function signInWithKey(page, key) {
  if (await page.locator("#connBox").isHidden()) await page.locator("#connGear").click();
  await page.locator("#entryKind").selectOption("smaster");
  await page.locator("#roomToken").fill(key);
  await page.locator("#connectBtn").click();
}

async function expectSignedInAs(page, name) {
  await expect(page.locator("body")).not.toHaveClass(/locked/);
  await expect.poll(() => page.evaluate(() => state.profile?.principal?.display_name)).toBe(name);
  await expect.poll(() => page.evaluate(() => state.profile?.building_role)).toBe("smaster");
}

// Hold the next `/sessions` exchange before any of it reaches the page.
async function holdSessionAtNetwork(page, outcome) {
  const hold = {};
  hold.arrived = new Promise(resolve => { hold.seen = resolve; });
  hold.finished = new Promise(resolve => { hold.done = resolve; });
  const released = new Promise(resolve => { hold.release = resolve; });
  let taken = false;
  await page.route("**/sessions", async route => {
    if (route.request().method() !== "POST" || taken) return route.fallback();
    taken = true;
    hold.seen();
    await released;
    if (outcome === "success") await route.continue();
    else if (outcome === "refusal") await route.fulfill({ status: 401, body: "that key opens nothing" });
    else await route.abort("connectionreset");
    hold.done();
  });
  return hold;
}

// Hold the next `/sessions` answer after its headers: fetch() has resolved
// and the page waits in r.json().
async function holdSessionAtBody(page) {
  await page.evaluate(() => {
    const gate = window.__sessionGate = { arrived: false };
    const open = new Promise(resolve => { gate.open = resolve; });
    const json = Response.prototype.json;
    let taken = false;
    Response.prototype.json = async function () {
      if (!taken && new URL(this.url).pathname === "/sessions") {
        taken = true;
        gate.arrived = true;
        await open;
      }
      return json.call(this);
    };
  });
  return {
    arrived: expect.poll(() => page.evaluate(() => window.__sessionGate.arrived), { timeout: 15_000 }).toBe(true),
    release: () => page.evaluate(() => window.__sessionGate.open()),
    finished: Promise.resolve(),
  };
}

const CASES = [
  { hold: "network", outcome: "success", end: "sign-out" },
  { hold: "network", outcome: "refusal", end: "sign-out" },
  { hold: "network", outcome: "broken connection", end: "sign-out" },
  { hold: "body", outcome: "success", end: "sign-out" },
  // No sign-out at all: another key is typed in over a session that is being
  // renewed. Its status reads "unknown" while that is out, like a page that
  // is still loading; it is not one.
  { hold: "network", outcome: "success", end: "typed-over" },
];

for (const [index, c] of CASES.entries()) {
test(`a session renewal held at the ${c.hold} and ending in ${c.outcome} leaves the next person alone (${c.end})`, async ({ browser, request }) => {
  test.setTimeout(90_000);
  const tag = `str${index}`;
  const OLD_TEXT = `${tag}-unsent-line-4c1e`;
  const oldKey = await smasterKey(request, `${tag}-old`);
  await smasterKey(request, `${tag}-peer`);
  const nextKey = await smasterKey(request, `${tag}-next`);

  const context = await browser.newContext();
  const page = await context.newPage();
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  const dialogs = [];
  page.on("dialog", dialog => { dialogs.push(dialog.message()); dialog.dismiss().catch(() => {}); });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("unauthenticated");

  // The first person signs in with a key and writes in a private conversation.
  await signInWithKey(page, oldKey);
  await expectSignedInAs(page, `${tag}-old`);
  // The key stays in memory for this page: that is what a renewal exchanges.
  expect(await page.evaluate(() => !!state.roomToken)).toBe(true);
  await page.evaluate(peer => openBirebir(peer), `${tag}-peer`);
  await expect.poll(() => page.evaluate(() => isBirebir(state.room))).toBe(true);
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);

  // Their next message is answered 401 once, as after a server restart; the
  // page renews the session, and that renewal is what is held.
  const posts = [];
  await page.route("**/rooms/*/messages", async route => {
    const request = route.request();
    if (request.method() !== "POST" || !(request.postData() || "").includes(OLD_TEXT)) return route.fallback();
    posts.push(request.headers()["x-session-token"] || "");
    if (posts.length === 1) return route.fulfill({ status: 401, body: "session ended" });
    return route.fulfill({ status: 403, body: "not yours" });
  });
  const hold = c.hold === "network"
    ? await holdSessionAtNetwork(page, c.outcome)
    : await holdSessionAtBody(page);
  await page.fill("#msg", OLD_TEXT);
  await page.keyboard.press("Enter");
  await hold.arrived;
  expect(posts.length).toBe(1);
  expect(await page.evaluate(() => state.authStatus)).toBe("unknown");

  // They leave (or are typed over), and the next person signs in.
  if (c.end === "sign-out") {
    if (await page.locator("#connBox").isHidden()) await page.locator("#connGear").click();
    await page.locator("#leaveBtn").click();
    await page.locator("#leaveYes").click();
    await expect(page.locator("#doorline")).toContainText("you left loca.");
  }
  await signInWithKey(page, nextKey);
  await expectSignedInAs(page, `${tag}-next`);
  const mine = await page.evaluate(() => ({ session: state.session, name: state.name }));
  expect(mine.name).toBe(`${tag}-next`);
  expect(mine.session).toBeTruthy();

  // Now the held answer arrives.
  await hold.release();
  await hold.finished;
  // Longer than the one safe retry a send makes (350 ms) and its answer.
  await page.evaluate(() => new Promise(resolve => setTimeout(resolve, 1200)));

  const now = await page.evaluate(() => ({
    session: state.session, name: state.name, auth: state.authStatus, admin: state.adminSession,
    locked: document.body.classList.contains("locked"),
    nameField: document.getElementById("name").value,
    composer: document.getElementById("msg").value,
    seat: JSON.parse(localStorage.getItem("loca-seat") || "null")?.name,
    profile: state.profile?.principal?.display_name,
    html: document.documentElement.outerHTML,
  }));
  // The identity is still the next person's, whole.
  expect(now.session).toBe(mine.session);
  expect(now.name).toBe(`${tag}-next`);
  expect(now.nameField).toBe(`${tag}-next`);
  expect(now.profile).toBe(`${tag}-next`);
  expect(now.seat).toBe(`${tag}-next`);
  expect({ auth: now.auth, admin: now.admin, locked: now.locked })
    .toEqual({ auth: "authenticated", admin: true, locked: false });
  // The last person's message was tried once, by them, and never again; it
  // is not in the next person's composer, page or a dialog either.
  expect(posts.length).toBe(1);
  expect(posts).not.toContain(mine.session);
  expect(now.composer).not.toContain(OLD_TEXT);
  expect(now.html).not.toContain(OLD_TEXT);
  expect(dialogs).toEqual([]);
  // And the session works: the next person reads their own Building.
  expect(await page.evaluate(async () => (await fetch("/profile", { headers: adminHeaders({}) })).status)).toBe(200);

  await context.close();
});
}

test("pressing connect twice exchanges a one-use pairing code once", async ({ browser, request }) => {
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("unauthenticated");
  let exchanges = 0;
  await page.route("**/sessions", route => {
    if (route.request().method() === "POST") exchanges++;
    return route.fallback();
  });
  await page.evaluate(code => {
    setEntryPath("master");
    document.getElementById("pairingCode").value = code;
    doConnect(state.homeRoom);
    doConnect(state.homeRoom);
  }, pairing_code);
  await expect.poll(() => page.evaluate(() => state.profile?.building_role)).toBe("master");
  expect(exchanges).toBe(1);
  expect(await page.evaluate(() => ({ auth: state.authStatus, admin: state.adminSession, locked: document.body.classList.contains("locked") })))
    .toEqual({ auth: "authenticated", admin: true, locked: false });
  await context.close();
});
