const { test, expect } = require("@playwright/test");

// Each task is an object: a view of its own, opened from its card, from
// "task #N" in chat, from a roster badge or from its address
// `#board/<loca>/<n>`, showing only the actions this viewer may take.

const MASTER = { "x-admin-token": "MASTER" };

async function session(request) {
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  return (await request.post("/sessions", { headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" } })).json();
}

async function memberPage(browser, request, name, room) {
  const { token } = await (await request.post("/members", { headers: MASTER, data: { name, kind: "user" } })).json();
  const invited = await request.post(`/rooms/${room}/invites`, { headers: MASTER, data: { name } });
  expect(invited.ok(), await invited.text()).toBeTruthy();
  const context = await browser.newContext();
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  const page = await context.newPage();
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await page.goto("/");
  await page.locator("#entryKind").selectOption("human");
  await page.locator("#name").fill(name);
  await page.locator("#roomToken").fill(token);
  await page.locator("#connectBtn").click();
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate(room => joinRoom(room), room);
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  return page;
}

// A board whose Lead tv-ada declared one task for tv-bob: [ada, bob, carol, id].
async function boardWithTask(browser, request, room) {
  await request.post("/rooms", { headers: MASTER, data: { name: room, extensions: ["notes", "focus"] } });
  const ada = await memberPage(browser, request, `${room}-ada`, room);
  const bob = await memberPage(browser, request, `${room}-bob`, room);
  const carol = await memberPage(browser, request, `${room}-carol`, room);
  expect((await request.post(`/rooms/${room}/lead`, { headers: MASTER, data: { lead: `${room}-ada` } })).ok()).toBeTruthy();
  expect((await request.put(`/rooms/${room}/extensions/board`, { headers: MASTER })).ok()).toBeTruthy();
  await expect.poll(() => ada.evaluate(() => mayDeclareTasks() && extensionOn("board"))).toBe(true);
  const id = await ada.evaluate(async ([room, bob]) => {
    const r = await fetch(`${serverBase()}/rooms/${room}/tasks`, {
      method: "POST", headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ title: "write the upgrade note", by: state.name, assigned_to: bob }),
    });
    return (await r.json()).id;
  }, [room, `${room}-bob`]);
  return [ada, bob, carol, id];
}

const view = page => page.locator("#taskView");

test("a card opens the task's view, with its facts, its address and a copyable link", async ({ browser, request }) => {
  const room = "tv-card";
  const [ada, bob, carol, id] = await boardWithTask(browser, request, room);
  await bob.locator("#tabBoard").click();
  await bob.locator(`#taskList .task[data-tid="${id}"] .ttitle`).click();
  await expect(view(bob)).toBeVisible();
  await expect(bob.locator("#taskViewTitle")).toHaveText(`#${id} write the upgrade note`);
  const facts = bob.locator("#taskViewBody .taskfacts");
  await expect(facts).toContainText("To do");
  await expect(facts).toContainText(`Holder${room}-bob`);
  await expect(facts).toContainText(`Declared byLead ${room}-ada`);
  await expect(facts).toContainText(`Assigned byLead ${room}-ada`);
  await expect(facts).toContainText("Declared");
  // The holder may start it; nobody offers it a review yet.
  await expect(bob.locator("#taskViewBody .taskviewacts button")).toHaveText(["Start"]);
  expect(await bob.evaluate(() => location.hash)).toBe(`#board/${room}/${id}`);
  await bob.locator("#taskViewCopy").click();
  await expect(bob.locator("#taskViewCopied")).toHaveText("Link copied.");
  expect(await bob.evaluate(() => navigator.clipboard.readText())).toMatch(new RegExp(`#board/${room}/${id}$`));
  // An action in the view moves the task, and the view follows.
  await bob.locator("#taskViewBody .taskviewacts").getByRole("button", { name: "Start" }).click();
  await expect(facts).toContainText("In progress");
  await bob.locator("#taskViewClose").click();
  await expect(view(bob)).toBeHidden();
  expect(await bob.evaluate(() => location.hash)).toBe("");
  for (const page of [ada, bob, carol]) await page.context().close();
});

test("task #N in chat and a roster badge open the task; a bystander is offered no action", async ({ browser, request }) => {
  const room = "tv-links";
  const [ada, bob, carol, id] = await boardWithTask(browser, request, room);
  // The server's assignment line, and a person's own words.
  await carol.fill("#msg", `I can help with task #${id} later`);
  await carol.keyboard.press("Enter");
  const line = carol.locator("#feed .row", { hasText: `Lead ${room}-ada assigned task #${id}` });
  await expect(line.locator(`a[data-task-ref="${id}"]`)).toHaveCount(1);
  const own = carol.locator("#feed .row", { hasText: "I can help with task" });
  const entries = await carol.evaluate(() => history.length);
  await own.locator(`a[data-task-ref="${id}"]`).click();
  await expect(view(carol)).toBeVisible();
  // The address is replaced, never pushed: Back does not walk through tasks.
  expect(await carol.evaluate(() => history.length)).toBe(entries);
  // carol is neither holder, Lead nor operator: no action, no reassigning.
  await expect(carol.locator("#taskViewBody .taskviewacts button")).toHaveCount(0);
  await expect(carol.locator("#taskReassign")).toHaveCount(0);
  await carol.locator("#taskViewClose").click();
  // The badge bob wears in the roster, under This Loca.
  await carol.locator("#sideLocaTab").click();
  const badge = carol.locator("#onlineList .omem", { hasText: `${room}-bob` }).locator(".tasktag");
  await expect(badge).toHaveText(`#${id}`);
  await badge.click();
  await expect(carol.locator("#taskViewTitle")).toHaveText(`#${id} write the upgrade note`);
  // The Lead may give it to someone else.
  await ada.evaluate(id => openTaskView(id), id);
  await expect(ada.locator("#taskReassign")).toHaveCount(1);
  for (const page of [ada, bob, carol]) await page.context().close();
});

test("the address #board/<loca>/<n> opens the loca's board and the task", async ({ browser, request }) => {
  const room = "tv-address";
  const [ada, bob, carol, id] = await boardWithTask(browser, request, room);
  for (const page of [ada, bob, carol]) await page.context().close();
  const info = await session(request);
  const page = await browser.newPage();
  await page.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "Master" }));
  }, { token: info.session_token, expiresAt: info.expires_at });
  await page.goto(`/#board/${room}/${id}`);
  await expect(view(page)).toBeVisible({ timeout: 15000 });
  await expect(page.locator("#taskViewTitle")).toHaveText(`#${id} write the upgrade note`);
  expect(await page.evaluate(() => [state.room, state.tab])).toEqual([room, "board"]);
  // A number the loca does not have says so.
  await page.locator("#taskViewClose").click();
  await page.evaluate(room => { location.hash = `#board/${room}/999`; }, room);
  await expect(page.locator("#feed")).toContainText("There is no task #999 in this loca.");
  await page.close();
});
