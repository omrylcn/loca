const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

// What the session cap does to a signed-in page. One credential holds at most
// 16 live sessions; taking a 17th ends its oldest, which can be the one a page
// is holding. A page signed in with a membership key still holds that key, so
// it must take a new session and carry on by itself. A Master pairing code is
// one-use, so there asking again is right. Renewal after a session ended on
// the server (expiry, sign-out) is covered by session-renewal.spec.js.

const MASTER = { "x-admin-token": "MASTER" };
const CAP = 16;   // SESSIONS_PER_CREDENTIAL in the server
const ROOM = "session-lifetime";

test.beforeEach(async ({ page, request }) => {
  await provisionRooms(request, [ROOM]);
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
});

async function signIn(page, name, key) {
  await page.goto("/");
  await page.locator("#name").fill(name);
  await page.locator("#roomToken").fill(key);
  await page.evaluate((room) => window.doConnect(room), ROOM);
  await expect.poll(() => page.evaluate(() => state.ws?.readyState)).toBe(1);
  return page.evaluate(() => state.session);
}

async function member(request, name) {
  const admitted = await request.post("/members", { headers: MASTER, data: { name, kind: "user" } });
  expect(admitted.ok()).toBeTruthy();
  const { token } = await admitted.json();
  const invited = await request.post(`/rooms/${ROOM}/invites`, { headers: MASTER, data: { name } });
  expect(invited.ok()).toBeTruthy();
  return { membership: token, invite: (await invited.json()).token };
}

async function takeSessions(request, headers, count) {
  for (let i = 0; i < count; i++) {
    const taken = await request.post("/sessions", { headers, data: { name: "elsewhere", kind: "user" } });
    expect(taken.status()).toBe(201);
  }
}

// The page carries on: a new session, an open socket, an unlocked page, and
// a message said afterwards reaches it, with nothing done by the person.
async function carriesOn(page, request, ended, text) {
  await expect.poll(() => page.evaluate((ended) => (
    !!state.session && state.session !== ended
      && state.ws?.readyState === 1
      && !document.body.classList.contains("locked")
  ), ended), { timeout: 30_000 }).toBe(true);
  const operator = await (await request.post("/sessions", {
    headers: MASTER, data: { name: "operator", kind: "user" },
  })).json();
  const said = await request.post(`/rooms/${ROOM}/messages`, {
    headers: { ...MASTER, "x-session-token": operator.session_token },
    data: { sender: "operator", sender_type: "user", text },
  });
  expect(said.ok()).toBeTruthy();
  await expect.poll(() => page.evaluate((text) => state.msgs.some(m => m.text === text), text))
    .toBe(true);
}

test("a membership-signed page whose session was evicted carries on by itself", async ({ page, request }) => {
  const { membership } = await member(request, "renewal-member");
  const session = await signIn(page, "renewal-member", membership);
  // Sixteen newer sessions of the same key end the page's, the oldest.
  await takeSessions(request, { "x-admin-token": membership }, CAP);
  await carriesOn(page, request, session, "after the member session was evicted");
});

test("a Master page whose pairing session was evicted asks for a new code", async ({ page, request }) => {
  const pairing = await (await request.post("/pairings", { headers: MASTER, data: {} })).json();
  await page.goto("/");
  await page.locator("#masterEntryButton").click();
  await page.locator("#pairingCode").fill(pairing.pairing_code);
  await page.evaluate((room) => window.doConnect(room), ROOM);
  await expect.poll(() => page.evaluate(() => state.adminSession)).toBe(true);
  // Every Master session counts under the root-recovery credential.
  await takeSessions(request, MASTER, CAP);
  await expect(page.locator("body")).toHaveClass(/locked/, { timeout: 30_000 });
  await expect(page.locator("#doorline")).toContainText("your session ended");
  expect(await page.evaluate(() => state.session)).toBeNull();
  expect(await page.evaluate(() => localStorage.getItem("loca-admin-session"))).toBeNull();
});
