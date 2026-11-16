const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

// The loca's Lead declares and assigns tasks from the Focus tab, like the
// Operator; a participant who is not the Lead is not offered the control.
// The server decides (the Lead is checked by principal); the page only
// stops offering what would be refused.

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

async function openFocus(page) {
  await page.evaluate(() => switchTab("tasks"));
  await expect(page.locator("#tasksPanel")).toBeVisible();
}

test("the Lead sees and uses the task controls; a participant does not see them", async ({ browser, request }) => {
  const room = "lead-tasks";
  await provisionRooms(request, [room]);
  const lead = await memberPage(browser, request, "lt-ada", room);
  const other = await memberPage(browser, request, "lt-bob", room);
  const named = await request.post(`/rooms/${room}/lead`, { headers: MASTER, data: { lead: "lt-ada" } });
  expect(named.ok(), await named.text()).toBeTruthy();
  expect((await named.json()).lead_principal).toMatch(/^pr_/);

  for (const page of [lead, other]) {
    await expect.poll(() => page.evaluate(() => state.settings?.lead)).toBe("lt-ada");
    await openFocus(page);
  }
  await expect(lead.locator("#taskCreate")).toBeVisible();
  await expect(other.locator("#taskCreate")).toBeHidden();

  // The Lead declares a task and gives it to the participant.
  await lead.locator("#taskCreate summary").click();
  await lead.locator("#tkTitle").fill("review the login fix");
  await lead.locator("#tkAssign").fill("lt-bob");
  await lead.locator("#tkCreate").click();
  const row = page => page.locator("#taskList .task", { hasText: "review the login fix" });
  for (const page of [lead, other]) {
    await expect(row(page)).toHaveCount(1);
    await expect(row(page).locator(".tmeta")).toContainText("lt-ada");
    await expect(row(page).locator(".tmeta")).toContainText("lt-bob");
  }
  const tasks = await (await request.get(`/rooms/${room}/tasks`, { headers: MASTER })).json();
  const task = tasks.find(t => t.title === "review the login fix");
  expect(task).toMatchObject({ created_by: "lt-ada", created_as: "lead", assigned_to: "lt-bob", assigned_as: "lead" });

  // The participant wears the badge, its tooltip naming the task and state.
  const seat = other.locator("#onlineList .omem", { hasText: "lt-bob" }).locator(".tasktag");
  await expect(seat).toHaveText(`#${task.id}`);
  await expect(seat).toHaveAttribute("title", `#${task.id} review the login fix (To do)`);

  // The server refuses the participant even if the page were bypassed.
  const refused = await other.evaluate(async room => {
    const r = await fetch(`${serverBase()}/rooms/${room}/tasks`, {
      method: "POST", headers: adminHeaders({ "content-type": "application/json" }),
      body: JSON.stringify({ title: "not mine to declare", by: state.name }),
    });
    return r.status;
  }, room);
  expect(refused).toBe(403);
  for (const page of [lead, other]) await page.close();
});
