const { test, expect } = require("@playwright/test");

// A task's view shows its review history, read-only: each request for
// review, go and no-go, with who, when, and the note or the reason.

const MASTER = { "x-admin-token": "MASTER" };

async function memberPage(browser, request, name, room) {
  const { token } = await (await request.post("/members", { headers: MASTER, data: { name, kind: "user" } })).json();
  const invited = await request.post(`/rooms/${room}/invites`, { headers: MASTER, data: { name } });
  expect(invited.ok(), await invited.text()).toBeTruthy();
  const page = await browser.newPage();
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

// PATCH a task as the page's own person.
const move = (page, room, id, body) => page.evaluate(async ([room, id, body]) => {
  const r = await fetch(`${serverBase()}/rooms/${room}/tasks/${id}`, {
    method: "PATCH", headers: adminHeaders({ "content-type": "application/json" }),
    body: JSON.stringify({ ...body, by: state.name }),
  });
  return r.status;
}, [room, id, body]);

test("the task view lists each ask, go and no-go with who and why", async ({ browser, request }) => {
  const room = "rh-view";
  await request.post("/rooms", { headers: MASTER, data: { name: room, extensions: ["notes"] } });
  const ada = await memberPage(browser, request, "rh-ada", room);
  const bob = await memberPage(browser, request, "rh-bob", room);
  expect((await request.post(`/rooms/${room}/lead`, { headers: MASTER, data: { lead: "rh-ada" } })).ok()).toBeTruthy();
  expect((await request.put(`/rooms/${room}/extensions/board`, { headers: MASTER })).ok()).toBeTruthy();
  await expect.poll(() => ada.evaluate(() => mayDeclareTasks() && extensionOn("board"))).toBe(true);
  const id = await ada.evaluate(async room => {
    const r = await fetch(`${serverBase()}/rooms/${room}/tasks`, {
      method: "POST", headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ title: "fix the session renewal", by: state.name, assigned_to: "rh-bob" }),
    });
    return (await r.json()).id;
  }, room);
  expect(await move(bob, room, id, { status: "taken" })).toBe(200);
  expect(await move(bob, room, id, { status: "done", note: "renewal fixed; run make e2e" })).toBe(200);
  expect(await move(ada, room, id, { status: "taken", reason: "the expiry path is untested" })).toBe(200);

  await bob.evaluate(id => openTaskView(id), id);
  const history = bob.locator("#taskViewBody .taskhistory li");
  await expect(history).toHaveCount(2);
  await expect(history.nth(0)).toContainText("rh-bob asked for review");
  await expect(history.nth(0)).toContainText("renewal fixed; run make e2e");
  await expect(history.nth(1)).toContainText("rh-ada: not done yet");
  await expect(history.nth(1)).toContainText("the expiry path is untested");
  await expect(history.nth(0).locator("time")).toHaveCount(1);
  // The view follows a new move live.
  expect(await move(bob, room, id, { status: "done", note: "expiry covered" })).toBe(200);
  expect(await move(ada, room, id, { status: "done" })).toBe(200);
  await expect(history).toHaveCount(4);
  await expect(history.nth(3)).toContainText("rh-ada: go");
  // Read-only: nothing in it can be changed.
  await expect(bob.locator("#taskViewBody .taskhistory").locator("button, input, textarea")).toHaveCount(0);
  for (const page of [ada, bob]) await page.close();
});

test("a finished task shows what was delivered, its link clickable", async ({ browser, request }) => {
  const room = "rh-delivered";
  await request.post("/rooms", { headers: MASTER, data: { name: room, extensions: ["notes"] } });
  const ada = await memberPage(browser, request, "rh-ada2", room);
  const bob = await memberPage(browser, request, "rh-bob2", room);
  expect((await request.post(`/rooms/${room}/lead`, { headers: MASTER, data: { lead: "rh-ada2" } })).ok()).toBeTruthy();
  expect((await request.put(`/rooms/${room}/extensions/board`, { headers: MASTER })).ok()).toBeTruthy();
  await expect.poll(() => ada.evaluate(() => mayDeclareTasks() && extensionOn("board"))).toBe(true);
  const id = await ada.evaluate(async room => {
    const r = await fetch(`${serverBase()}/rooms/${room}/tasks`, {
      method: "POST", headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ title: "fix the login", by: state.name, assigned_to: "rh-bob2" }),
    });
    return (await r.json()).id;
  }, room);
  expect(await move(bob, room, id, { status: "taken" })).toBe(200);
  expect(await move(bob, room, id, { status: "done", note: "first try" })).toBe(200);
  expect(await move(ada, room, id, { status: "taken", reason: "untested" })).toBe(200);
  const pr = "https://github.com/example/app/pull/34";
  expect(await move(bob, room, id, { status: "done", note: `tested; see ${pr}.` })).toBe(200);
  expect(await move(ada, room, id, { status: "done" })).toBe(200);

  // The card on the board: the last note asked with, the link clickable.
  await ada.locator("#tabBoard").click();
  const card = ada.locator(`#boardPanel .task[data-tid="${id}"] .tdelivered`);
  await expect(card).toContainText(`Delivered: tested; see ${pr}.`);
  await expect(card.locator("a")).toHaveAttribute("href", pr);
  await expect(card.locator("a")).toHaveAttribute("rel", "noopener noreferrer");
  // The task view says it too.
  await bob.evaluate(id => openTaskView(id), id);
  const facts = bob.locator("#taskViewBody .taskfacts");
  await expect(facts).toContainText(`Delivered`);
  await expect(facts.locator(`a[href="${pr}"]`)).toHaveCount(1);
  for (const page of [ada, bob]) await page.close();
});
