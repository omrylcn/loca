const { test, expect } = require("@playwright/test");

// A desktop client whose remembered server does not answer keeps what it
// holds and retries: a server that is down is not a sign-out. Two things were
// wrong with that wait. The stylesheet hid its line, so the page was blank.
// And a person who signs in with a key (a remembered seat) retried forever,
// with the sign-in page and its server field hidden, so a server that had
// moved could not be followed. The line must name the address and offer to
// sign out and use another one, and what is held must never be sent to the
// new address.

const UNREACHABLE = "http://127.0.0.1:9";
const KEY = "lk_held_elsewhere";
const SESSION = "st_held_elsewhere";

function remember(page, what, extra = {}) {
  return page.addInitScript(({ server, what, key, session, extra }) => {
    localStorage.setItem("loca-gs-seen", "1");
    if (what === "seat") {
      localStorage.setItem("loca-seat", JSON.stringify({ name: "ada", roomToken: key, room: "demo" }));
    } else {
      localStorage.setItem("loca-admin-session", JSON.stringify({
        token: session, expiresAt: Date.now() + 3_600_000, name: "operator",
      }));
    }
    window.__LOCA_DEFAULT_SERVER__ = server;
    Object.assign(window, extra);
  }, { server: UNREACHABLE, what, key: KEY, session: SESSION, extra });
}

test("a remembered seat at an unreachable server can be left for another address", async ({ page, baseURL }) => {
  await remember(page, "seat");
  const leaked = [];
  page.on("request", req => {
    const h = req.headers();
    if (req.url().startsWith(baseURL) && (h["x-room-token"] === KEY || h["x-session-token"])) leaked.push(req.url());
  });

  await page.goto("/");
  await expect(page.locator("#doorline")).toContainText(`connection unavailable at ${UNREACHABLE}`);
  await expect(page.locator("#doorline")).toBeVisible();
  await expect(page.locator("body")).toHaveClass(/auth-pending/);
  await expect(page.locator("#connBox")).toBeHidden();
  const other = page.locator("#authOtherServer");
  await expect(other).toBeVisible();
  // The retries go on, and the button they leave is the one first shown.
  await other.evaluate(el => { el.dataset.first = "1"; });
  await page.waitForTimeout(4_500);
  await expect(page.locator("body")).toHaveClass(/auth-pending/);
  await expect(other).toHaveAttribute("data-first", "1");

  await other.click();
  await expect(page.locator("body")).toHaveClass(/locked/);
  await expect(page.locator("body")).not.toHaveClass(/auth-pending/);
  await expect(page.locator("#doorline")).toContainText(`cannot reach ${UNREACHABLE}`);
  await expect(page.locator("#server")).toBeVisible();
  expect(await page.evaluate(() => localStorage.getItem("loca-seat"))).toBeNull();

  await page.locator("#server").fill(baseURL);
  await page.locator("#server").dispatchEvent("change");
  await expect(page.locator("#doorline")).toContainText("sign in with your key");
  await expect(page.locator("#masterEntryButton")).toBeVisible();
  // The retrying is over: the page stays at the sign-in page.
  await page.waitForTimeout(2_500);
  await expect(page.locator("body")).toHaveClass(/locked/);
  await expect(page.locator("body")).not.toHaveClass(/auth-pending/);
  expect(leaked).toEqual([]);
});

test("a held Master session waiting on an unreachable server shows its line", async ({ page }) => {
  await remember(page, "session");
  await page.goto("/");
  await expect(page.locator("#doorline")).toContainText(`connection unavailable at ${UNREACHABLE}`);
  await expect(page.locator("#doorline")).toBeVisible();
});

test("Loca Host, locked to its own server, offers no other address", async ({ page }) => {
  await remember(page, "seat", { __LOCA_LOCK_SERVER__: true });
  await page.goto("/");
  await expect(page.locator("#doorline")).toContainText(`connection unavailable at ${UNREACHABLE}`);
  await expect(page.locator("#doorline")).toBeVisible();
  await expect(page.locator("#authOtherServer")).toHaveCount(0);
});
