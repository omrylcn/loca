const { test, expect } = require("@playwright/test");

// "+ call" opens a dialog, "Call into <loca>", instead of a list squeezed
// under the button where a name ran into the locas beside it. Candidates are
// grouped Agents and People; each shows whether they are online, where they
// sit, their floors and, for an agent, who admitted it. Several can be
// ticked and called at once: one request each through the call route, with
// a result for each. A floor loca offers only that floor's people.

const MASTER = { "x-admin-token": "MASTER" };

// Letters only: a synthetic tag that keeps each test's names apart on the
// one server the suite shares.
const tagOf = () => Date.now().toString(36).replace(/\d/g, d => "abcdefghij"[d]);

async function masterSession(request) {
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  return (await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" },
  })).json();
}

async function building(request, tag, { floor = false } = {}) {
  const m = await masterSession(request);
  const as = { "x-session-token": m.session_token };
  let group = null;
  if (floor) group = (await (await request.post("/groups", { headers: as, data: { name: `${tag}-floor` } })).json()).id;
  const room = `${tag}-loca`;
  const made = await request.post("/rooms", { headers: as, data: group ? { name: room, group } : { name: room } });
  expect(made.status(), await made.text()).toBe(201);
  const admit = async (name, kind) => {
    const r = await request.post("/members", { headers: MASTER, data: { name, kind } });
    expect(r.status(), await r.text()).toBe(201);
  };
  const agents = [`${tag}-agent-a`, `${tag}-agent-b`, `${tag}-agent-c`];
  for (const name of agents) await admit(name, "agent");
  const person = `${tag}-person`;
  await admit(person, "user");
  // Somewhere else to sit: an agent seated in another loca shows it.
  const elsewhere = `${tag}-elsewhere`;
  expect((await request.post("/rooms", { headers: as, data: { name: elsewhere } })).status()).toBe(201);
  expect((await request.post(`/rooms/${elsewhere}/invites`, { headers: MASTER, data: { name: agents[2] } })).ok()).toBeTruthy();
  return { m, as, room, group, agents, person, elsewhere };
}

async function openAsMaster(browser, m, room, viewport = { width: 1280, height: 900 }) {
  const page = await browser.newPage({ viewport });
  await page.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "" }));
  }, { token: m.session_token, expiresAt: m.expires_at });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate(room => joinRoom(room), room);
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  await page.evaluate(() => {
    if (window.innerWidth <= 760) setMobileSidebar(true);
    setSidebarView("loca");
  });
  await expect(page.locator("#callBtn")).toBeVisible();
  return page;
}

const dialog = (page) => page.locator("#callDialog");
const row = (page, name) => dialog(page).locator(".callcand").filter({ hasText: name });

test("+ call opens the call dialog: a search, Agents and People, and who each one is", async ({ browser, request }) => {
  const tag = tagOf();
  const b = await building(request, tag);
  const page = await openAsMaster(browser, b.m, b.room);
  await page.locator("#callBtn").click();
  await expect(dialog(page)).toBeVisible();
  await expect(dialog(page).locator("h2")).toHaveText(`Call into ${b.room}`);
  await expect(page.locator("#callSearch")).toBeFocused();
  await expect(dialog(page).getByLabel("Search")).toBeVisible();
  await expect(dialog(page).locator(".callgroup h3")).toHaveText([/^Agents/, /^People/]);
  // Each candidate is a real checkbox with its name for a label.
  await expect(dialog(page).getByRole("checkbox", { name: new RegExp(b.agents[0]) })).toBeVisible();
  // Online or not, where they sit, their floors and who admitted the agent.
  await expect(row(page, b.agents[0]).locator(".callpresence")).toHaveText("away");
  await expect(row(page, b.agents[2]).locator(".callwhere")).toContainText(b.elsewhere);
  await expect(row(page, b.agents[0]).locator(".callfloors")).toContainText("ground floor");
  await expect(row(page, b.agents[0]).locator(".callowner")).toHaveText("admitted by the Master");
  await expect(row(page, b.person).locator(".callowner")).toHaveCount(0);
  // Nobody already seated here is offered.
  await expect(dialog(page).locator(".callcand").filter({ hasText: "(you)" })).toHaveCount(0);
  // Esc closes it and gives the focus back to "+ call"; the backdrop too.
  await page.keyboard.press("Escape");
  await expect(dialog(page)).toBeHidden();
  await expect(page.locator("#callBtn")).toBeFocused();
  await page.locator("#callBtn").click();
  await expect(dialog(page)).toBeVisible();
  await page.locator("#callDialogBackdrop").click({ position: { x: 5, y: 5 } });
  await expect(dialog(page)).toBeHidden();
  await page.close();
});

test("the search narrows the list; ticks are kept while it does", async ({ browser, request }) => {
  const tag = tagOf();
  const b = await building(request, tag);
  const page = await openAsMaster(browser, b.m, b.room);
  await page.locator("#callBtn").click();
  await row(page, b.agents[0]).getByRole("checkbox").check();
  // Other tests' members share this server: search this test's own name.
  await page.locator("#callSearch").fill(b.agents[1].toUpperCase());
  await expect(dialog(page).locator(".callcand:visible")).toHaveCount(1);
  await expect(row(page, b.agents[1])).toBeVisible();
  await expect(row(page, b.person)).toBeHidden();
  await expect(page.locator("#callSubmit")).toHaveText("Call 1");
  await page.locator("#callSearch").fill("nobody-has-this-name");
  await expect(dialog(page).locator(".callnone")).toContainText("Nobody matches");
  await page.locator("#callSearch").fill("");
  await expect(row(page, b.agents[0]).getByRole("checkbox")).toBeChecked();
  await page.close();
});

test("two agents ticked and called: both are seated, one request each, a result for each", async ({ browser, request }) => {
  const tag = tagOf();
  const b = await building(request, tag);
  const page = await openAsMaster(browser, b.m, b.room);
  const calls = [];
  page.on("request", r => { if (r.method() === "POST" && r.url().endsWith(`/rooms/${b.room}/call`)) calls.push(JSON.parse(r.postData())); });
  await page.locator("#callBtn").click();
  await expect(page.locator("#callSubmit")).toBeDisabled();
  await row(page, b.agents[0]).getByRole("checkbox").check();
  await row(page, b.agents[1]).getByRole("checkbox").check();
  await expect(page.locator("#callSubmit")).toHaveText("Call 2");
  await page.locator("#callSubmit").click();
  const results = dialog(page).locator(".callresult");
  await expect(results).toHaveCount(2);
  await expect(results.filter({ hasText: b.agents[0] })).toContainText("called in");
  await expect(results.filter({ hasText: b.agents[1] })).toContainText("called in");
  expect(calls).toEqual([{ name: b.agents[0] }, { name: b.agents[1] }]);
  const invites = await (await request.get(`/rooms/${b.room}/invites`, { headers: MASTER })).json();
  expect(invites.map(i => i.name).sort()).toEqual([b.agents[0], b.agents[1]].sort());
  // Back to the list: the two now have seats and are not offered again.
  await page.locator("#callAgain").click();
  await expect(row(page, b.agents[2])).toBeVisible();
  await expect(row(page, b.agents[0])).toHaveCount(0);
  await page.close();
});

test("a floor loca offers only the floor's people", async ({ browser, request }) => {
  const tag = tagOf();
  const b = await building(request, tag, { floor: true });
  const profiles = await (await request.get("/profiles", { headers: b.as })).json();
  for (const name of [b.agents[1], b.person]) {
    const id = profiles.find(p => p.display_name === name).id;
    expect((await request.put(`/groups/${b.group}/members/${id}`, { headers: b.as })).ok()).toBeTruthy();
  }
  const page = await openAsMaster(browser, b.m, b.room);
  await page.locator("#callBtn").click();
  await expect(dialog(page).locator(".callcand")).toHaveCount(2);
  await expect(row(page, b.agents[1])).toBeVisible();
  await expect(row(page, b.person)).toBeVisible();
  await expect(row(page, b.agents[1]).locator(".callfloors")).toContainText(`${tag}-floor`);
  await expect(dialog(page)).toContainText(`Only ${tag}-floor's people can be called into this loca.`);
  await page.close();
});

test("a partial failure is listed beside the calls that worked", async ({ browser, request }) => {
  const tag = tagOf();
  const b = await building(request, tag);
  const page = await openAsMaster(browser, b.m, b.room);
  // The server refuses one of the two (a synthetic refusal; the other call is real).
  await page.route(`**/rooms/${b.room}/call`, async (route) => {
    const body = JSON.parse(route.request().postData() || "{}");
    if (body.name === b.agents[1]) {
      return route.fulfill({ status: 409, contentType: "text/plain", body: "this loca has no free seat" });
    }
    return route.continue();
  });
  await page.locator("#callBtn").click();
  await row(page, b.agents[0]).getByRole("checkbox").check();
  await row(page, b.agents[1]).getByRole("checkbox").check();
  await page.locator("#callSubmit").click();
  const results = dialog(page).locator(".callresult");
  await expect(results).toHaveCount(2);
  await expect(results.filter({ hasText: b.agents[0] })).toContainText("called in");
  await expect(results.filter({ hasText: b.agents[1] })).toContainText("not called: this loca has no free seat");
  await expect(dialog(page).locator("#callSummary")).toHaveText("1 of 2 called in. 1 could not be called.");
  await page.close();
});

test("on a phone the dialog is a full sheet, and Tab stays inside it", async ({ browser, request }) => {
  const tag = tagOf();
  const b = await building(request, tag);
  const page = await openAsMaster(browser, b.m, b.room, { width: 390, height: 844 });
  await page.locator("#callBtn").click();
  const card = await dialog(page).locator(".extcard").boundingBox();
  expect(Math.round(card.x)).toBe(0);
  expect(Math.round(card.y)).toBe(0);
  expect(Math.round(card.width)).toBe(390);
  expect(Math.round(card.height)).toBe(844);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  // No row runs past the sheet.
  const wide = await page.evaluate(() => [...document.querySelectorAll("#callDialog .callcand")]
    .filter(r => r.scrollWidth > r.clientWidth + 1 || r.getBoundingClientRect().right > window.innerWidth).length);
  expect(wide).toBe(0);
  // Focus is trapped: thirty Tabs never leave the dialog.
  for (let i = 0; i < 30; i++) {
    await page.keyboard.press("Tab");
    expect(await page.evaluate(() => document.getElementById("callDialog").contains(document.activeElement))).toBe(true);
  }
  await page.close();
});
