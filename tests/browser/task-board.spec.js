const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

// The Loca Board: with the `board` extension on, its tab shows four
// columns; the holder's "done" asks for review; the Lead says go or no-go
// (with a reason), and a participant is offered neither. With it off there
// is no list at all (loca-board-tab.spec.js).

const MASTER = { "x-admin-token": "MASTER" };

async function memberPage(browser, request, name, room, before = async () => {}) {
  const { token } = await (await request.post("/members", { headers: MASTER, data: { name, kind: "user" } })).json();
  const invited = await request.post(`/rooms/${room}/invites`, { headers: MASTER, data: { name } });
  expect(invited.ok(), await invited.text()).toBeTruthy();
  const page = await browser.newPage();
  await before(page);
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

test("four columns, a review step, and go / no-go for whoever gave the task", async ({ browser, request }) => {
  const room = "task-board";
  await provisionRooms(request, [room]);
  const lead = await memberPage(browser, request, "bd-lead", room);
  const bob = await memberPage(browser, request, "bd-bob", room);
  expect((await request.post(`/rooms/${room}/lead`, { headers: MASTER, data: { lead: "bd-lead" } })).ok()).toBeTruthy();
  const on = await request.put(`/rooms/${room}/extensions/board`, { headers: MASTER });
  expect(on.ok(), await on.text()).toBeTruthy();

  for (const page of [lead, bob]) {
    await expect.poll(() => page.evaluate(() => extensionOn("board"))).toBe(true);
    await page.evaluate(() => switchTab("board"));
  }
  // The Lead declares the task from the Loca Board tab, for bob.
  await lead.locator("#taskCreate summary").click();
  await lead.locator("#tkTitle").fill("fix the session renewal");
  await lead.locator("#tkAssign").selectOption("bd-bob");
  await lead.locator("#tkCreate").click();
  for (const page of [lead, bob]) {
    await expect(page.locator("#taskList .boardcol .boardhead")).toHaveText(["To do · 1", "Doing · 0", "Review · 0", "Done · 0"]);
    await expect(card(page, "open", "fix the session renewal")).toHaveCount(1);
  }

  await card(bob, "open", "fix the session renewal").getByRole("button", { name: "Start" }).click();
  await expect(card(bob, "taken", "fix the session renewal")).toHaveCount(1);
  // Asking for review opens the card's own form, with an optional note.
  await card(bob, "taken", "fix the session renewal").getByRole("button", { name: "Ask for review" }).click();
  await card(bob, "taken", "fix the session renewal").locator(".tform textarea").fill("renews now; see the test");
  await card(bob, "taken", "fix the session renewal").getByRole("button", { name: "Send for review" }).click();
  for (const page of [lead, bob]) await expect(card(page, "review", "fix the session renewal")).toHaveCount(1);
  await expect(card(lead, "review", "fix the session renewal")).toContainText("Note: renews now; see the test");
  // The holder is not offered the review; the Lead is.
  await expect(card(bob, "review", "fix the session renewal").getByRole("button", { name: /^(Approve|Request changes)$/ })).toHaveCount(0);
  await expect(card(lead, "review", "fix the session renewal").getByRole("button", { name: "Approve", exact: true })).toHaveCount(1);
  // The badge counts a task in review, with its state in the tooltip.
  const tasks = await (await request.get(`/rooms/${room}/tasks`, { headers: MASTER })).json();
  const task = tasks.find(t => t.title === "fix the session renewal");
  await expect(lead.locator("#onlineList .omem", { hasText: "bd-bob" }).locator(".tasktag"))
    .toHaveAttribute("title", `#${task.id} fix the session renewal (Review)`);

  // No-go with a reason sends it back to Doing, where bob reads why.
  // The reason is written on the card, not in a browser prompt.
  lead.on("dialog", dialog => { throw new Error(`a native dialog opened: ${dialog.message()}`); });
  await card(lead, "review", "fix the session renewal").getByRole("button", { name: "Request changes" }).click();
  await card(lead, "review", "fix the session renewal").locator(".tform textarea").fill("the expiry path is untested");
  await card(lead, "review", "fix the session renewal").getByRole("button", { name: "Send back" }).click();
  await expect(card(bob, "taken", "fix the session renewal")).toContainText("Not yet: the expiry path is untested");
  await card(bob, "taken", "fix the session renewal").getByRole("button", { name: "Ask for review" }).click();
  await card(bob, "taken", "fix the session renewal").getByRole("button", { name: "Send for review" }).click();
  await expect(card(lead, "review", "fix the session renewal")).toHaveCount(1);
  await card(lead, "review", "fix the session renewal").getByRole("button", { name: "Approve", exact: true }).click();
  for (const page of [lead, bob]) await expect(card(page, "done", "fix the session renewal")).toHaveCount(1);
  for (const page of [lead, bob]) await page.close();
});

test("a card is dragged to the column of a move its person may make; the others stay shut", async ({ browser, request }) => {
  const room = "task-board-drag";
  await provisionRooms(request, [room]);
  const lead = await memberPage(browser, request, "dg-lead", room);
  const bob = await memberPage(browser, request, "dg-bob", room);
  expect((await request.post(`/rooms/${room}/lead`, { headers: MASTER, data: { lead: "dg-lead" } })).ok()).toBeTruthy();
  const on = await request.put(`/rooms/${room}/extensions/board`, { headers: MASTER });
  expect(on.ok(), await on.text()).toBeTruthy();
  for (const page of [lead, bob]) {
    await expect.poll(() => page.evaluate(() => extensionOn("board"))).toBe(true);
    await page.evaluate(() => switchTab("board"));
  }
  // One task the Lead gives to bob, one the Operator gives to bob.
  await lead.locator("#taskCreate summary").click();
  await lead.locator("#tkTitle").fill("from the lead");
  await lead.locator("#tkAssign").selectOption("dg-bob");
  await lead.locator("#tkCreate").click();
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const master = await (await request.post("/sessions", { headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" } })).json();
  const made = await request.post(`/rooms/${room}/tasks`, {
    headers: { "x-session-token": master.session_token }, data: { title: "from the operator", by: "x", assigned_to: "dg-bob" },
  });
  expect(made.ok(), await made.text()).toBeTruthy();
  await expect(card(bob, "open", "from the operator")).toHaveCount(1);

  // While bob holds his card, only Doing opens; a drop there starts it.
  const held = card(bob, "open", "from the lead");
  await expect(held).toHaveAttribute("draggable", "true");
  await held.dispatchEvent("dragstart", { dataTransfer: await bob.evaluateHandle(() => new DataTransfer()) });
  await expect(column(bob, "taken")).toHaveClass(/dropok/);
  await expect(column(bob, "review")).toHaveClass(/dropno/);
  await expect(column(bob, "done")).toHaveClass(/dropno/);
  await held.dispatchEvent("dragend");
  await expect(column(bob, "review")).not.toHaveClass(/dropno/);
  await held.dragTo(column(bob, "taken"));
  await expect(card(bob, "taken", "from the lead")).toHaveCount(1);

  // Dropped on Review, it asks for the review note first.
  await card(bob, "taken", "from the lead").dragTo(column(bob, "review"));
  await card(bob, "taken", "from the lead").getByRole("button", { name: "Send for review" }).click();
  await expect(card(lead, "review", "from the lead")).toHaveCount(1);
  // The Lead gave it, so the Lead approves it by dropping it on Done.
  await card(lead, "review", "from the lead").dragTo(column(lead, "done"));
  await expect(card(bob, "done", "from the lead")).toHaveCount(1);

  // The Operator's task waits for the Operator: the Lead is offered no review.
  await card(bob, "open", "from the operator").getByRole("button", { name: "Start" }).click();
  await card(bob, "taken", "from the operator").getByRole("button", { name: "Ask for review" }).click();
  await card(bob, "taken", "from the operator").getByRole("button", { name: "Send for review" }).click();
  await expect(card(lead, "review", "from the operator")).toHaveCount(1);
  await expect(card(lead, "review", "from the operator").getByRole("button", { name: /^(Approve|Request changes)$/ })).toHaveCount(0);
  await expect(card(lead, "review", "from the operator")).not.toHaveAttribute("draggable", "true");
  for (const page of [lead, bob]) await page.close();
});
