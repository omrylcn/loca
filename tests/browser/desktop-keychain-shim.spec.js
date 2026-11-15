const { test, expect } = require("@playwright/test");
const fs = require("node:fs");
const path = require("node:path");

// The desktop shell's document-start Storage override (KEYCHAIN_SHIM in
// desktop/src-tauri/src/main.rs), run in a real page with a stand-in for
// Tauri's invoke that records which keychain commands it was asked for. The
// init script runs again, with the same launch-time values, on every load of
// the page, exactly as the shell's initialization script does.
function keychainShim() {
  const source = fs.readFileSync(
    path.join(__dirname, "../../desktop/src-tauri/src/main.rs"),
    "utf8",
  );
  const found = source.match(/const KEYCHAIN_SHIM: &str = r#"([\s\S]*?)"#;/);
  if (!found) throw new Error("KEYCHAIN_SHIM not found in main.rs");
  return found[1];
}

// `storage` stands for the window's sessionStorage: "ok"; "full" (every write
// throws QuotaExceededError); "refuse" (writes and removals throw); "absent"
// (reading window.sessionStorage itself throws); "fullAfterFirst" (the first
// write of the test succeeds, every later one, on any load, throws). Setting
// window.__fullNow in a page makes its writes fail from then on, for that
// load only.
// `origins`: what the shell names as this window's origins (main.rs
// APP_ORIGIN); by default the test page's own.
async function openWithShim(page, boot, storage = "ok", origins = null) {
  await page.route("**/__keychain-shim", (route) =>
    route.fulfill({ contentType: "text/html", body: "<!doctype html><title>shim</title><p id=doorline>door</p>" }),
  );
  await page.addInitScript((storage) => {
    if (storage === "absent") {
      Object.defineProperty(window, "sessionStorage", {
        configurable: true, get() { throw new DOMException("denied", "SecurityError"); },
      });
      return;
    }
    const get = Storage.prototype.getItem, set = Storage.prototype.setItem, remove = Storage.prototype.removeItem;
    Storage.prototype.setItem = function (k, v) {
      if (this === window.sessionStorage) {
        let fail = (storage !== "ok" && storage !== "fullAfterFirst") || window.__fullNow;
        if (storage === "fullAfterFirst") {
          fail = get.call(window.localStorage, "__wrote") === "1";
          set.call(window.localStorage, "__wrote", "1");
        }
        if (fail) throw new DOMException("full", "QuotaExceededError");
      }
      return set.call(this, k, v);
    };
    Storage.prototype.removeItem = function (k) {
      if (this === window.sessionStorage && storage === "refuse") throw new DOMException("denied", "SecurityError");
      return remove.call(this, k);
    };
  }, storage);
  await page.addInitScript(
    ({ boot, code, origins }) => {
      window.__kcCalls = [];
      window.__LOCA_KC_ORIGINS__ = origins || [location.origin];
      window.__TAURI__ = {
        core: {
          invoke: (cmd, args) => {
            window.__kcCalls.push([cmd, args && args.key]);
            return Promise.resolve(null);
          },
        },
      };
      window.__LOCA_KC_BOOT__ = boot;
      (0, eval)(code);
    },
    { boot, code: keychainShim(), origins },
  );
  await page.goto("/__keychain-shim");
}

const SESSION = JSON.stringify({ token: "st_launch_synthetic", expiresAt: 1893456000000, name: "Master" });
const RENEWED = JSON.stringify({ token: "st_renewed_synthetic", expiresAt: 1893456000000, name: "Master" });

test("desktop keychain shim: a sessionStorage removal does not delete the stored Master session", async ({ page }) => {
  await openWithShim(page, { "loca-admin-session": SESSION });
  // What state.js does after a successful /whoami: save the session, then
  // drop any legacy sessionStorage copy.
  const result = await page.evaluate((renewed) => {
    localStorage.setItem("loca-admin-session", renewed);
    sessionStorage.removeItem("loca-admin-session");
    return {
      stored: localStorage.getItem("loca-admin-session"),
      calls: window.__kcCalls.filter(([, key]) => key === "loca-admin-session").map(([cmd]) => cmd),
    };
  }, RENEWED);
  expect(result.stored).toBe(RENEWED);
  expect(result.calls).not.toContain("kc_delete");
});

test("desktop keychain shim: sessionStorage reads and writes stay out of the keychain", async ({ page }) => {
  await openWithShim(page, { "loca-admin-session": SESSION });
  const result = await page.evaluate(() => {
    window.__kcCalls.length = 0;
    const seen = sessionStorage.getItem("loca-admin-session");
    sessionStorage.removeItem("loca-admin-session");
    return { seen, calls: window.__kcCalls.slice(), stored: localStorage.getItem("loca-admin-session") };
  });
  expect(result.seen).toBeNull();
  expect(result.calls).toEqual([]);
  expect(result.stored).toBe(SESSION);
});

test("desktop keychain shim: a reload keeps a sign-in made after launch", async ({ page }) => {
  // The window was built before anyone signed in: nothing in the keychain.
  await openWithShim(page, {});
  const seat = { name: "fx-ada", roomToken: "dv_synthetic_davet", room: "demo" };
  await page.evaluate((s) => localStorage.setItem("loca-seat", JSON.stringify(s)), seat);
  await page.reload();
  expect(await page.evaluate(() => JSON.parse(localStorage.getItem("loca-seat") || "null"))).toEqual(seat);
  // Still never in the page's on-disk localStorage.
  expect(await page.evaluate(() => window.localStorage["loca-seat"])).toBeUndefined();
});

test("desktop keychain shim: a reload after sign-out does not bring the launch-time session back", async ({ page }) => {
  await openWithShim(page, { "loca-admin-session": SESSION });
  await page.evaluate(() => localStorage.removeItem("loca-admin-session"));
  await page.reload();
  expect(await page.evaluate(() => localStorage.getItem("loca-admin-session"))).toBeNull();
});

// The launch-time values are a one-time hand-over to the window. A change the
// window's own storage could not keep must never let a reload fall back to
// them, or to an older kept copy: that would sign in again someone who signed
// out, with a key they removed. The page starts signed out instead.
const SEAT_A = JSON.stringify({ name: "fx-ada", roomToken: "mb_synthetic_ada", room: "demo" });
const SEAT_B = JSON.stringify({ name: "fx-bob", roomToken: "mb_synthetic_bob", room: "demo" });

async function credentials(page) {
  return page.evaluate(() => ({
    seat: localStorage.getItem("loca-seat"),
    session: localStorage.getItem("loca-admin-session"),
  }));
}

async function signOut(page) {
  await page.evaluate(() => {
    localStorage.removeItem("loca-seat");
    localStorage.removeItem("loca-admin-session");
  });
}

for (const storage of ["full", "refuse"]) {
  test(`desktop keychain shim: sign-out then reload stays signed out (sessionStorage ${storage})`, async ({ page }) => {
    await openWithShim(page, { "loca-seat": SEAT_A, "loca-admin-session": SESSION }, storage);
    expect(await credentials(page)).toEqual({ seat: SEAT_A, session: SESSION });
    await signOut(page);
    expect(await page.evaluate(() => window.__kcCalls.filter(([cmd]) => cmd === "kc_delete").length)).toBe(2);
    await page.reload();
    expect(await credentials(page)).toEqual({ seat: null, session: null });
    await expect(page.locator("#doorline")).toContainText("could not keep your sign-in");
  });
}

test("desktop keychain shim: storage that fills up after launch does not bring back the copy kept before", async ({ page }) => {
  await openWithShim(page, { "loca-seat": SEAT_A, "loca-admin-session": SESSION });
  await page.evaluate(() => { window.__fullNow = true; });
  await signOut(page);
  await page.reload();
  expect(await credentials(page)).toEqual({ seat: null, session: null });
});

test("desktop keychain shim: a renewed session that cannot be kept does not bring back the launch-time one", async ({ page }) => {
  await openWithShim(page, { "loca-admin-session": SESSION }, "full");
  await page.evaluate((renewed) => localStorage.setItem("loca-admin-session", renewed), RENEWED);
  expect((await credentials(page)).session).toBe(RENEWED);
  await page.reload();
  expect(await credentials(page)).toEqual({ seat: null, session: null });
});

test("desktop keychain shim: a sign-in as someone else that cannot be kept does not bring back the launch-time person", async ({ page }) => {
  await openWithShim(page, { "loca-seat": SEAT_A }, "full");
  await page.evaluate((seat) => localStorage.setItem("loca-seat", seat), SEAT_B);
  await page.reload();
  expect(await credentials(page)).toEqual({ seat: null, session: null });
});

test("desktop keychain shim: with working storage a reload keeps the latest values and says nothing", async ({ page }) => {
  await openWithShim(page, { "loca-seat": SEAT_A, "loca-admin-session": SESSION });
  await page.reload();
  expect(await credentials(page)).toEqual({ seat: SEAT_A, session: SESSION });
  await page.evaluate((seat) => localStorage.setItem("loca-seat", seat), SEAT_B);
  await page.reload();
  await page.reload();
  expect(await credentials(page)).toEqual({ seat: SEAT_B, session: SESSION });
  await expect(page.locator("#doorline")).toHaveText("door");
});

test("desktop keychain shim: with no sessionStorage the window never takes the launch-time values", async ({ page }) => {
  await openWithShim(page, { "loca-seat": SEAT_A, "loca-admin-session": SESSION }, "absent");
  expect(await credentials(page)).toEqual({ seat: null, session: null });
  await expect(page.locator("#doorline")).toContainText("could not keep your sign-in");
  await page.reload();
  expect(await credentials(page)).toEqual({ seat: null, session: null });
});

// The two records check each other: window.name alone emptied (a page script,
// a navigation) must not make a reload look like a new window.
test("desktop keychain shim: a cleared window name next to a kept copy leaves the page signed out", async ({ page }) => {
  await openWithShim(page, { "loca-seat": SEAT_A, "loca-admin-session": SESSION });
  await signOut(page);
  await page.evaluate(() => { window.name = ""; });
  await page.reload();
  expect(await credentials(page)).toEqual({ seat: null, session: null });
  await expect(page.locator("#doorline")).toContainText("could not keep your sign-in");
});

test("desktop keychain shim: a kept copy under another counter leaves the page signed out", async ({ page }) => {
  await openWithShim(page, { "loca-seat": SEAT_A, "loca-admin-session": SESSION });
  await page.evaluate(() => { window.name = "loca-desktop-credentials:7"; });
  await page.reload();
  expect(await credentials(page)).toEqual({ seat: null, session: null });
});

test("desktop keychain shim: storage that stays full never matches the copy kept before the sign-out", async ({ page }) => {
  await openWithShim(page, { "loca-seat": SEAT_A, "loca-admin-session": SESSION }, "fullAfterFirst");
  await signOut(page);
  for (let load = 0; load < 3; load++) {
    await page.reload();
    expect(await credentials(page)).toEqual({ seat: null, session: null });
  }
});

// The init script runs on every top-level page of the window. On any origin
// but the app's own the hand-over is not given and nothing is installed.
test("desktop keychain shim: on another origin nothing is handed over and nothing is installed", async ({ page }) => {
  await openWithShim(page, { "loca-seat": SEAT_A, "loca-admin-session": SESSION }, "ok", ["tauri://localhost"]);
  const seen = await page.evaluate(() => ({
    seat: localStorage.getItem("loca-seat"),
    session: localStorage.getItem("loca-admin-session"),
    boot: window.__LOCA_KC_BOOT__ === undefined ? null : "present",
    name: window.name,
    kept: sessionStorage.getItem("loca-desktop-credentials"),
  }));
  expect(seen).toEqual({ seat: null, session: null, boot: null, name: "", kept: null });
  // No storage override: a credential key is plain localStorage here, and
  // nothing is sent to the keychain.
  await page.evaluate(() => localStorage.setItem("loca-seat", "plain"));
  expect(await page.evaluate(() => window.localStorage["loca-seat"])).toBe("plain");
  expect(await page.evaluate(() => window.__kcCalls)).toEqual([]);
});

// The real web UI under the shim: the reason a load started signed out stays
// on the sign-in view when the page later hears 401 from the server, until
// the person signs in again.
async function openAppWithShim(page, boot) {
  await page.addInitScript(
    ({ boot, code }) => {
      localStorage.setItem("loca-gs-seen", "1");
      window.__TAURI__ = { core: { invoke: () => Promise.resolve(null) } };
      window.__LOCA_KC_ORIGINS__ = [location.origin];
      window.__LOCA_KC_BOOT__ = boot;
      (0, eval)(code);
    },
    { boot, code: keychainShim() },
  );
  await page.goto("/");
}

test("desktop keychain shim: the reason a reload started signed out survives a later 401", async ({ page }) => {
  await openAppWithShim(page, { "loca-seat": SEAT_A });
  await page.evaluate(() => { window.name = ""; });   // the two records now disagree
  await page.reload();
  const door = page.locator("#doorline");
  await expect(door).toContainText("could not keep your sign-in");
  // A later read of the room list is refused (no credential): the general
  // door text must not replace the reason.
  await page.evaluate(() => { state.authStatus = "unauthenticated"; return refreshRooms(); });
  await expect.poll(() => page.evaluate(() => roomsRefreshing)).toBe(false);
  await expect(door).toContainText("could not keep your sign-in");
  // Signing in again (a credential stored) ends it.
  await page.evaluate((seat) => localStorage.setItem("loca-seat", seat), SEAT_B);
  expect(await page.evaluate(() => window.__LOCA_SIGNIN_NOTICE__)).toBeNull();
});
