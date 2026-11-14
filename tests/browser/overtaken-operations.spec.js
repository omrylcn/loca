const { test, expect } = require("@playwright/test");
const { MASTER, smasterKey, freshDoor, signInWithKey, expectSignedInAs, switchTo, settle, expectStill } = require("./support/switch");

// What one person started at this tab is theirs. Each test lets the first
// person start something, switches to a second person, and only then lets the
// unfinished part come due. It must not be sent under the second person's
// session, show them what it fetched, end their session or touch what they
// have typed or staged.

async function firstPersonInABirebir(browser, request, tag) {
  const oldKey = await smasterKey(request, `${tag}-old`);
  await smasterKey(request, `${tag}-peer`);
  const nextKey = await smasterKey(request, `${tag}-next`);
  const door = await freshDoor(browser);
  await signInWithKey(door.page, oldKey);
  await expectSignedInAs(door.page, `${tag}-old`);
  await door.page.evaluate(peer => openBirebir(peer), `${tag}-peer`);
  await expect.poll(() => door.page.evaluate(() => isBirebir(state.room))).toBe(true);
  await expect.poll(() => door.page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  return { ...door, nextKey, next: `${tag}-next` };
}

for (const [index, first] of ["503", "broken connection"].entries()) {
test(`a send's one retry after ${first} is not made once its writer has left`, async ({ browser, request }) => {
  const tag = `ovr${index}`;
  const OLD_TEXT = `${tag}-unsent-line-9b2d`;
  const { context, page, dialogs, nextKey, next } = await firstPersonInABirebir(browser, request, tag);

  const posts = [];
  await page.route("**/rooms/*/messages", async route => {
    const sent = route.request();
    if (sent.method() !== "POST" || !(sent.postData() || "").includes(OLD_TEXT)) return route.fallback();
    posts.push(sent.headers()["x-session-token"] || "");
    if (posts.length > 1) return route.fulfill({ status: 403, body: "not yours" });
    return first === "503" ? route.fulfill({ status: 503, body: "upstream lost" }) : route.abort("connectionreset");
  });
  // Pin the 350 ms the retry waits: it fires when the test says so.
  await page.evaluate(() => {
    const real = window.setTimeout;
    const gate = window.__retryGate = { waiting: false };
    window.setTimeout = (fn, ms, ...rest) => {
      if (ms !== 350 || gate.waiting) return real(fn, ms, ...rest);
      gate.waiting = true;
      gate.fire = () => fn(...rest);
      return 0;
    };
  });
  await page.fill("#msg", OLD_TEXT);
  await page.keyboard.press("Enter");
  await expect.poll(() => page.evaluate(() => window.__retryGate.waiting)).toBe(true);
  expect(posts.length).toBe(1);

  const mine = await switchTo(page, nextKey, next);
  await page.evaluate(() => window.__retryGate.fire());
  await settle(page);

  expect(posts.length).toBe(1);
  expect(posts).not.toContain(mine.session);
  expect(await page.evaluate(() => document.getElementById("msg").value)).not.toContain(OLD_TEXT);
  expect(await page.content()).not.toContain(OLD_TEXT);
  expect(dialogs).toEqual([]);
  await expectStill(page, mine);
  await context.close();
});
}

for (const held of ["network", "body"]) {
test(`a key created just before leaving is not shown to the next person (held at the ${held})`, async ({ browser, request }) => {
  const tag = `ovk-${held}`;
  const oldKey = await smasterKey(request, `${tag}-old`);
  const nextKey = await smasterKey(request, `${tag}-next`);
  const { context, page, dialogs } = await freshDoor(browser);
  await signInWithKey(page, oldKey);
  await expectSignedInAs(page, `${tag}-old`);

  const hold = {};
  hold.arrived = new Promise(resolve => { hold.seen = resolve; });
  const released = new Promise(resolve => { hold.release = resolve; });
  if (held === "network") {
    // The server has made the key; its answer is held on the way back.
    await page.route("**/profile/credentials", async route => {
      if (route.request().method() !== "POST") return route.fallback();
      const response = await route.fetch();
      const body = await response.json();
      hold.secret = body.secret;
      hold.seen();
      await released;
      await route.fulfill({ response, json: body }).catch(() => {});
    });
  } else {
    // The answer has arrived and been read; the code that asked is handed it
    // only when the test says so.
    await page.evaluate(() => {
      const gate = window.__keyGate = { secret: "" };
      const open = new Promise(resolve => { gate.open = resolve; });
      const json = Response.prototype.json;
      Response.prototype.json = async function () {
        const data = await json.call(this);
        if (data && typeof data.secret === "string" && !gate.secret) { gate.secret = data.secret; await open; }
        return data;
      };
    });
  }
  await page.evaluate(() => {
    document.getElementById("credentialLabel").value = "laptop";
    createProfileCredential();
  });
  if (held === "network") await hold.arrived;
  else {
    await expect.poll(() => page.evaluate(() => window.__keyGate.secret)).not.toBe("");
    hold.secret = await page.evaluate(() => window.__keyGate.secret);
  }
  expect(hold.secret.length).toBeGreaterThan(8);

  const mine = await switchTo(page, nextKey, `${tag}-next`);
  if (held === "network") hold.release(); else await page.evaluate(() => window.__keyGate.open());
  await settle(page);

  expect(await page.content()).not.toContain(hold.secret);
  await expect(page.locator("#credentialSecret")).toBeHidden();
  expect(await page.evaluate(() => document.getElementById("credentialSecret").textContent)).toBe("");
  expect(dialogs).toEqual([]);
  await expectStill(page, mine);
  await context.close();
});
}

test("revoking one's own key, answered after the next person signed in, does not end their session", async ({ browser, request }) => {
  const tag = "ovx";
  const oldKey = await smasterKey(request, `${tag}-old`);
  const nextKey = await smasterKey(request, `${tag}-next`);
  const { context, page, answer } = await freshDoor(browser);
  await signInWithKey(page, oldKey);
  await expectSignedInAs(page, `${tag}-old`);
  await expect.poll(() => page.evaluate(() => state.credentials.some(c => c.current && !c.revoked_at))).toBe(true);

  const hold = {};
  hold.arrived = new Promise(resolve => { hold.seen = resolve; });
  const released = new Promise(resolve => { hold.release = resolve; });
  await page.route("**/profile/credentials/*", async route => {
    if (route.request().method() !== "DELETE") return route.fallback();
    const response = await route.fetch();   // the server has revoked it
    hold.status = response.status();
    hold.seen();
    await released;
    await route.fulfill({ response }).catch(() => {});
  });
  answer.yes = true;   // "Revoke the key behind this session?"
  await page.evaluate(() => {
    const own = state.credentials.find(c => c.current && !c.revoked_at);
    revokeProfileCredential(own.id, true);
  });
  await hold.arrived;
  expect(hold.status).toBeLessThan(300);

  const mine = await switchTo(page, nextKey, `${tag}-next`);
  hold.release();
  await settle(page);
  await expectStill(page, mine);
  await context.close();
});

test("a renewed send answered late does not clear the next person's staged attachments", async ({ browser, request }) => {
  const tag = "ova";
  const OLD_TEXT = `${tag}-renewed-line-51c7`;
  const { context, page, dialogs, nextKey, next } = await firstPersonInABirebir(browser, request, tag);
  expect(await page.evaluate(() => !!state.roomToken)).toBe(true);

  // First try: 401, as after a restart. The page renews and sends again; that
  // second answer is marked so its body can be held.
  let tries = 0;
  await page.route("**/rooms/*/messages", async route => {
    const sent = route.request();
    if (sent.method() !== "POST" || !(sent.postData() || "").includes(OLD_TEXT)) return route.fallback();
    tries++;
    if (tries === 1) return route.fulfill({ status: 401, body: "session ended" });
    const response = await route.fetch();
    await route.fulfill({ response, headers: { ...response.headers(), "x-held": "1" } }).catch(() => {});
  });
  await page.evaluate(() => {
    const gate = window.__bodyGate = { arrived: false };
    const open = new Promise(resolve => { gate.open = resolve; });
    const json = Response.prototype.json;
    Response.prototype.json = async function () {
      if (this.headers.get("x-held") === "1" && !gate.arrived) { gate.arrived = true; await open; }
      return json.call(this);
    };
  });
  await page.fill("#msg", OLD_TEXT);
  await page.keyboard.press("Enter");
  await expect.poll(() => page.evaluate(() => window.__bodyGate.arrived), { timeout: 15_000 }).toBe(true);
  expect(tries).toBe(2);

  const mine = await switchTo(page, nextKey, next);
  // The next person stages a file of their own.
  await page.evaluate(() => {
    state.pendingAttachments = [{ id: "att_next_person", name: "mine.txt", size: 3, mime: "text/plain" }];
    renderAttachStrip();
  });
  await page.evaluate(() => window.__bodyGate.open());
  await settle(page);

  expect(await page.evaluate(() => state.pendingAttachments.map(a => a.id))).toEqual(["att_next_person"]);
  expect(dialogs).toEqual([]);
  await expectStill(page, mine);
  await context.close();
});

test("a join request approved after someone else signed in does not take the tab from them", async ({ browser, request }) => {
  const tag = "ovj";
  const nextKey = await smasterKey(request, `${tag}-next`);
  const { context, page } = await freshDoor(browser);

  // Someone asks to join from the door and waits for a Master.
  await page.locator("#name").fill(`${tag}-asker`);
  await page.locator("#joinRequestBtn").click();
  await expect(page.locator("#joinRequestStatus")).toContainText("waiting for a Master");

  // Before any answer, another person signs in at this tab.
  await signInWithKey(page, nextKey);
  await expectSignedInAs(page, `${tag}-next`);
  const mine = await page.evaluate(() => ({ session: state.session, name: state.name }));

  // Now the Master approves the request.
  expect((await request.post("/admission-stock", { headers: MASTER, data: { count: 1 } })).ok()).toBeTruthy();
  const pending = await (await request.get("/join-requests", { headers: MASTER })).json();
  const asked = pending.pending.find(r => r.name === `${tag}-asker`);
  expect(asked).toBeTruthy();
  expect((await request.post(`/join-requests/${asked.id}/approve`, { headers: MASTER })).ok()).toBeTruthy();

  // Longer than the request's 3 s watch.
  await settle(page, 4500);
  await expectStill(page, mine);
  expect(await page.evaluate(() => document.getElementById("roomToken").value)).not.toMatch(/^mb_/);
  await context.close();
});
