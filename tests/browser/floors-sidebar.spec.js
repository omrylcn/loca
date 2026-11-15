const { test, expect } = require("@playwright/test");
const { MASTER } = require("./support/switch");

// Building → Floor → Loca. A group is a floor in the interface and the default
// group is the Ground floor. "Your Locas" lists the locas under floor headings
// that open and close; each viewer sees only the floors of locas the server
// lists for them.

async function masterSession(request) {
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  return (await request.post("/sessions", { headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" } })).json();
}

// Two floors (alpha with two locas, beta with one), a Ground-floor loca, and
// a plain member who is on alpha and invited to the Ground-floor loca only.
async function building(request, tag) {
  const m = await masterSession(request);
  const as = { "x-session-token": m.session_token };
  const floor = async (name) => (await (await request.post("/groups", { headers: as, data: { name } })).json()).id;
  const alpha = await floor(`${tag}-alpha`);
  const beta = await floor(`${tag}-beta`);
  const loca = async (name, group) => {
    const r = await request.post("/rooms", { headers: as, data: group ? { name, group } : { name } });
    expect(r.status(), await r.text()).toBe(201);
  };
  await loca(`${tag}-a1`, alpha);
  await loca(`${tag}-a2`, alpha);
  await loca(`${tag}-b1`, beta);
  await loca(`${tag}-g1`);
  const member = `${tag}-member`;
  const { token } = await (await request.post("/members", { headers: MASTER, data: { name: member, kind: "user" } })).json();
  const profiles = await (await request.get("/profiles", { headers: as })).json();
  const id = profiles.find(p => p.display_name === member).id;
  expect((await request.put(`/groups/${alpha}/members/${id}`, { headers: as })).ok()).toBeTruthy();
  expect((await request.post(`/rooms/${tag}-g1/invites`, { headers: MASTER, data: { name: member } })).ok()).toBeTruthy();
  return { master: m, member, memberKey: token };
}

async function openAsMaster(browser, m) {
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "Master" }));
  }, { token: m.session_token, expiresAt: m.expires_at });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await expect.poll(() => page.evaluate(() => state.principalId)).toBeTruthy();
  return { context, page };
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
  return { context, page };
}

// The floors as drawn: [floor name, [loca names, in order]] for each heading.
async function floors(page) {
  return page.evaluate(() => [...document.querySelectorAll("#roomList details.floor")].map(d => [
    d.querySelector(":scope > summary .rgroupname").textContent,
    [...d.querySelectorAll(":scope > .roomitem .rname")].map(r => r.textContent),
  ]));
}
function floorOf(page, name) {
  return page.locator("#roomList details.floor", { has: page.locator(":scope > summary .rgroupname", { hasText: name }) });
}
function only(list, tag) {
  return list.map(([name, rooms]) => [name, rooms.filter(r => r.startsWith(tag))])
    .filter(([name, rooms]) => name.startsWith(tag) || name === "Ground floor" && rooms.length);
}

test("the Master's locas are listed by floor: Ground floor first, then floors by name", async ({ browser, request }) => {
  const tag = "fl1";
  const { master } = await building(request, tag);
  const { context, page } = await openAsMaster(browser, master);
  await expect.poll(async () => only(await floors(page), tag)).toEqual([
    ["Ground floor", [`${tag}-g1`]],
    [`${tag}-alpha`, [`${tag}-a1`, `${tag}-a2`]],
    [`${tag}-beta`, [`${tag}-b1`]],
  ]);
  // Care, the ranks' own house, is on no floor.
  expect((await floors(page)).flatMap(([, rooms]) => rooms)).not.toContain("iye");
  await context.close();
});

test("Care has its own place above the floors, under no floor heading", async ({ browser, request }) => {
  const tag = "fl9";
  const { master } = await building(request, tag);
  const { context, page } = await openAsMaster(browser, master);
  await expect(floorOf(page, `${tag}-alpha`)).toBeVisible();
  const careLoca = page.locator("#roomList > .roomitem", { hasText: "iye" });
  await expect(careLoca).toHaveCount(1);
  await expect(page.locator("#roomList details.floor .rname", { hasText: /^iye$/ })).toHaveCount(0);
  // Above the floors: the first thing in the list.
  expect(await page.evaluate(() => {
    const first = document.querySelector("#roomList > *");
    return first.matches(".roomitem") && first.querySelector(".rname").textContent;
  })).toBe("iye");

  // Closing the Ground floor does not take Care with it, and going into Care
  // does not open the Ground floor again.
  const ground = floorOf(page, "Ground floor");
  await ground.locator(":scope > summary").click();
  await expect(ground).not.toHaveAttribute("open", "");
  await expect(careLoca).toBeVisible();
  await page.evaluate(() => joinRoom("iye"));
  await expect(page.locator("#roomList > .roomitem .room[aria-current=page]", { hasText: "iye" })).toBeVisible();
  await expect(ground).not.toHaveAttribute("open", "");

  // Its unread marker is its own; the closed Ground floor does not count it.
  await page.evaluate(room => joinRoom(room), `${tag}-a1`);
  const { token } = await (await request.post("/smasters", { headers: MASTER, data: { name: `${tag}-sm` } })).json();
  const s = await (await request.post("/sessions", { headers: { "x-admin-token": token }, data: { name: `${tag}-sm`, kind: "user" } })).json();
  const said = await request.post("/rooms/iye/messages", {
    headers: { "x-session-token": s.session_token },
    data: { sender: `${tag}-sm`, sender_type: "user", text: "fl9 unread", op_id: `fl9-${Date.now()}` },
  });
  expect(said.ok(), await said.text()).toBeTruthy();
  await expect.poll(async () => {
    await page.evaluate(() => refreshRooms());
    return careLoca.locator(".unreadbadge").textContent({ timeout: 1000 }).catch(() => null);
  }, { timeout: 15_000 }).toBe("1");
  await expect(ground.locator(":scope > summary .unreadbadge")).toHaveCount(0);

  // Hiding it still works.
  await careLoca.locator("summary.roompreftrigger").click();
  await careLoca.locator("[data-room-preference=hide]").click();
  await expect(page.locator("#roomList > .roomitem", { hasText: "iye" })).toHaveCount(0);
  await expect(page.locator("#hiddenLocas")).toContainText("iye");
  await context.close();
});

test("a member on one floor sees that floor and the Ground floor, and nothing of another floor", async ({ browser, request }) => {
  const tag = "fl2";
  const { member, memberKey } = await building(request, tag);
  const { context, page } = await openAsMember(browser, member, memberKey);
  await expect.poll(() => floors(page)).toEqual([
    ["Ground floor", [`${tag}-g1`]],
    [`${tag}-alpha`, [`${tag}-a1`, `${tag}-a2`]],
  ]);
  const sidebar = await page.locator("#sidebar").innerHTML();
  expect(sidebar).not.toContain(`${tag}-beta`);
  expect(sidebar).not.toContain(`${tag}-b1`);
  await context.close();
});

test("with only Ground-floor locas the list has no floor heading", async ({ browser, request }) => {
  const name = "fl3-solo";
  const { token } = await (await request.post("/members", { headers: MASTER, data: { name, kind: "user" } })).json();
  await request.post("/rooms", { headers: MASTER, data: { name: "fl3-g1" } });
  await request.post("/rooms/fl3-g1/invites", { headers: MASTER, data: { name } });
  const { context, page } = await openAsMember(browser, name, token);
  await expect(page.locator("#roomList .roomitem", { hasText: "fl3-g1" })).toBeVisible();
  await expect(page.locator("#roomList details.floor")).toHaveCount(0);
  await expect(page.locator("#roomList")).not.toContainText("Ground floor");
  await context.close();
});

test("a floor closes and opens, by mouse and keyboard, and stays as left across a reload", async ({ browser, request }) => {
  const tag = "fl4";
  const { master } = await building(request, tag);
  const { context, page } = await openAsMaster(browser, master);
  const beta = floorOf(page, `${tag}-beta`);
  const b1 = page.locator("#roomList .roomitem", { hasText: `${tag}-b1` });
  await expect(b1).toBeVisible();
  await beta.locator(":scope > summary").click();
  await expect(b1).toBeHidden();
  await expect(beta).not.toHaveAttribute("open", "");
  await page.reload();
  await expect.poll(() => page.evaluate(() => state.principalId)).toBeTruthy();
  await expect(beta.locator(":scope > summary")).toBeVisible();
  await expect(beta).not.toHaveAttribute("open", "");
  await expect(b1).toBeHidden();
  // Keyboard: the heading is focusable and Enter opens it.
  await beta.locator(":scope > summary").focus();
  await page.keyboard.press("Enter");
  await expect(b1).toBeVisible();
  await expect(beta).toHaveAttribute("open", "");
  await page.reload();
  await expect.poll(() => page.evaluate(() => state.principalId)).toBeTruthy();
  await expect(page.locator("#roomList .roomitem", { hasText: `${tag}-b1` })).toBeVisible();
  await context.close();
});

test("going into a loca opens its floor", async ({ browser, request }) => {
  const tag = "fl5";
  const { master } = await building(request, tag);
  const { context, page } = await openAsMaster(browser, master);
  const alpha = floorOf(page, `${tag}-alpha`);
  await alpha.locator(":scope > summary").click();
  await expect(alpha).not.toHaveAttribute("open", "");
  // Arrive at a loca on the closed floor from elsewhere (a link, a call).
  await page.evaluate(room => joinRoom(room), `${tag}-a2`);
  await expect(alpha).toHaveAttribute("open", "");
  const a2 = page.locator("#roomList .roomitem .room", { hasText: `${tag}-a2` });
  await expect(a2).toBeVisible();
  await expect(a2).toHaveAttribute("aria-current", "page");
  // Closing the floor one is in still keeps one's own loca in sight.
  await alpha.locator(":scope > summary").click();
  await expect(alpha).not.toHaveAttribute("open", "");
  await expect(page.locator("#roomList .room[aria-current=page]", { hasText: `${tag}-a2` })).toBeVisible();
  await expect(page.locator("#roomList .roomitem", { hasText: `${tag}-a1` })).toBeHidden();
  await context.close();
});

test("move up and down reorder locas within their floor", async ({ browser, request }) => {
  const tag = "fl6";
  const { master } = await building(request, tag);
  const { context, page } = await openAsMaster(browser, master);
  const move = async (room, action) => {
    const item = page.locator("#roomList .roomitem", { hasText: room });
    await item.locator("summary.roompreftrigger").click();
    await item.locator(`[data-room-preference=${action}]`).click();
  };
  const alphaOf = async () => (await floors(page)).find(([n]) => n === `${tag}-alpha`)[1];
  await expect.poll(alphaOf).toEqual([`${tag}-a1`, `${tag}-a2`]);
  // The last of a floor has nowhere further down in it.
  await move(`${tag}-a2`, "down");
  await expect.poll(alphaOf).toEqual([`${tag}-a1`, `${tag}-a2`]);
  await move(`${tag}-a1`, "down");
  await expect.poll(alphaOf).toEqual([`${tag}-a2`, `${tag}-a1`]);
  await move(`${tag}-a1`, "up");
  await expect.poll(alphaOf).toEqual([`${tag}-a1`, `${tag}-a2`]);
  // Hiding still works inside a floor, and a floor left empty is not drawn.
  await move(`${tag}-b1`, "hide");
  await expect(floorOf(page, `${tag}-beta`)).toHaveCount(0);
  await expect(page.locator("#hiddenLocas")).toBeVisible();
  await context.close();
});

test("a closed floor still shows that a loca on it has unread messages", async ({ browser, request }) => {
  const tag = "fl7";
  const { master, member, memberKey } = await building(request, tag);
  const { context, page } = await openAsMaster(browser, master);
  const alpha = floorOf(page, `${tag}-alpha`);
  await alpha.locator(":scope > summary").click();
  await expect(alpha).not.toHaveAttribute("open", "");
  await expect(alpha.locator(":scope > summary .unreadbadge")).toHaveCount(0);
  // The member, who is on alpha, says something in a2.
  const s = await (await request.post("/sessions", { headers: { "x-admin-token": memberKey }, data: { name: member, kind: "user" } })).json();
  const said = await request.post(`/rooms/${tag}-a2/messages`, {
    headers: { "x-session-token": s.session_token },
    data: { sender: member, sender_type: "user", text: "fl7 unread", op_id: `fl7-${Date.now()}` },
  });
  expect(said.ok(), await said.text()).toBeTruthy();
  // The room list is read again (the health poll does this every few seconds).
  await expect.poll(async () => {
    await page.evaluate(() => refreshRooms());
    return alpha.locator(":scope > summary .unreadbadge").textContent({ timeout: 1000 }).catch(() => null);
  }, { timeout: 15_000 }).toBe("1");
  // Open, the floor's locas carry their own markers again.
  await alpha.locator(":scope > summary").click();
  await expect(alpha.locator(":scope > summary .unreadbadge")).toHaveCount(0);
  await expect(page.locator("#roomList .roomitem", { hasText: `${tag}-a2` }).locator(".unreadbadge")).toHaveText("1");
  await context.close();
});

// The visible text of a region and the words it offers through placeholders,
// titles, labels and tips. Names people chose (floors, locas, people) are
// taken out first: one may well be called "so-group".
async function wordsOf(page, selector) {
  return page.evaluate((selector) => {
    const chosen = [
      ...((typeof state !== "undefined" && state.groups) || []).map(g => g.name),
      ...((typeof state !== "undefined" && state.rooms) || []).map(r => r.room),
      ...((typeof state !== "undefined" && state.profilesList) || []).map(p => p.display_name),
    ].filter(Boolean).sort((a, b) => b.length - a.length);
    const words = [];
    for (const root of document.querySelectorAll(selector)) {
      words.push(root.innerText);
      for (const el of root.querySelectorAll("*")) {
        if (!el.getClientRects().length) continue;
        for (const attr of ["placeholder", "title", "aria-label", "data-tip", "data-tip-detail"]) {
          const value = el.getAttribute(attr);
          if (value) words.push(value);
        }
        if (el.tagName === "OPTION") words.push(el.textContent);
      }
    }
    let text = words.join("\n");
    for (const name of chosen) text = text.split(name).join("");
    return text;
  }, selector);
}
const GROUP_WORD = /\bgroups?\b/i;

test("no visible text calls a floor a group", async ({ browser, request }) => {
  const tag = "fl8";
  const { master } = await building(request, tag);
  // The sign-in view, signed out.
  const door = await browser.newPage();
  await door.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await door.goto("/");
  await expect(door.locator("body")).toHaveClass(/locked/);
  expect(await wordsOf(door, "body")).not.toMatch(GROUP_WORD);
  await door.close();

  const { context, page } = await openAsMaster(browser, master);
  await expect(floorOf(page, `${tag}-alpha`)).toBeVisible();
  // The sidebar with its floors, and the account menu.
  await page.locator("#whoami .profilemenu > summary").click();
  expect(await wordsOf(page, "#sidebar")).not.toMatch(GROUP_WORD);
  await page.locator("#whoami .profilemenu > summary").click();
  // This Loca on a floor's loca, with the move dialog open.
  await page.evaluate(room => joinRoom(room), `${tag}-a1`);
  await page.evaluate(() => setSidebarView("loca"));
  const summary = page.locator("#locaSummary");
  await expect(summary).toContainText(`${tag}-alpha`);
  await summary.locator("[data-loca-group-change]").click();
  await expect(summary.locator("[data-loca-group-target]")).toBeVisible();
  expect(await wordsOf(page, "#sideLocaView")).not.toMatch(GROUP_WORD);
  await summary.locator("[data-loca-group-cancel]").click();
  await page.evaluate(() => setSidebarView("building"));
  // The Building view: People with its floor filter, and the Floors tab.
  await page.evaluate(() => switchTab("people"));
  await expect(page.locator("#btabGroups")).toBeVisible();
  await expect(page.locator("#peopleList")).toContainText(`${tag}-member`);
  expect(await wordsOf(page, "#peoplePanel")).not.toMatch(GROUP_WORD);
  await page.locator("#btabGroups").click();
  await expect(page.locator("#groupsPane")).toContainText(`${tag}-alpha`);
  expect(await wordsOf(page, "#peoplePanel")).not.toMatch(GROUP_WORD);
  // A person's card, and the new-loca dialog.
  await page.locator("#groupsPane [data-person-card]", { hasText: `${tag}-member` }).first().click();
  await expect(page.locator("#personCard")).toBeVisible();
  expect(await wordsOf(page, "#personCard")).not.toMatch(GROUP_WORD);
  await page.locator("#personCardClose").click();
  await page.locator("#newRoomName").fill(`${tag}-new`);
  await page.locator("#newRoomBtn").click();
  await expect(page.locator("#extCreateGroupField")).toBeVisible();
  expect(await wordsOf(page, "#extCreate")).not.toMatch(GROUP_WORD);
  await context.close();
});
