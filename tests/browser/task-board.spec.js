const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

// The Task board: with the `board` extension on, the Focus tab shows four
// columns; the holder's "done" asks for review; the Lead says go or no-go
// (with a reason), and a participant is offered neither. With it off the
// list is as before, and a status the page does not know is shown as is.

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

const column = (page, status) => page.locator(`#taskList .taskboard .boardcol[data-column="${status}"]`);
const card = (page, status, title) => column(page, status).locator(".task", { hasText: title });

test("four columns, a review step, and go / no-go for the Lead only", async ({ browser, request }) => {
  const room = "task-board";
  await provisionRooms(request, [room]);
  const lead = await memberPage(browser, request, "bd-lead", room);
  const bob = await memberPage(browser, request, "bd-bob", room);
  expect((await request.post(`/rooms/${room}/lead`, { headers: MASTER, data: { lead: "bd-lead" } })).ok()).toBeTruthy();
  const on = await request.put(`/rooms/${room}/extensions/board`, { headers: MASTER });
  expect(on.ok(), await on.text()).toBeTruthy();

  for (const page of [lead, bob]) {
    await expect.poll(() => page.evaluate(() => extensionOn("board"))).toBe(true);
    await page.evaluate(() => switchTab("tasks"));
  }
  // The Lead declares the task from the Focus tab, for bob.
  await lead.locator("#taskCreate summary").click();
  await lead.locator("#tkTitle").fill("fix the session renewal");
  await lead.locator("#tkAssign").fill("bd-bob");
  await lead.locator("#tkCreate").click();
  for (const page of [lead, bob]) {
    await expect(page.locator("#taskList .boardcol .boardhead")).toHaveText(["To do · 1", "Doing · 0", "Review · 0", "Done · 0"]);
    await expect(card(page, "open", "fix the session renewal")).toHaveCount(1);
  }

  await card(bob, "open", "fix the session renewal").getByRole("button", { name: "Start" }).click();
  await expect(card(bob, "taken", "fix the session renewal")).toHaveCount(1);
  await card(bob, "taken", "fix the session renewal").getByRole("button", { name: "Ask for review" }).click();
  for (const page of [lead, bob]) await expect(card(page, "review", "fix the session renewal")).toHaveCount(1);
  // The holder is not offered the review; the Lead is.
  await expect(card(bob, "review", "fix the session renewal").getByRole("button", { name: /^(Go|No-go)$/ })).toHaveCount(0);
  await expect(card(lead, "review", "fix the session renewal").getByRole("button", { name: "Go", exact: true })).toHaveCount(1);
  // The badge counts a task in review, with its state in the tooltip.
  const tasks = await (await request.get(`/rooms/${room}/tasks`, { headers: MASTER })).json();
  const task = tasks.find(t => t.title === "fix the session renewal");
  await expect(lead.locator("#onlineList .omem", { hasText: "bd-bob" }).locator(".tasktag"))
    .toHaveAttribute("title", `#${task.id} fix the session renewal (Review)`);

  // No-go with a reason sends it back to Doing, where bob reads why.
  lead.once("dialog", dialog => dialog.accept("the expiry path is untested"));
  await card(lead, "review", "fix the session renewal").getByRole("button", { name: "No-go" }).click();
  await expect(card(bob, "taken", "fix the session renewal")).toContainText("Not yet: the expiry path is untested");
  await card(bob, "taken", "fix the session renewal").getByRole("button", { name: "Ask for review" }).click();
  await expect(card(lead, "review", "fix the session renewal")).toHaveCount(1);
  await card(lead, "review", "fix the session renewal").getByRole("button", { name: "Go", exact: true }).click();
  for (const page of [lead, bob]) await expect(card(page, "done", "fix the session renewal")).toHaveCount(1);
  for (const page of [lead, bob]) await page.close();
});

test("with the board off the list is as before and an unknown status reads as itself", async ({ browser, request }) => {
  const room = "task-board-off";
  await provisionRooms(request, [room]);
  const bob = await memberPage(browser, request, "bd-off", room);
  await bob.evaluate(() => switchTab("tasks"));
  await bob.evaluate(() => {
    const now = Date.now();
    for (const [id, status] of [[901, "review"], [902, "someday"]]) {
      state.tasks[id] = { id, room: state.room, title: `task ${status}`, created_by: "x", status, created_at: now, progress_at: now };
    }
    renderTasks();
  });
  await expect(bob.locator("#taskList .taskboard")).toHaveCount(0);
  await expect(bob.locator("#taskList .task", { hasText: "task review" }).locator(".tchip")).toHaveText("Review");
  // An older page has no label for a newer state: the word itself is shown.
  await expect(bob.locator("#taskList .task", { hasText: "task someday" }).locator(".tchip")).toHaveText("someday");
  await bob.close();
});
