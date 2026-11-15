const { test, expect } = require("@playwright/test");

// When Loca Host's own server does not start, the shell gives its page no
// server address (main.rs page_server) and says why (__LOCA_HOST_PROVISIONING__).
// The page must then talk to no server at all: not a baked address such as
// http://127.0.0.1:8787 (the #server placeholder), whatever listens there,
// and not its own origin. It shows the failure instead.

const BAKED = "http://127.0.0.1:8787";

async function openHost(page, baseURL, server) {
  const asked = [];
  page.on("request", sent => {
    const url = new URL(sent.url());
    // The page's own files are loaded from its origin; anything else is a
    // request to a server.
    if (url.origin === new URL(baseURL).origin
        && (url.pathname === "/" || url.pathname.startsWith("/assets/"))) return;
    asked.push(`${sent.method()} ${sent.url()}`);
  });
  await page.route(`${BAKED}/**`, route => route.fulfill({ status: 200, contentType: "application/json", body: "{}" }));
  await page.addInitScript((server) => {
    localStorage.setItem("loca-gs-seen", "1");
    window.__LOCA_LOCK_SERVER__ = true;
    window.__LOCA_DEFAULT_SERVER__ = server;
    window.__LOCA_HOST_PROVISIONING__ = { state: "error", stage: "server_start" };
    // What the Host's keychain may still hold from an earlier start.
    localStorage.setItem("loca-seat", JSON.stringify({ name: "you", roomToken: "", room: "iye" }));
    localStorage.setItem("loca-admin-session", JSON.stringify({
      token: "st_synthetic_master", expiresAt: Date.now() + 3600_000, name: "Master",
    }));
  }, server);
  await page.goto("/");
  return asked;
}

test("Loca Host without its own server talks to no server and shows the failure", async ({ page, baseURL }) => {
  const asked = await openHost(page, baseURL, "");
  await page.waitForTimeout(6000); // past a health-poll period
  // Even a sign-in attempt goes nowhere.
  await page.evaluate(() => doConnect("iye"));
  await page.waitForTimeout(1000);
  expect(asked).toEqual([]);
  expect(await page.evaluate(() => serverBase())).toBe("");
  await expect(page.locator("#hostProvisionError")).toBeVisible();
  await expect(page.locator("#doorline")).toContainText("could not start its own server");
});

test("Loca Host talks only to the server it started, whatever is typed in the server field", async ({ page, baseURL }) => {
  // A stand-in for the Host's own server: the test server itself.
  const own = new URL(baseURL).origin;
  await openHost(page, baseURL, own);
  await page.evaluate(() => { document.getElementById("server").value = "http://127.0.0.1:8787"; });
  expect(await page.evaluate(() => serverBase())).toBe(own);
});
