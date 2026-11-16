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
  // Care has no floor of its own, so it is on the Ground floor.
  expect((await floors(page)).find(([n]) => n === "Ground floor")[1]).toContain("iye");
  await context.close();
});

test("Care is on the Ground floor like every loca without a floor, first there, with a care tag", async ({ browser, request }) => {
  const tag = "fl9";
  const { master } = await building(request, tag);
  const { context, page } = await openAsMaster(browser, master);
  await expect(floorOf(page, `${tag}-alpha`)).toBeVisible();
  const ground = floorOf(page, "Ground floor");
  // Under the Ground floor heading, first, and nowhere else.
  await expect(page.locator("#roomList > .roomitem", { hasText: "iye" })).toHaveCount(0);
  await expect.poll(async () => (await floors(page)).find(([n]) => n === "Ground floor")[1][0]).toBe("iye");
  const careLoca = ground.locator(".roomitem", { has: page.locator(".rname", { hasText: /^iye$/ }) });
  await expect(careLoca.locator(".caretag")).toHaveText("care");
  // No other loca carries the tag.
  await expect(page.locator("#roomList .caretag")).toHaveCount(1);

  // It closes with its floor, and going into it opens the floor again.
  await ground.locator(":scope > summary").click();
  await expect(ground).not.toHaveAttribute("open", "");
  await page.evaluate(() => joinRoom("iye"));
  await expect(ground).toHaveAttribute("open", "");
  await expect(careLoca.locator(".room[aria-current=page]")).toBeVisible();
  await context.close();
});

test("a named floor's heading says floor; the Ground floor's and a name ending in floor are not doubled", async ({ browser, request }) => {
  const tag = "fl10";
  const { master } = await building(request, tag);
  const as = { "x-session-token": master.session_token };
  const made = await request.post("/groups", { headers: as, data: { name: `${tag}-Top Floor` } });
  const top = (await made.json()).id;
  expect((await request.post("/rooms", { headers: as, data: { name: `${tag}-t1`, group: top } })).status()).toBe(201);
  const { context, page } = await openAsMaster(browser, master);
  const heading = name => floorOf(page, name).locator(":scope > summary");
  // The word is drawn after the name; the name itself stays the group's.
  const shown = name => heading(name).locator(".rgroupname").evaluate(el => el.textContent + getComputedStyle(el, "::after").content.replace(/^none$/, "").replaceAll('"', ""));
  await expect(heading(`${tag}-alpha`).locator(".rgroupname")).toHaveText(`${tag}-alpha`);
  expect(await shown(`${tag}-alpha`)).toBe(`${tag}-alpha floor`);
  expect(await shown("Ground floor")).toBe("Ground floor");
  expect(await shown(`${tag}-Top Floor`)).toBe(`${tag}-Top Floor`);
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

test("with only Ground-floor locas the list is headed Ground floor, and there is no Your Locas label", async ({ browser, request }) => {
  const name = "fl3-solo";
  const { token } = await (await request.post("/members", { headers: MASTER, data: { name, kind: "user" } })).json();
  await request.post("/rooms", { headers: MASTER, data: { name: "fl3-g1" } });
  await request.post("/rooms/fl3-g1/invites", { headers: MASTER, data: { name } });
  const { context, page } = await openAsMember(browser, name, token);
  await expect(page.locator("#roomList .roomitem", { hasText: "fl3-g1" })).toBeVisible();
  await expect(page.locator("#roomList details.floor")).toHaveCount(1);
  await expect(floorOf(page, "Ground floor").locator(".roomitem", { hasText: "fl3-g1" })).toBeVisible();
  await expect(page.locator("#sideBuildingView .rlabel", { hasText: /^YOUR LOCAS$/i })).toHaveCount(0);
  await context.close();
});

test("the only floor in sight closes too: its heading folds every loca away and opens them again", async ({ browser, request }) => {
  const tag = "fl3c";
  const m = await masterSession(request);
  const as = { "x-session-token": m.session_token };
  for (const name of [`${tag}-one`, `${tag}-two`]) {
    expect((await request.post("/rooms", { headers: as, data: { name } })).status()).toBe(201);
  }
  const member = `${tag}-member`;
  const { token } = await (await request.post("/members", { headers: MASTER, data: { name: member, kind: "user" } })).json();
  for (const name of [`${tag}-one`, `${tag}-two`]) {
    expect((await request.post(`/rooms/${name}/invites`, { headers: MASTER, data: { name: member } })).ok()).toBeTruthy();
  }
  const { context, page } = await openAsMember(browser, member, token);
  const only = page.locator("#roomList details.floor");
  await expect(only).toHaveCount(1);
  await page.evaluate(room => joinRoom(room), `${tag}-one`);
  const rows = page.locator("#roomList .roomitem");
  await expect(rows.filter({ hasText: `${tag}-one` })).toBeVisible();
  await only.locator(":scope > summary").click();
  await expect(only).not.toHaveAttribute("open", "");
  // Closed is closed: not even the loca the viewer is in stays listed.
  for (const name of [`${tag}-one`, `${tag}-two`]) await expect(rows.filter({ hasText: name })).toBeHidden();
  await expect(page.locator("#roomList > .roomitem")).toHaveCount(0);
  await only.locator(":scope > summary").click();
  await expect(rows.filter({ hasText: `${tag}-two` })).toBeVisible();
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

test("a floor drawn before the viewer's preferences were known does not undo a stored close", async ({ browser, request }) => {
  const tag = "fl4b";
  const { master } = await building(request, tag);
  const { context, page } = await openAsMaster(browser, master);
  const beta = floorOf(page, `${tag}-beta`);
  await beta.locator(":scope > summary").click();
  await expect(beta).not.toHaveAttribute("open", "");
  const key = await page.evaluate(() => Object.keys(localStorage).find(k => k.startsWith("loca-room-preferences:")));
  const closed = () => page.evaluate(k => JSON.parse(localStorage.getItem(k)).collapsed.length, key);
  expect(await closed()).toBe(1);
  // What happens at a page start: the list is drawn while the identity is
  // unknown (every floor open), then the identity arrives. The headings drawn
  // open fire their `toggle` only afterwards.
  await page.evaluate(async () => {
    const principal = state.principalId;
    state.principalId = null;
    renderRooms();
    state.principalId = principal;
    await new Promise(resolve => setTimeout(resolve, 50));
    renderRooms();
    await new Promise(resolve => setTimeout(resolve, 50));
  });
  expect(await closed()).toBe(1);
  await expect(beta).not.toHaveAttribute("open", "");
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
  await context.close();
});

test("a closed floor shows no loca at all, not even the one the viewer is in", async ({ browser, request }) => {
  const tag = "fl5b";
  const { master } = await building(request, tag);
  const { context, page } = await openAsMaster(browser, master);
  const alpha = floorOf(page, `${tag}-alpha`);
  await page.evaluate(room => joinRoom(room), `${tag}-a2`);
  await expect(page.locator("#roomList .room[aria-current=page]", { hasText: `${tag}-a2` })).toBeVisible();
  await alpha.locator(":scope > summary").click();
  await expect(alpha).not.toHaveAttribute("open", "");
  for (const name of [`${tag}-a1`, `${tag}-a2`]) {
    await expect(page.locator("#roomList .roomitem", { hasText: name })).toBeHidden();
  }
  // Nothing of the floor is drawn outside it, and the heading still counts it.
  await expect(page.locator("#roomList > .roomitem")).toHaveCount(0);
  await expect(alpha.locator(":scope > summary .rgroupcount")).toHaveText("2");
  await alpha.locator(":scope > summary").click();
  await expect(page.locator("#roomList .room[aria-current=page]", { hasText: `${tag}-a2` })).toBeVisible();
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
