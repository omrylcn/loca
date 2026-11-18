const { test, expect } = require("@playwright/test");

// The Loca Board's List and Board views: filters for Mine, holder and
// stage, sort by last change (default), number or stage; the choice of view
// is remembered by this browser; on a phone the List comes first and the
// Board shows one stage at a time.

const MASTER = { "x-admin-token": "MASTER" };

async function session(request) {
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  return (await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" },
  })).json();
}

async function memberPage(browser, request, name, room, viewport) {
  const { token } = await (await request.post("/members", { headers: MASTER, data: { name, kind: "user" } })).json();
  const invited = await request.post(`/rooms/${room}/invites`, { headers: MASTER, data: { name } });
  expect(invited.ok(), await invited.text()).toBeTruthy();
  const page = await browser.newPage(viewport ? { viewport } : {});
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

// A board with five tasks: #1 to do (bl-bob), #2 doing (bl-cem), #3 in
// review (bl-bob), #4 done (bl-cem), #5 to do (nobody); #3 moves last.
async function seed(request, room) {
  expect((await request.post(`/rooms/${room}/lead`, { headers: MASTER, data: { lead: "bl-lead" } })).ok()).toBeTruthy();
  expect((await request.put(`/rooms/${room}/extensions/board`, { headers: MASTER })).ok()).toBeTruthy();
  const s = { "x-session-token": (await session(request)).session_token };
  const plan = [["first to do", "bl-bob"], ["doing", "bl-cem"], ["in review", "bl-bob"], ["finished", "bl-cem"], ["unassigned", null]];
  const ids = [];
  for (const [title, assigned_to] of plan) {
    ids.push((await (await request.post(`/rooms/${room}/tasks`, { headers: s, data: { title, by: "x", assigned_to } })).json()).id);
  }
  const pause = () => new Promise(resolve => setTimeout(resolve, 15));
  for (const [i, status] of [[1, "taken"], [3, "taken"], [3, "done"], [2, "taken"], [2, "review"]]) {
    await pause();
    const r = await request.patch(`/rooms/${room}/tasks/${ids[i]}`, { headers: s, data: { status, by: "x" } });
    expect(r.ok(), await r.text()).toBeTruthy();
  }
  return ids;
}

const rowTitles = page => page.locator("#taskList .boardlist .task .ttitle").evaluateAll(rows =>
  rows.map(row => row.firstChild.textContent.trim()));

test("the List filters by Mine, holder and stage, and sorts", async ({ browser, request }) => {
  const room = "bl-list";
  await request.post("/rooms", { headers: MASTER, data: { name: room, extensions: ["notes"] } });
  const bob = await memberPage(browser, request, "bl-bob", room);
  await memberPage(browser, request, "bl-cem", room).then(page => page.close());
  await seed(request, room);
  await expect.poll(() => bob.evaluate(() => extensionOn("board"))).toBe(true);
  await bob.locator("#tabBoard").click();
  await bob.locator('#boardTools [data-board-view="list"]').click();
  await expect(bob.locator("#taskList .boardlist")).toBeVisible();
  // Last change first; Removed hidden; every stage but Removed.
  await expect.poll(() => rowTitles(bob)).toEqual(["in review", "finished", "doing", "unassigned", "first to do"]);
  await bob.locator("#boardSort").selectOption("number");
  await expect.poll(() => rowTitles(bob)).toEqual(["first to do", "doing", "in review", "finished", "unassigned"]);
  await bob.locator("#boardSort").selectOption("stage");
  await expect.poll(() => rowTitles(bob)).toEqual(["unassigned", "first to do", "doing", "in review", "finished"]);
  await bob.locator("#boardMine").check();
  await expect.poll(() => rowTitles(bob)).toEqual(["first to do", "in review"]);
  await bob.locator("#boardMine").uncheck();
  await bob.locator("#boardHolder").selectOption("bl-cem");
  await expect.poll(() => rowTitles(bob)).toEqual(["doing", "finished"]);
  await bob.locator("#boardHolder").selectOption("");
  await bob.locator("#boardStage").selectOption("open");
  await expect.poll(() => rowTitles(bob)).toEqual(["unassigned", "first to do"]);
  await bob.close();
});

test("the chosen view is remembered by this browser", async ({ browser, request }) => {
  const room = "bl-remember";
  await request.post("/rooms", { headers: MASTER, data: { name: room, extensions: ["notes"] } });
  const bob = await memberPage(browser, request, "bl-dan", room);
  await seed(request, room);
  await expect.poll(() => bob.evaluate(() => extensionOn("board"))).toBe(true);
  await bob.locator("#tabBoard").click();
  // A wide screen starts on the Board.
  await expect(bob.locator("#taskList .taskboard")).toBeVisible();
  await bob.locator('#boardTools [data-board-view="list"]').click();
  await expect(bob.locator("#taskList .boardlist")).toBeVisible();
  await bob.reload();
  await expect.poll(() => bob.evaluate(() => state.authStatus)).toBe("authenticated");
  await bob.evaluate(room => joinRoom(room), room);
  await expect.poll(() => bob.evaluate(() => extensionOn("board"))).toBe(true);
  await bob.locator("#tabBoard").click();
  await expect(bob.locator("#taskList .boardlist")).toBeVisible();
  await expect(bob.locator('#boardTools [data-board-view="list"]')).toHaveAttribute("aria-checked", "true");
  await bob.close();
});

test("on a phone the List comes first and the Board shows one stage at a time", async ({ browser, request }) => {
  const room = "bl-phone";
  await request.post("/rooms", { headers: MASTER, data: { name: room, extensions: ["notes"] } });
  const bob = await memberPage(browser, request, "bl-eli", room, { width: 390, height: 844 });
  await seed(request, room);
  await expect.poll(() => bob.evaluate(() => extensionOn("board"))).toBe(true);
  await bob.evaluate(() => switchTab("board"));
  await expect(bob.locator("#taskList .boardlist")).toBeVisible();
  await bob.locator('#boardTools [data-board-view="board"]').click();
  await expect(bob.locator("#taskList .boardcol")).toHaveCount(1);
  await expect(bob.locator("#taskList .boardcol")).toHaveAttribute("data-column", "open");
  await bob.locator('#taskList [data-board-stage="review"]').click();
  await expect(bob.locator("#taskList .boardcol")).toHaveAttribute("data-column", "review");
  await expect(bob.locator('#taskList [data-board-stage="review"]')).toHaveText("Review · 1");
  await bob.close();
});
