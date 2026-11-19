const { test, expect } = require("@playwright/test");

// The Loca Board's list is read once (GET /rooms/:id/tasks) and then kept by
// the live `task` frames. A task created while that read is on its way must
// not be lost when the older answer arrives: the answer is the past, the
// frame the present. (Seen in the gate: a fifth task created during the read
// vanished from the List until reload.)

const MASTER = { "x-admin-token": "MASTER" };

test("a task created while the task list is being read stays on the board", async ({ browser, request }) => {
  const room = "task-race";
  await request.post("/rooms", { headers: MASTER, data: { name: room, extensions: ["notes"] } });
  // Tasks are declared under a Lead, on the Loca Board (as board-list does).
  expect((await request.post(`/rooms/${room}/lead`, { headers: MASTER, data: { lead: "race-lead" } })).ok()).toBeTruthy();
  expect((await request.put(`/rooms/${room}/extensions/board`, { headers: MASTER })).ok()).toBeTruthy();
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const info = await (await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" },
  })).json();
  const s = { "x-session-token": info.session_token };
  expect((await request.post(`/rooms/${room}/tasks`, { headers: s, data: { title: "before the read", by: "x" } })).ok()).toBeTruthy();

  const page = await browser.newPage();
  await page.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "" }));
  }, { token: info.session_token, expiresAt: info.expires_at });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate(room => joinRoom(room), room);
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  await expect.poll(() => page.evaluate(() => extensionOn("board"))).toBe(true);

  // Hold the next task-list answer until a new task has been created and its
  // frame has reached the page.
  let release, answered;
  const held = new Promise(resolve => { release = resolve; });
  const read = new Promise(resolve => { answered = resolve; });
  await page.route(url => new URL(url).pathname === `/rooms/${room}/tasks`, async route => {
    const answer = await route.fetch();
    const body = await answer.text();
    answered();
    await held;
    await route.fulfill({ response: answer, body });
  });
  const reading = page.evaluate(() => fetchTasks());
  // The server has answered (without the new task) before it is created.
  await read;
  expect((await request.post(`/rooms/${room}/tasks`, { headers: s, data: { title: "during the read", by: "x" } })).ok()).toBeTruthy();
  await expect.poll(() => page.evaluate(() => Object.values(state.tasks).map(t => t.title))).toContain("during the read");
  release();
  await reading;

  const titles = await page.evaluate(() => Object.values(state.tasks).map(t => t.title).sort());
  expect(titles).toEqual(["before the read", "during the read"]);
  await page.close();
});
