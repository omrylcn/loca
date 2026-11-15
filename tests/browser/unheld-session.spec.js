const { test, expect } = require("@playwright/test");
const { MASTER, smasterKey, freshDoor, openSignInForm, signInWithKey, expectSignedInAs, signOut, settle, expectStill } = require("./support/switch");

// A session nobody at the tab holds must not stay on the server. The page
// ends it itself, at the server that granted it, with that session's own
// token and nothing else, and whatever becomes of that clean-up is nobody's
// business at the tab: the person now at it is not touched.
//
// First: a sign-in to one server answered after the tab was pointed at
// another.
//
// Two servers: the test server under its own address (B, the page's origin)
// and the same server under another name (A, http://localhost:<port>). The
// page reaches A cross-origin, so A is served through page.route: every
// request to A is recorded exactly as the page sent it and forwarded to the
// real server, and its answer is handed back with the CORS header a real
// cross-origin server would add (Playwright answers the preflight of a routed
// request itself). The session A grants is a real one.
//
// The page a Building serves only connects to that Building (CSP
// connect-src 'self'); the page that can be pointed at another server is the
// desktop shell's, which sets no CSP. The browser context here lifts the CSP
// to stand in for it.

async function shellDoor(browser) {
  const context = await browser.newContext({ bypassCSP: true });
  const page = await context.newPage();
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  const dialogs = [];
  page.on("dialog", dialog => { dialogs.push(dialog.message()); dialog.dismiss().catch(() => {}); });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("unauthenticated");
  return { context, page, dialogs };
}

async function serveA(page, origin, a) {
  const seen = [];
  const gates = {};
  const gate = (name) => {
    const g = {};
    g.arrived = new Promise(resolve => { g.seen = resolve; });
    g.released = new Promise(resolve => { g.release = resolve; });
    gates[name] = g;
    return g;
  };
  const sessionGate = gate("post");
  const deleteGate = gate("delete");
  const cors = { "access-control-allow-origin": origin, vary: "origin" };
  await page.route(`${a}/**`, async route => {
    const sent = route.request();
    const method = sent.method();
    seen.push({ method, url: sent.url(), headers: { ...sent.headers() } });
    const toReal = sent.url().replace(a, origin);
    if (method === "POST" && new URL(sent.url()).pathname === "/sessions") {
      // The server answers; the answer is held on its way back to the page.
      const response = await route.fetch({ url: toReal });
      const body = await response.json();
      sessionGate.token = body.session_token;
      sessionGate.seen();
      await sessionGate.released;
      return route.fulfill({ status: response.status(), json: body, headers: cors }).catch(() => {});
    }
    if (method === "DELETE" && new URL(sent.url()).pathname === "/sessions") {
      deleteGate.seen();
      const outcome = await deleteGate.released;
      if (outcome === "network error") return route.abort("connectionreset").catch(() => {});
      if (outcome === "401") return route.fulfill({ status: 401, body: "invalid or expired session", headers: cors }).catch(() => {});
      if (outcome === "500") return route.fulfill({ status: 500, body: "boom", headers: cors }).catch(() => {});
      const response = await route.fetch({ url: toReal });
      deleteGate.status = response.status();
      return route.fulfill({ status: response.status(), body: await response.text(), headers: cors }).catch(() => {});
    }
    const response = await route.fetch({ url: toReal });
    return route.fulfill({ status: response.status(), body: await response.body(), headers: { ...cors, "content-type": response.headers()["content-type"] || "text/plain" } }).catch(() => {});
  });
  return { seen, gates };
}

// What the page tells whoever is at the tab, recorded from now on: the door
// line, the join-request line, the connection status and system lines in the
// conversation. Reaching "connecting…" and "connected" is not a warning.
async function watchWarnings(page) {
  await page.evaluate(() => {
    const log = window.__warnings = [];
    const quiet = new Set(["connecting…", "connected"]);
    new MutationObserver(records => {
      for (const r of records) {
        const el = r.target.nodeType === 1 ? r.target : r.target.parentElement;
        for (const n of r.addedNodes) if (n.nodeType === 1 && n.classList.contains("sysline")) log.push(`feed: ${n.textContent}`);
        const line = el && el.closest("#doorline, #joinRequestStatus");
        if (line) log.push(`${line.id}: ${line.textContent}`);
      }
    }).observe(document.body, { subtree: true, childList: true, characterData: true });
    new MutationObserver(records => {
      for (const r of records) {
        const label = r.target.getAttribute("aria-label");
        if (!quiet.has(label)) log.push(`connDot: ${label}`);
      }
    }).observe(document.getElementById("connDot"), { attributes: true, attributeFilter: ["aria-label"] });
  });
}

const OUTCOMES = ["answered", "network error", "401", "500"];

for (const outcome of OUTCOMES) {
test(`a sign-in answered after the tab moved to another server is ended there (clean-up: ${outcome})`, async ({ browser, request, baseURL }) => {
  test.setTimeout(60_000);
  const tag = `uhs${OUTCOMES.indexOf(outcome)}`;
  const keyA = await smasterKey(request, `${tag}-a`);
  const keyB = await smasterKey(request, `${tag}-b`);
  const origin = new URL(baseURL).origin;
  const a = origin.replace("127.0.0.1", "localhost");
  expect(a).not.toBe(origin);

  const { context, page, dialogs } = await shellDoor(browser);
  const errors = [];
  page.on("pageerror", error => errors.push(String(error)));
  const toB = [];
  page.on("request", sent => {
    if (sent.url().startsWith(origin + "/sessions")) toB.push({ method: sent.method(), headers: { ...sent.headers() } });
  });
  const { seen, gates } = await serveA(page, origin, a);

  // Sign in to A; A grants a session, and its answer is held.
  await page.locator("#server").fill(a);
  await signInWithKey(page, keyA);
  await gates.post.arrived;
  const tokenA = gates.post.token;
  expect(tokenA).toMatch(/^st_/);
  expect((await request.get("/whoami", { headers: { "x-session-token": tokenA } })).status()).toBe(200);

  // Before it is read, the tab is pointed at B. Then A's answer arrives.
  await page.locator("#server").fill(origin);
  gates.post.release();

  // The page ends A's session at A, with A's token and nothing else.
  await expect.poll(() => seen.filter(r => r.method === "DELETE").length, { timeout: 10_000 }).toBe(1);
  await gates.delete.arrived;
  const deletes = seen.filter(r => r.method === "DELETE");
  expect(deletes.length).toBe(1);
  expect(deletes[0].url).toBe(`${a}/sessions`);
  expect(deletes[0].headers["x-session-token"]).toBe(tokenA);
  for (const other of ["x-room-token", "x-pairing-code", "x-admin-token", "authorization"]) {
    expect(deletes[0].headers[other]).toBeUndefined();
  }
  expect(await page.evaluate(() => state.session)).toBeNull();

  // While that clean-up is still out, the person signs in to B.
  await signInWithKey(page, keyB);
  await expectSignedInAs(page, `${tag}-b`);
  const mine = await page.evaluate(() => ({ session: state.session, name: state.name }));
  expect(mine.session).not.toBe(tokenA);
  await watchWarnings(page);

  // Now the clean-up comes back, as the outcome says.
  gates.delete.release(outcome);
  await settle(page, 1200);

  if (outcome === "answered") {
    expect(gates.delete.status).toBe(204);
    expect((await request.get("/whoami", { headers: { "x-session-token": tokenA } })).status()).toBe(401);
  }
  // B is untouched: the same session, still valid at the server, and the page
  // still B's. Nothing was sent to B about A, and nothing was reported.
  expect((await request.get("/whoami", { headers: { "x-session-token": mine.session } })).status()).toBe(200);
  await expectStill(page, mine);
  expect(await page.evaluate(() => state.server)).toBe(origin);
  expect(toB.filter(r => r.method === "DELETE")).toEqual([]);
  expect(seen.filter(r => r.method === "DELETE").length).toBe(1);
  expect(await page.evaluate(() => window.__warnings)).toEqual([]);
  expect(await page.locator("#joinRequestStatus").textContent()).toBe("");
  expect(dialogs).toEqual([]);
  expect(errors).toEqual([]);
  await context.close();
});
}

// The rest run against the one test server. Every DELETE /sessions the page
// sends is recorded as sent.
function recordDeletes(page) {
  const deletes = [];
  page.on("request", sent => {
    if (sent.method() === "DELETE" && new URL(sent.url()).pathname === "/sessions") {
      deletes.push({ url: sent.url(), headers: { ...sent.headers() } });
    }
  });
  return deletes;
}

function expectOnlyItsOwnToken(del, origin, token) {
  expect(del.url).toBe(`${origin}/sessions`);
  expect(del.headers["x-session-token"]).toBe(token);
  for (const other of ["x-room-token", "x-pairing-code", "x-admin-token", "authorization"]) {
    expect(del.headers[other]).toBeUndefined();
  }
}

async function whoamiStatus(request, token) {
  return (await request.get("/whoami", { headers: { "x-session-token": token } })).status();
}

async function retakeSeat(page) {
  await openSignInForm(page);
  await page.locator("#connectBtn").click();
}

// A sign-in whose answer is still on its way when its person leaves, or when
// another key is typed in over theirs. The server has made the session; the
// page must learn its token and end it, not apply it.
const OVERTAKEN = [
  { hold: "network", end: "sign-out" },
  { hold: "body", end: "sign-out" },
  { hold: "network", end: "typed-over" },
  { hold: "body", end: "typed-over" },
];
for (const [index, c] of OVERTAKEN.entries()) {
test(`a sign-in answered after someone else took the tab is ended on the server (held at the ${c.hold}, ${c.end})`, async ({ browser, request, baseURL }) => {
  test.setTimeout(60_000);
  const tag = `uho${index}`;
  const firstKey = await smasterKey(request, `${tag}-first`);
  const nextKey = await smasterKey(request, `${tag}-next`);
  const origin = new URL(baseURL).origin;
  const { context, page, dialogs } = await freshDoor(browser);
  const errors = [];
  page.on("pageerror", error => errors.push(String(error)));
  const deletes = recordDeletes(page);

  await signInWithKey(page, firstKey);
  await expectSignedInAs(page, `${tag}-first`);
  const before = await page.evaluate(() => state.session);

  // The first person takes their seat again with the same key: a fresh
  // exchange, whose answer is held after the server has made the session.
  const held = {};
  held.arrived = new Promise(resolve => { held.seen = resolve; });
  const released = new Promise(resolve => { held.release = resolve; });
  await page.route("**/sessions", async route => {
    if (route.request().method() !== "POST" || held.token) return route.fallback();
    const response = await route.fetch();
    const body = await response.json();
    held.token = body.session_token;
    if (c.hold === "body") {
      return route.fulfill({ response, json: body, headers: { ...response.headers(), "x-held": "1" } });
    }
    held.seen();
    await released;
    await route.fulfill({ response, json: body }).catch(() => {});
  });
  if (c.hold === "body") {
    await page.evaluate(() => {
      const gate = window.__sessionGate = { arrived: false };
      const open = new Promise(resolve => { gate.open = resolve; });
      const json = Response.prototype.json;
      Response.prototype.json = async function () {
        if (this.headers.get("x-held") === "1" && !gate.arrived) { gate.arrived = true; await open; }
        return json.call(this);
      };
    });
  }
  await retakeSeat(page);
  if (c.hold === "body") await expect.poll(() => page.evaluate(() => window.__sessionGate.arrived), { timeout: 15_000 }).toBe(true);
  else await held.arrived;
  expect(held.token).toMatch(/^st_/);
  expect(held.token).not.toBe(before);
  expect(await whoamiStatus(request, held.token)).toBe(200);

  // Someone else takes the tab.
  if (c.end === "sign-out") await signOut(page);
  await signInWithKey(page, nextKey);
  await expectSignedInAs(page, `${tag}-next`);
  const mine = await page.evaluate(() => ({ session: state.session, name: state.name }));
  await watchWarnings(page);

  // Now the held answer arrives.
  if (c.hold === "body") await page.evaluate(() => window.__sessionGate.open());
  else held.release();
  await settle(page, 1200);

  // The session that answer made is ended at the server, with its own token.
  await expect.poll(() => whoamiStatus(request, held.token), { timeout: 10_000 }).toBe(401);
  const ended = deletes.filter(d => d.headers["x-session-token"] === held.token);
  expect(ended.length).toBe(1);
  expectOnlyItsOwnToken(ended[0], origin, held.token);
  expect(deletes.some(d => d.headers["x-session-token"] === mine.session)).toBe(false);
  // The next person is untouched.
  expect(await whoamiStatus(request, mine.session)).toBe(200);
  await expectStill(page, mine);
  expect(await page.evaluate(() => window.__warnings)).toEqual([]);
  expect(dialogs).toEqual([]);
  expect(errors).toEqual([]);
  await context.close();
});
}

test("leaving ends the session at the server even if the next person signs in before that is sent", async ({ browser, request, baseURL }) => {
  test.setTimeout(60_000);
  const tag = "uhl";
  const firstKey = await smasterKey(request, `${tag}-first`);
  const nextKey = await smasterKey(request, `${tag}-next`);
  const origin = new URL(baseURL).origin;
  const { context, page, dialogs } = await freshDoor(browser);
  const errors = [];
  page.on("pageerror", error => errors.push(String(error)));
  const deletes = recordDeletes(page);
  await signInWithKey(page, firstKey);
  await expectSignedInAs(page, `${tag}-first`);
  const leaver = await page.evaluate(() => state.session);

  // The leave's DELETE is held on its way out.
  const held = {};
  held.arrived = new Promise(resolve => { held.seen = resolve; });
  const released = new Promise(resolve => { held.release = resolve; });
  await page.route("**/sessions", async route => {
    if (route.request().method() !== "DELETE" || held.taken) return route.fallback();
    held.taken = true;
    held.seen();
    await released;
    await route.continue().catch(() => {});
  });
  await signOut(page);
  await held.arrived;

  // The next person signs in while it is out; then it goes on.
  await signInWithKey(page, nextKey);
  await expectSignedInAs(page, `${tag}-next`);
  const mine = await page.evaluate(() => ({ session: state.session, name: state.name }));
  await watchWarnings(page);
  held.release();
  await settle(page, 1200);

  await expect.poll(() => whoamiStatus(request, leaver), { timeout: 10_000 }).toBe(401);
  expect(deletes.length).toBe(1);
  expectOnlyItsOwnToken(deletes[0], origin, leaver);
  expect(await whoamiStatus(request, mine.session)).toBe(200);
  await expectStill(page, mine);
  expect(await page.evaluate(() => window.__warnings)).toEqual([]);
  expect(dialogs).toEqual([]);
  expect(errors).toEqual([]);
  await context.close();
});

test("a key typed in over a live session ends that session at the server", async ({ browser, request, baseURL }) => {
  const tag = "uht";
  const firstKey = await smasterKey(request, `${tag}-first`);
  const nextKey = await smasterKey(request, `${tag}-next`);
  const origin = new URL(baseURL).origin;
  const { context, page, dialogs } = await freshDoor(browser);
  const deletes = recordDeletes(page);
  await signInWithKey(page, firstKey);
  await expectSignedInAs(page, `${tag}-first`);
  const first = await page.evaluate(() => state.session);

  await signInWithKey(page, nextKey);
  await expectSignedInAs(page, `${tag}-next`);
  const mine = await page.evaluate(() => ({ session: state.session, name: state.name }));

  await expect.poll(() => whoamiStatus(request, first), { timeout: 10_000 }).toBe(401);
  expect(deletes.length).toBe(1);
  expectOnlyItsOwnToken(deletes[0], origin, first);
  expect(await whoamiStatus(request, mine.session)).toBe(200);
  await settle(page);
  await expectStill(page, mine);
  expect(dialogs).toEqual([]);
  await context.close();
});

test("taking one's seat again ends the session it replaces", async ({ browser, request, baseURL }) => {
  const tag = "uhr";
  const key = await smasterKey(request, `${tag}-one`);
  const origin = new URL(baseURL).origin;
  const { context, page } = await freshDoor(browser);
  const deletes = recordDeletes(page);
  await signInWithKey(page, key);
  await expectSignedInAs(page, `${tag}-one`);
  const replaced = await page.evaluate(() => state.session);

  await retakeSeat(page);
  // While the seat is being taken again the page holds no session for a
  // moment: wait for the new one, not merely for the old one to be gone.
  await expect.poll(() => page.evaluate(r => !!state.session && state.session !== r, replaced)).toBe(true);
  await expectSignedInAs(page, `${tag}-one`);
  const mine = await page.evaluate(() => ({ session: state.session, name: state.name }));
  expect(mine.session).toBeTruthy();

  await expect.poll(() => whoamiStatus(request, replaced), { timeout: 10_000 }).toBe(401);
  expect(deletes.length).toBe(1);
  expectOnlyItsOwnToken(deletes[0], origin, replaced);
  expect(await whoamiStatus(request, mine.session)).toBe(200);
  await settle(page);
  await expectStill(page, mine);
  await context.close();
});

// Guard: the Master session this browser keeps is still held (by the browser,
// for its next load and its other tabs) when a key is typed in over it, so it
// is not ended.
test("a key typed in over the Master session this browser keeps does not end it", async ({ browser, request }) => {
  const tag = "uhm";
  const nextKey = await smasterKey(request, `${tag}-next`);
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const { context, page } = await freshDoor(browser);
  const deletes = recordDeletes(page);
  await page.evaluate(code => {
    setEntryPath("master");
    document.getElementById("pairingCode").value = code;
  }, pairing_code);
  await page.locator("#connectBtn").click();
  await expect.poll(() => page.evaluate(() => state.profile?.building_role)).toBe("master");
  const master = await page.evaluate(() => state.session);
  expect(await page.evaluate(() => JSON.parse(localStorage.getItem("loca-admin-session")).token)).toBe(master);

  await openSignInForm(page);
  await page.locator("#masterEntryButton").click();
  await signInWithKey(page, nextKey);
  await expectSignedInAs(page, `${tag}-next`);
  await settle(page);
  expect(deletes).toEqual([]);
  expect(await whoamiStatus(request, master)).toBe(200);
  await context.close();
});

// The first person's exchange is held as long as the test likes; the next
// person's sign-in does not wait for it, and whatever that first answer turns
// out to be, nothing of it reaches the next person: not their state, not the
// exchange they have in flight (sessionTaking), not the one they finished.
const LATE = ["success", "refusal", "network error", "unreadable body"];
for (const [index, outcome] of LATE.entries()) {
for (const order of ["after the next sign-in finished", "while the next sign-in is in flight"]) {
test(`a first sign-in answered late (${outcome}) ${order} leaves the next person alone`, async ({ browser, request }) => {
  test.setTimeout(60_000);
  const tag = `uhw${index}${order.startsWith("after") ? "a" : "b"}`;
  const firstKey = await smasterKey(request, `${tag}-first`);
  const nextKey = await smasterKey(request, `${tag}-next`);
  const { context, page, dialogs } = await freshDoor(browser);
  const errors = [];
  page.on("pageerror", error => errors.push(String(error)));
  const deletes = recordDeletes(page);

  const gates = {};
  for (const who of ["first", "next"]) {
    gates[who] = { posts: 0 };
    gates[who].arrived = new Promise(resolve => { gates[who].seen = resolve; });
    gates[who].released = new Promise(resolve => { gates[who].release = resolve; });
  }
  const holdNext = order.startsWith("while");
  await page.route("**/sessions", async route => {
    const sent = route.request();
    if (sent.method() !== "POST") return route.fallback();
    const key = sent.headers()["x-room-token"];
    const who = key === firstKey ? "first" : key === nextKey ? "next" : null;
    if (!who) return route.fallback();
    const g = gates[who];
    g.posts++;
    if (who === "next" && !holdNext) return route.fallback();
    if (who === "first" && outcome === "success") {
      const response = await route.fetch();
      const body = await response.json();
      g.token = body.session_token;
      g.seen();
      await g.released;
      return route.fulfill({ response, json: body }).catch(() => {});
    }
    g.seen();
    await g.released;
    if (who === "next") return route.continue().catch(() => {});
    if (outcome === "refusal") return route.fulfill({ status: 401, body: "that key opens nothing" }).catch(() => {});
    if (outcome === "network error") return route.abort("connectionreset").catch(() => {});
    return route.fulfill({ status: 201, contentType: "application/json", body: "{not json" }).catch(() => {});
  });

  // The first person signs in; their exchange is held indefinitely.
  await signInWithKey(page, firstKey);
  await gates.first.arrived;

  // The next person signs in over it.
  await signInWithKey(page, nextKey);
  if (holdNext) {
    await gates.next.arrived;
    expect(await page.evaluate(k => !!sessionTaking && sessionTaking.key.includes(k), nextKey)).toBe(true);
    await watchWarnings(page);
    // The first answer comes back while the next one is still out.
    gates.first.release();
    await settle(page, 800);
    const between = await page.evaluate(k => ({
      ours: !!sessionTaking && sessionTaking.key.includes(k),
      session: state.session, auth: state.authStatus,
      locked: document.body.classList.contains("locked"),
    }), nextKey);
    // Nothing of it was applied, and the next person's exchange is still theirs.
    expect(between).toEqual({ ours: true, session: null, auth: "unknown", locked: true });
    // And it said nothing. (Checked here: what the next person's own first
    // render writes once they are in is theirs.)
    expect(await page.evaluate(() => window.__warnings)).toEqual([]);
    // Asking again joins the exchange in flight instead of starting another.
    await page.evaluate(() => { takeSession(); });
    expect(gates.next.posts).toBe(1);
    gates.next.release();
    await expectSignedInAs(page, `${tag}-next`);
  } else {
    // The next person is in and usable while the first answer is still held.
    await expectSignedInAs(page, `${tag}-next`);
    expect(await page.evaluate(async () => (await fetch("/profile", { headers: adminHeaders({}) })).status)).toBe(200);
    expect(await page.evaluate(() => sessionTaking)).toBeNull();
    await watchWarnings(page);
    gates.first.release();
  }
  const mine = await page.evaluate(() => ({ session: state.session, name: state.name }));
  await settle(page, 1200);

  expect(await page.evaluate(() => sessionTaking)).toBeNull();
  await expectStill(page, mine);
  expect(await whoamiStatus(request, mine.session)).toBe(200);
  if (outcome === "success") {
    await expect.poll(() => whoamiStatus(request, gates.first.token), { timeout: 10_000 }).toBe(401);
    const ended = deletes.filter(d => d.headers["x-session-token"] === gates.first.token);
    expect(ended.length).toBe(1);
  }
  expect(deletes.some(d => d.headers["x-session-token"] === mine.session)).toBe(false);
  expect(gates.first.posts).toBe(1);
  expect(gates.next.posts).toBe(1);
  if (!holdNext) expect(await page.evaluate(() => window.__warnings)).toEqual([]);
  expect(dialogs).toEqual([]);
  expect(errors).toEqual([]);
  await context.close();
});
}
}

// A message sent under one session, answered after the person took their
// seat again (a new session, same person, same generation). A 401 for the
// session it went out with says nothing about the new one: the page sends the
// message again under the new session, with the same op_id, and takes no
// third session that would replace the new one and leave it on the server.
const LATE_ANSWERS = ["401", "network error", "503", "401 while the new session is being taken"];
for (const [index, answer] of LATE_ANSWERS.entries()) {
test(`a message sent before taking one's seat again, answered ${answer}, is sent once under the new session and strands nothing`, async ({ browser, request }) => {
  test.setTimeout(60_000);
  const tag = `uhs4${index}`;
  const TEXT = `${tag}-late-line-6e1a`;
  const key = await smasterKey(request, `${tag}-one`);
  const { context, page, dialogs } = await freshDoor(browser);
  const errors = [];
  page.on("pageerror", error => errors.push(String(error)));
  await signInWithKey(page, key);
  await expectSignedInAs(page, `${tag}-one`);
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  const room = await page.evaluate(() => state.room);
  const s1 = await page.evaluate(() => state.session);

  // The message goes out under S1 and its answer is held.
  const posts = [];
  const msg = {};
  msg.arrived = new Promise(resolve => { msg.seen = resolve; });
  const msgReleased = new Promise(resolve => { msg.release = resolve; });
  await page.route("**/rooms/*/messages", async route => {
    const sent = route.request();
    if (sent.method() !== "POST" || !(sent.postData() || "").includes(TEXT)) return route.fallback();
    posts.push({ session: sent.headers()["x-session-token"] || "", opId: JSON.parse(sent.postData()).op_id });
    if (posts.length > 1) return route.fallback();
    msg.seen();
    const how = await msgReleased;
    if (how === "network error") return route.abort("connectionreset").catch(() => {});
    if (how === "503") return route.fulfill({ status: 503, body: "upstream lost" }).catch(() => {});
    return route.fulfill({ status: 401, body: "session ended" }).catch(() => {});
  });
  // Every session exchange from here on is counted; the re-take's can be held.
  const exchanges = [];
  const take = {};
  take.arrived = new Promise(resolve => { take.seen = resolve; });
  const takeReleased = new Promise(resolve => { take.release = resolve; });
  const holdTake = answer.startsWith("401 while");
  await page.route("**/sessions", async route => {
    if (route.request().method() !== "POST") return route.fallback();
    exchanges.push(route.request().headers()["x-room-token"] || "");
    if (!holdTake || exchanges.length > 1) return route.fallback();
    take.seen();
    await takeReleased;
    await route.fallback();
  });
  await page.fill("#msg", TEXT);
  await page.keyboard.press("Enter");
  await msg.arrived;

  // The same person takes their seat again: S1 ends, S2 is taken.
  await retakeSeat(page);
  if (holdTake) {
    await take.arrived;
    expect(await page.evaluate(() => !!sessionTaking && state.session === null)).toBe(true);
    // The 401 for S1 arrives while S2 is still being taken.
    msg.release("401");
    await settle(page, 600);
    expect(exchanges.length).toBe(1);
    take.release();
  }
  await expect.poll(() => page.evaluate(() => state.session)).not.toBe(s1);
  await expect.poll(() => page.evaluate(() => state.session)).toBeTruthy();
  await expectSignedInAs(page, `${tag}-one`);
  const s2 = await page.evaluate(() => state.session);
  if (!holdTake) msg.release(answer);

  // The message is sent once more, under S2, with the same op_id, and is on
  // the server exactly once.
  const onServer = async () => {
    const list = await (await request.get(`/rooms/${encodeURIComponent(room)}/messages?since=0&limit=1000`, { headers: { "x-session-token": s2 } })).json();
    return list.filter(m => m.text === TEXT).length;
  };
  await expect.poll(onServer, { timeout: 10_000 }).toBe(1);
  await settle(page, 1000);
  expect(await onServer()).toBe(1);
  expect(posts.length).toBe(2);
  expect(posts[0].session).toBe(s1);
  expect(posts[1].session).toBe(s2);
  expect(posts[1].opId).toBe(posts[0].opId);

  // S2 is still the page's, and the only session taken: nothing stranded.
  expect(await page.evaluate(() => state.session)).toBe(s2);
  expect(exchanges.length).toBe(1);
  expect(await whoamiStatus(request, s2)).toBe(200);
  expect(await whoamiStatus(request, s1)).toBe(401);
  expect(await page.evaluate(() => document.getElementById("msg").value)).not.toContain(TEXT);
  await expectStill(page, { session: s2, name: `${tag}-one` });
  expect(dialogs).toEqual([]);
  expect(errors).toEqual([]);
  await context.close();
});
}

// A loca that refuses a session is not a session that ended. A member signed
// in with their membership key loses only this loca's invite: the session is
// still valid, so it is not replaced (and nothing is left behind on the
// server), and the person is told what happened.
//
// This needs an invite-only Building (REQUIRE_INVITE=1, no ROOM_TOKEN), as in
// production: the suite's server sets a building key, and a session taken
// without a loca then reaches every loca, so no loca would refuse it. The
// test starts its own server of the same build for that.
const { spawn } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

async function inviteOnlyServer() {
  const port = Number(process.env.PLAYWRIGHT_PORT || "18787") + 101;
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "loca-invite-only-"));
  const child = spawn(path.join(__dirname, "../../target/debug/room-server"), [], {
    env: { ...process.env, PORT: String(port), BIND_ADDR: "127.0.0.1", ADMIN_TOKEN: "MASTER",
      DB_PATH: path.join(dir, "loca.sqlite3"), REQUIRE_INVITE: "1", REQUIRE_SESSIONS: "1",
      LEGACY_WS_QUERY_AUTH: "0", ROOM_TOKEN: "" },
    stdio: "ignore",
  });
  const base = `http://127.0.0.1:${port}`;
  for (let i = 0; i < 100; i++) {
    try { if ((await fetch(`${base}/health`)).ok) return { base, stop: () => child.kill() }; } catch (e) {}
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  child.kill();
  throw new Error("invite-only server did not start");
}

test("a member whose invite for this loca is revoked keeps their session and is told so", async ({ browser, request }) => {
  test.setTimeout(60_000);
  const server = await inviteOnlyServer();
  try {
    const at = p => `${server.base}${p}`;
    const room = "uhd-room";
    const name = "uhd-member";
    const TEXT = "uhd-line-after-invite-revoked-4b2e";
    expect((await request.post(at("/rooms"), { headers: MASTER, data: { name: room } })).status()).toBe(201);
    const member = await (await request.post(at("/members"), { headers: MASTER, data: { name, kind: "user" } })).json();
    // The invite is named by its id in management URLs, never by itself.
    const invite = (await (await request.post(at(`/rooms/${room}/invites`), { headers: MASTER, data: { name, kind: "user" } })).json()).id;
    expect(invite).toMatch(/^ivid_/);
    const context = await browser.newContext();
    const page = await context.newPage();
    await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
    const dialogs = [];
    page.on("dialog", dialog => { dialogs.push(dialog.message()); dialog.dismiss().catch(() => {}); });
    const errors = [];
    page.on("pageerror", error => errors.push(String(error)));
    const held = new Set();
    let exchanges = 0;
    let deletes = 0;
    page.on("request", sent => {
      const token = sent.headers()["x-session-token"];
      if (token) held.add(token);
      if (new URL(sent.url()).pathname === "/sessions") {
        if (sent.method() === "POST") exchanges++;
        if (sent.method() === "DELETE") deletes++;
      }
    });
    await page.goto(at("/"));
    await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("unauthenticated");
    await page.locator("#name").fill(name);
    await page.locator("#roomToken").fill(member.token);
    await page.evaluate(r => { doConnect(r); }, room);
    await expect.poll(() => page.evaluate(() => state.room)).toBe(room);
    await expect.poll(() => page.evaluate(() => !!state.session && state.ws && state.ws.readyState), { timeout: 15_000 }).toBe(1);
    const s1 = await page.evaluate(() => state.session);
    const who = async token => (await request.get(at("/whoami"), { headers: { "x-session-token": token } })).status();
    exchanges = 0;

    // Only this loca's invite is revoked; the membership and its session stay.
    expect((await request.delete(at(`/rooms/${room}/invites/${encodeURIComponent(invite)}`), { headers: MASTER })).ok()).toBeTruthy();
    expect(await who(s1)).toBe(200);
    await page.fill("#msg", TEXT);
    await page.keyboard.press("Enter");

    await expect(page.locator("#feed")).toContainText("you are no longer in this loca — you are still signed in", { timeout: 10_000 });
    await settle(page, 1500);
    expect(await page.evaluate(() => document.getElementById("msg").value)).toContain(TEXT);
    // No exchange, nothing ended, the same session, and no valid session the
    // page does not hold.
    expect(exchanges).toBe(0);
    expect(deletes).toBe(0);
    const current = await page.evaluate(() => state.session);
    expect(current).toBe(s1);
    for (const token of held) expect(await who(token)).toBe(token === current ? 200 : 401);
    expect(await page.evaluate(() => document.body.classList.contains("locked"))).toBe(false);
    expect(dialogs).toEqual([]);
    expect(errors).toEqual([]);
    await context.close();
  } finally {
    server.stop();
  }
});
