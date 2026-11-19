const { test, expect } = require("@playwright/test");
const { MASTER } = require("./support/switch");
const { provisionRooms } = require("./support/building");

// Building-wide search, walked as a member uses it: the sidebar's field
// offers "Search all messages for “q”", Enter opens the results grouped by
// loca (only the locas this member sits in), and a hit opens its loca at
// that message.

async function member(request, name, rooms) {
  const r = await request.post("/members", { headers: MASTER, data: { name, kind: "user" } });
  expect(r.status(), await r.text()).toBe(201);
  const { token } = await r.json();
  for (const room of rooms) {
    expect((await request.post(`/rooms/${room}/invites`, { headers: MASTER, data: { name } })).ok()).toBeTruthy();
  }
  return token;
}

async function sessionOf(request, name, key) {
  const r = await request.post("/sessions", { headers: { "x-room-token": key }, data: { name, kind: "user" } });
  expect(r.ok(), await r.text()).toBeTruthy();
  return { "x-session-token": (await r.json()).session_token };
}

// The Master's own session (this server requires sessions to post).
async function masterSession(request) {
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const info = await (await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" },
  })).json();
  return { "x-session-token": info.session_token };
}

async function say(request, headers, room, text) {
  const r = await request.post(`/rooms/${room}/messages`, {
    headers, data: { sender: "x", sender_type: "user", text },
  });
  expect(r.ok(), await r.text()).toBeTruthy();
  return (await r.json()).id;
}

async function openAsMember(browser, name, key) {
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await page.goto("/");
  await page.locator("#name").fill(name);
  await page.locator("#roomToken").fill(key);
  await page.locator("#connectBtn").click();
  await expect(page.locator("body")).not.toHaveClass(/locked/);
  await expect.poll(() => page.evaluate(() => state.principalId)).toBeTruthy();
  return page;
}

test("search all messages shows only one's own locas, grouped, and a hit opens the message", async ({ browser, request }) => {
  const [alpha, beta, gamma] = ["bs-alpha", "bs-beta", "bs-gamma"];
  await provisionRooms(request, [alpha, beta, gamma]);
  const key = await member(request, "bs-ayse", [alpha, beta]);
  const ayse = await sessionOf(request, "bs-ayse", key);
  const inAlpha = await say(request, ayse, alpha, "the haystack-needle sits in alpha");
  await say(request, ayse, beta, "a haystack-needle in beta too");
  await say(request, await masterSession(request), gamma, "gamma keeps its own haystack-needle");

  const page = await openAsMember(browser, "bs-ayse", key);
  await expect(page.locator("#sideSearchAll")).toBeHidden();
  await page.locator("#sideSearch").fill("haystack-needle");
  await expect(page.locator("#sideSearchAll")).toHaveText("Search all messages for “haystack-needle”");
  await page.locator("#sideSearch").press("Enter");

  const view = page.locator("#searchView");
  await expect(view).toBeVisible();
  await expect(page.locator("#searchQuery")).toHaveValue("haystack-needle");
  await expect(view.locator(".searchgroup")).toHaveCount(2);
  await expect(view.locator(".searchgroup").first()).toContainText(beta);
  await expect(view.locator(".searchgroup").nth(1)).toContainText(alpha);
  await expect(view).not.toContainText(gamma);
  await expect(view.locator("#searchCount")).toHaveText("Messages 2");
  await expect(view.locator("mark").first()).toHaveText("haystack-needle");

  // A hit opens its loca and marks the message there.
  await view.locator(`[data-search-room="${alpha}"]`).click();
  await expect(view).toBeHidden();
  await expect.poll(() => page.evaluate(() => state.room)).toBe(alpha);
  await expect(page.locator(`#feed .row[data-id="${inAlpha}"]`)).toHaveClass(/searchfound/);

  // The line under the field opens the same view.
  await page.locator("#sideSearch").fill("beta too");
  await page.locator("#sideSearchAll").click();
  await expect(view).toBeVisible();
  await expect(view.locator(".searchgroup")).toHaveCount(1);
  await expect(view.locator(".searchgroup")).toContainText(beta);
  await page.locator("#searchClose").click();
  await expect(view).toBeHidden();
  await page.context().close();
});
