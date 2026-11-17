const { test, expect } = require("@playwright/test");

// Loca Host (the desktop app with its own server) hands its page a Master
// session it mints from the Host's admin token: at start, and again when the
// page asks because the one it holds has ended (main.rs host_master_session;
// state.js hostMasterSession). Here the shell is a stand-in: its
// host_master_session mints a real Master session on the test server, the way
// the shell does on its own. The page never sees an admin token.

const MASTER = { "x-admin-token": "MASTER" };
const HOST_SEAT = JSON.stringify({ name: "you", roomToken: "", room: "iye" });

async function mintMaster(request) {
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const minted = await (await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" },
  })).json();
  return JSON.stringify({ token: minted.session_token, expiresAt: minted.expires_at, name: "Master" });
}

// provisioning: what the shell injects as __LOCA_HOST_PROVISIONING__
// (undefined in a browser). initial: the Master session the page starts with.
async function openPage(page, request, { provisioning, initial, mint }) {
  await page.exposeFunction("__mintHostSession", mint || (() => mintMaster(request)));
  await page.addInitScript(({ provisioning, initial, seat }) => {
    localStorage.setItem("loca-gs-seen", "1");
    window.__hostCalls = 0;
    if (provisioning) {
      window.__LOCA_HOST_PROVISIONING__ = provisioning;
      window.__TAURI__ = { core: { invoke: (cmd) => {
        if (cmd !== "host_master_session") return Promise.reject(new Error(`unknown command ${cmd}`));
        window.__hostCalls++;
        return window.__mintHostSession();
      } } };
    }
    if (!sessionStorage.getItem("__started")) {
      sessionStorage.setItem("__started", "1");
      localStorage.setItem("loca-seat", seat);
      localStorage.setItem("loca-admin-session", initial);
    }
  }, { provisioning, initial, seat: HOST_SEAT });
  await page.goto("/");
}

async function endOnServer(request, token) {
  const ended = await request.delete("/sessions", { headers: { "x-session-token": token } });
  expect(ended.status()).toBe(204);
}

test("Loca Host: a Master session ended on the server is renewed by the Host, with no sign-in view", async ({ page, request }) => {
  await openPage(page, request, { provisioning: { state: "ready" }, initial: await mintMaster(request) });
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await expect.poll(() => page.evaluate(() => state.room)).toBe("iye");
  const first = await page.evaluate(() => state.session);

  await endOnServer(request, first);
  await expect.poll(() => page.evaluate(() => state.session), { timeout: 20_000 }).not.toBe(first);
  await expect.poll(() => page.evaluate(() => ({
    auth: state.authStatus, admin: state.adminSession, held: !!state.session,
    ws: state.ws && state.ws.readyState, room: state.room,
  })), { timeout: 15_000 }).toEqual({ auth: "authenticated", admin: true, held: true, ws: 1, room: "iye" });
  await expect(page.locator("body")).not.toHaveClass(/locked/);
  await expect(page.locator("#doorline")).not.toContainText("pairing code");
  expect(await page.evaluate(() => window.__hostCalls)).toBe(1);
  // The renewed session is what the page keeps for its next load.
  expect(await page.evaluate(() => JSON.parse(localStorage.getItem("loca-admin-session")).token))
    .toBe(await page.evaluate(() => state.session));
});

test("Loca Host: a Master session that expired before the page loaded is renewed at load", async ({ page, request }) => {
  const expired = JSON.stringify({ token: "st_expired_synthetic", expiresAt: Date.now() - 60_000, name: "Master" });
  await openPage(page, request, { provisioning: { state: "ready" }, initial: expired });
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  await expect(page.locator("body")).not.toHaveClass(/locked/);
  expect(await page.evaluate(() => window.__hostCalls)).toBe(1);
});

test("a browser Master whose session ended is asked to sign in again; nothing renews it", async ({ page, request }) => {
  await openPage(page, request, { provisioning: undefined, initial: await mintMaster(request) });
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await endOnServer(request, await page.evaluate(() => state.session));
  await expect(page.locator("body")).toHaveClass(/locked/, { timeout: 20_000 });
  await expect(page.locator("#doorline")).toContainText("your session ended");
});

test("the desktop client (no Host) does not ask for a Master session when one ends", async ({ page, request }) => {
  await openPage(page, request, { provisioning: { state: "not_applicable" }, initial: await mintMaster(request) });
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await endOnServer(request, await page.evaluate(() => state.session));
  await expect(page.locator("body")).toHaveClass(/locked/, { timeout: 20_000 });
  expect(await page.evaluate(() => window.__hostCalls)).toBe(0);
});

// Signing out is the person's decision, on a Host too: the page then holds no
// Master session, and the Host replaces only one the page held. A reload must
// not sign the person back in.
test("Loca Host: after signing out, a reload stays signed out and asks the Host for nothing", async ({ page, request }) => {
  await openPage(page, request, { provisioning: { state: "ready" }, initial: await mintMaster(request) });
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate(() => signOutHere());
  await expect(page.locator("body")).toHaveClass(/locked/);
  // The Host's person holds no pairing code: the door names what works
  // (desktop bug I).
  await expect(page.locator("#doorline")).toContainText("you signed out");
  await expect(page.locator("#doorline")).toContainText("reopen Loca Host");
  await expect(page.locator("#doorline")).not.toContainText("pairing code");
  await page.reload();
  await expect(page.locator("body")).toHaveClass(/locked/);
  await expect(page.locator("#doorline")).toContainText("reopen Loca Host");
  await expect(page.locator("#doorline")).not.toContainText("pairing code");
  await page.waitForTimeout(2500);   // past the health poll that could ask again
  expect(await page.evaluate(() => ({
    auth: state.authStatus, held: !!state.session, stored: localStorage.getItem("loca-admin-session"),
  }))).toEqual({ auth: "unauthenticated", held: false, stored: null });
  expect(await page.evaluate(() => window.__hostCalls)).toBe(0);
});

// The Host is minting while the person signs out. The answer is for someone
// who has left: it is not stored, and the session it carries is ended.
test("Loca Host: a session the Host hands over after the person signed out is not kept, and is ended", async ({ page, request }) => {
  let release;
  const held = new Promise(resolve => { release = resolve; });
  let minted = null;
  const mint = async () => { minted = await mintMaster(request); await held; return minted; };
  const expired = JSON.stringify({ token: "st_expired_synthetic", expiresAt: Date.now() - 60_000, name: "Master" });
  await openPage(page, request, { provisioning: { state: "ready" }, initial: expired, mint });
  await expect.poll(() => page.evaluate(() => window.__hostCalls)).toBe(1);
  await expect.poll(() => minted).not.toBeNull();
  await page.evaluate(() => signOutHere());
  await expect(page.locator("body")).toHaveClass(/locked/);
  release();
  await page.waitForTimeout(1500);
  expect(await page.evaluate(() => ({
    auth: state.authStatus, held: !!state.session, stored: localStorage.getItem("loca-admin-session"),
  }))).toEqual({ auth: "unauthenticated", held: false, stored: null });
  await expect(page.locator("body")).toHaveClass(/locked/);
  // Nobody holds the minted session: the page ended it at the server.
  const token = JSON.parse(minted).token;
  await expect.poll(async () => (await request.get("/whoami", { headers: { "x-session-token": token } })).status()).toBe(401);
});
