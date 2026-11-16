const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

// A person with an unfinished task wears its number beside their name in the
// roster and on their lines. The task belongs to the principal the server
// recorded for it, never to whoever wears the name now (the rank and
// operator badges follow the same rule, rank-not-operator.spec.js).

const MASTER = { "x-admin-token": "MASTER" };

async function masterPage(browser, request, room) {
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const info = await (await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" },
  })).json();
  const page = await browser.newPage();
  await page.addInitScript(({ token, expiresAt, room }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "" }));
    localStorage.setItem("loca-seated-rooms", JSON.stringify([room]));
  }, { token: info.session_token, expiresAt: info.expires_at, room });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await enter(page, room);
  // This server requires sessions: the Master's writes carry this one.
  page.sessionHeaders = { "x-session-token": info.session_token };
  return page;
}

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
  await enter(page, room);
  return page;
}

async function enter(page, room) {
  await page.evaluate(room => joinRoom(room), room);
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
}

async function say(page, text) {
  await page.fill("#msg", text);
  await page.keyboard.press("Enter");
  await expect(page.locator("#feed .row", { hasText: text })).toHaveCount(1);
}

const lineTag = (page, text) => page.locator("#feed .row", { hasText: text }).locator(".roletags .tasktag");
const seatTag = (page, name) => page.locator("#onlineList .omem", { hasText: name }).locator(".tasktag");

test("the assignee wears the task number in the roster and on their lines until it is done", async ({ browser, request }) => {
  await provisionRooms(request, ["task-badge"]);
  const master = await masterPage(browser, request, "task-badge");
  const ada = await memberPage(browser, request, "tb-ada", "task-badge");
  await say(ada, "a line by ada");
  await say(master, "a line by the master");

  const created = await request.post("/rooms/task-badge/tasks", {
    headers: master.sessionHeaders, data: { title: "review the login fix", by: "x", assigned_to: "tb-ada" },
  });
  expect(created.status(), await created.text()).toBe(201);
  const task = await created.json();
  expect(task.assigned_principal).toMatch(/^pr_/);

  for (const viewer of [master, ada]) {
    await expect(lineTag(viewer, "a line by ada")).toHaveText(`#${task.id}`);
    await expect(lineTag(viewer, "a line by ada")).toHaveAttribute("title", `#${task.id} review the login fix (To do)`);
    await expect(seatTag(viewer, "tb-ada")).toHaveText(`#${task.id}`);
    await expect(lineTag(viewer, "a line by the master")).toHaveCount(0);
  }

  // A second task: the one in progress is shown first, with a count.
  const second = await (await request.post("/rooms/task-badge/tasks", {
    headers: master.sessionHeaders, data: { title: "write the note", by: "x", assigned_to: "tb-ada" },
  })).json();
  await ada.evaluate(id => taskAct("take", id), second.id);
  await expect(seatTag(master, "tb-ada")).toHaveText(`#${second.id} +1`);

  // Finished or removed tasks are not worn.
  for (const [id, status] of [[second.id, "done"], [task.id, "cancelled"]]) {
    const r = await request.patch(`/rooms/task-badge/tasks/${id}`, { headers: master.sessionHeaders, data: { status, by: "x" } });
    expect(r.ok(), await r.text()).toBeTruthy();
  }
  await expect(seatTag(master, "tb-ada")).toHaveCount(0);
  await expect(lineTag(master, "a line by ada")).toHaveCount(0);
  for (const page of [master, ada]) await page.close();
});

test("a task is not worn by someone else who now wears its assignee's name", async ({ browser, request }) => {
  await provisionRooms(request, ["task-badge-reuse"]);
  const master = await masterPage(browser, request, "task-badge-reuse");
  const ada = await memberPage(browser, request, "tb-reuse", "task-badge-reuse");
  await say(ada, "a verified line");
  await expect(master.locator("#feed .row", { hasText: "a verified line" }).locator(".roletags"))
    .toHaveAttribute("data-writer", /^pr_/);

  // The task belongs to another principal that once wore this name.
  await master.evaluate(() => {
    state.tasks[900] = { id: 900, room: state.room, title: "theirs", created_by: "x", assigned_to: "tb-reuse",
      assigned_principal: "pr_someone_else", status: "taken", created_at: Date.now(), progress_at: Date.now() };
    refreshTaskBadges();
  });
  await expect(lineTag(master, "a verified line")).toHaveCount(0);
  await expect(seatTag(master, "tb-reuse")).toHaveCount(0);

  // A task given by name alone is not worn by a verified person either; only
  // a line nobody verified, which is itself only a name, wears it.
  await master.evaluate(() => {
    state.tasks[900] = { ...state.tasks[900], assigned_principal: undefined };
    addMsg({ id: 990000101, room: state.room, sender: "tb-reuse", sender_type: "user", ts: Date.now(), text: "an unverified line" });
    refreshTaskBadges();
  });
  await expect(lineTag(master, "a verified line")).toHaveCount(0);
  await expect(seatTag(master, "tb-reuse")).toHaveCount(0);
  await expect(lineTag(master, "an unverified line")).toHaveText("#900");
  for (const page of [master, ada]) await page.close();
});
