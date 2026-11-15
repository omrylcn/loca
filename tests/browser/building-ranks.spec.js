const { test, expect } = require("@playwright/test");

const MASTER = { "x-admin-token": "MASTER" };

async function adminSession(request, key, name) {
  if (key === "MASTER") {
    const paired = await request.post("/pairings?ttl_hours=1", { headers: MASTER });
    const { pairing_code } = await paired.json();
    const signedIn = await request.post("/sessions", {
      headers: { "x-pairing-code": pairing_code }, data: { name, kind: "user" },
    });
    expect(signedIn.ok()).toBe(true);
    return signedIn.json();
  }
  const signedIn = await request.post("/sessions", {
    headers: { "x-admin-token": key }, data: { name, kind: "user" },
  });
  expect(signedIn.ok()).toBe(true);
  return signedIn.json();
}

async function openAs(page, info, name) {
  await page.addInitScript(({ token, expiresAt, name }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name }));
  }, { token: info.session_token, expiresAt: info.expires_at, name });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
}

test("Building lists the Master and every Smaster by name, with the rank beside it", async ({ page, request }) => {
  const created = await request.post("/smasters", { headers: MASTER, data: { name: "ui-rank-listed" } });
  expect(created.ok()).toBe(true);
  // A title is not a name.
  const refused = await request.post("/smasters", { headers: MASTER, data: { name: "Smaster" } });
  expect(refused.status()).toBe(400);

  const info = await adminSession(request, "MASTER", "operator");
  await openAs(page, info, "operator");
  await page.evaluate(() => switchTab("people"));
  await expect.poll(() => page.evaluate(() => state.profile?.building_role)).toBe("master");
  const rows = page.locator("#peopleList .prow.rankrow");
  await expect(rows.filter({ hasText: "ui-rank-listed" })).toHaveCount(1);
  await expect(rows.filter({ hasText: "ui-rank-listed" }).locator(".rankbadge")).toHaveText("Smaster");
  await expect(rows.locator(".rankbadge.rank-master")).toHaveCount(1);
  // Nobody signed in with it yet: the Building calls it an unused key, not a person.
  await expect(rows.filter({ hasText: "ui-rank-listed" })).toContainText("unused code");
});

test("a Smaster renames its own profile; titles and taken names are refused", async ({ page, request }) => {
  const created = await request.post("/smasters", { headers: MASTER, data: { name: "ui-rank-before" } });
  const { token } = await created.json();
  const info = await adminSession(request, token, "ignored-body-name");
  await openAs(page, info, "ui-rank-before");
  await page.evaluate(() => switchTab("people"));
  await expect.poll(() => page.evaluate(() => state.profile?.building_role)).toBe("smaster");

  await page.locator("#whoami summary").first().click();
  const input = page.locator("#profileNameInput");
  await expect(input).toHaveValue("ui-rank-before");
  await expect(page.locator("#whoami .profileconnected .rankbadge")).toHaveText("Smaster");

  await input.fill("master");
  await page.locator("#profileRename").click();
  await expect(page.locator("#profileRenameNote")).toContainText("title");

  await input.fill("ui-rank-after");
  await page.locator("#profileRename").click();
  await expect(page.locator("#profileRenameNote")).toHaveText("Renamed to ui-rank-after.");
  await expect.poll(() => page.evaluate(() => state.name)).toBe("ui-rank-after");
  await expect(page.locator("#whoami .profileconnected b").first()).toHaveText("ui-rank-after");

  // Same key, same rank, new name — the Building now lists the new name.
  const ranks = await (await request.get("/building/ranks", { headers: MASTER })).json();
  expect(ranks.some(r => r.name === "ui-rank-after" && r.role === "smaster")).toBe(true);
  expect(ranks.some(r => r.name === "ui-rank-before")).toBe(false);
});

test("loca roster shows rank badges and offers no moderation the server would refuse", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await page.goto("/");
  await page.evaluate(() => {
    state.room = "rank-room";
    state.name = "deputy";
    state.adminSession = true;
    state.profile = { building_role: "smaster" };
    state.members = [
      { name: "omrylcn", type: "user", role: "master" },
      { name: "deputy", type: "user", role: "smaster" },
      { name: "selin", type: "user", role: "smaster" },
      { name: "worker", type: "agent" },
    ];
    renderMembers();
  });
  const row = name => page.locator("#onlineList .omem").filter({ hasText: name });
  await expect(row("omrylcn").locator(".rankbadge")).toHaveText("Master");
  await expect(row("selin").locator(".rankbadge")).toHaveText("Smaster");
  await expect(row("worker").locator(".rankbadge")).toHaveCount(0);
  // A Smaster sees no mute/kick/ban on the Master or a peer Smaster, but does on a member.
  await expect(row("omrylcn").locator("[data-mod]")).toHaveCount(0);
  await expect(row("selin").locator("[data-mod]")).toHaveCount(0);
  await expect(row("worker").locator('[data-mod="kick"]')).toHaveCount(1);
  // The Master may moderate a Smaster.
  await page.evaluate(() => { state.profile.building_role = "master"; state.name = "omrylcn"; renderMembers(); });
  await expect(row("selin").locator('[data-mod="kick"]')).toHaveCount(1);
  await expect(row("omrylcn").locator("[data-mod]")).toHaveCount(0);
});

test("Building header counts what the directory shows, and a phone never scrolls sideways", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await page.setViewportSize({ width: 390, height: 800 });
  await page.goto("/");
  // Open the Building view without a live server fetch, then render fixtures.
  await page.evaluate(() => { state.adminSession = false; switchTab("people"); });
  await page.evaluate(() => {
    state.adminSession = true;
    state.profile = { building_role: "master" };
    state.ranks = [{ name: "omrylcn", role: "master", locas: ["fa-backend"] }, { name: "ayse", role: "smaster", locas: [] }];
    state.people = [
      { name: "claude-api", kind: "agent", online: true, locas: ["fa-backend"], membership_id: "mbid_a", runtime: { ready: true } },
      { name: "a-very-long-agent-name-that-should-not-push-the-page", kind: "agent", online: false, locas: [], membership_id: "mbid_b" },
      { name: "selin", kind: "user", online: true, locas: [], membership_id: "mbid_c" },
    ];
    state.joinRequests = [];
    renderPeople();
  });
  await expect(page.locator("#subStatus")).toHaveText("2 ranks · 3 members · 2 online · 1 in lobby");
  await expect(page.locator("#peopleList .locachip").first()).toHaveText("#fa-backend");
  await expect(page.locator("[data-revoke-member]").first()).toHaveText("Revoke");
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
  expect(overflow).toBeLessThanOrEqual(0);
});

test("a rename carries the rank's past messages, live in every open feed and after reload", async ({ page, browser, request }) => {
  const { provisionRooms } = require("./support/building");
  await provisionRooms(request, ["rename-room"]);
  const { token } = await (await request.post("/smasters", { headers: MASTER, data: { name: "ui-said-before" } })).json();
  const info = await adminSession(request, token, "x");
  await page.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    if (!sessionStorage.getItem("init")) {
      sessionStorage.setItem("init", "1");
      localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "ui-said-before" }));
      localStorage.setItem("loca-seated-rooms", JSON.stringify(["rename-room"]));
    }
  }, { token: info.session_token, expiresAt: info.expires_at });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate(() => joinRoom("rename-room"));
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  await page.fill("#msg", "said under the old name");
  await page.keyboard.press("Enter");
  await expect(page.locator("#feed")).toContainText("said under the old name");

  // Someone else watches the same loca in another browser.
  const other = await browser.newPage();
  const watcher = await adminSession(request, "MASTER", "w");
  await other.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "Master" }));
  }, { token: watcher.session_token, expiresAt: watcher.expires_at });
  await other.goto("/");
  await expect.poll(() => other.evaluate(() => state.authStatus)).toBe("authenticated");
  await other.evaluate(() => joinRoom("rename-room"));
  const otherSender = other.locator("#feed .row", { hasText: "said under the old name" }).locator(".sender");
  await expect(otherSender).toHaveText("ui-said-before");

  await page.locator("#whoami summary").first().click();
  await page.locator("#profileNameInput").fill("ui-said-after");
  await page.locator("#profileRename").click();
  await expect(page.locator("#profileRenameNote")).toHaveText("Renamed to ui-said-after.");

  // The other feed relabels without a reload; ours too, and the name sticks.
  await expect(otherSender).toHaveText("ui-said-after");
  const mine = page.locator("#feed .row", { hasText: "said under the old name" }).locator(".sender");
  await expect(mine).toHaveText("ui-said-after");
  expect(await page.evaluate(() => JSON.parse(localStorage.getItem("loca-admin-session")).name)).toBe("ui-said-after");
  await expect(page.locator("#name")).toHaveValue("ui-said-after");
  await page.reload();
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate(() => joinRoom("rename-room"));
  await expect(mine).toHaveText("ui-said-after");
  await other.close();
});

test("a name typed at Smaster sign-in becomes the display name; a refused one is explained", async ({ page, request }) => {
  const { token } = await (await request.post("/smasters", { headers: MASTER, data: { name: "ui-placeholder-sm" } })).json();
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await page.goto("/");
  await page.locator("#entryKind").selectOption("smaster");
  await expect(page.locator("#name")).toHaveAttribute("placeholder", /display name/);
  await page.locator("#name").fill("ui-chosen-name");
  await page.locator("#roomToken").fill(token);
  await page.locator("#connectBtn").click();
  await expect.poll(() => page.evaluate(() => state.profile?.principal?.display_name)).toBe("ui-chosen-name");
  await expect.poll(() => page.evaluate(() => state.name)).toBe("ui-chosen-name");
  const ranks = await (await request.get("/building/ranks", { headers: MASTER })).json();
  expect(ranks.some(r => r.name === "ui-chosen-name" && r.role === "smaster")).toBe(true);
  expect(ranks.some(r => r.name === "ui-placeholder-sm")).toBe(false);

  // A title is refused: the key still signs in, and the profile says why.
  const second = await (await request.post("/smasters", { headers: MASTER, data: { name: "ui-placeholder-two" } })).json();
  const fresh = await page.context().browser().newPage();
  await fresh.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await fresh.goto("/");
  await fresh.locator("#entryKind").selectOption("smaster");
  await fresh.locator("#name").fill("ui-chosen-name");
  await fresh.locator("#roomToken").fill(second.token);
  await fresh.locator("#connectBtn").click();
  await expect(fresh.locator("#profileRenameNote")).toContainText('"ui-chosen-name" was not set');
  await expect.poll(() => fresh.evaluate(() => state.name)).toBe("ui-placeholder-two");
  await fresh.close();
});

test("the operator wears its title on every line it writes, like the lead", async ({ page, request }) => {
  const { provisionRooms } = require("./support/building");
  await provisionRooms(request, ["optag-room"]);
  const info = await adminSession(request, "MASTER", "operator");
  await page.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "Master" }));
    localStorage.setItem("loca-seated-rooms", JSON.stringify(["optag-room"]));
  }, { token: info.session_token, expiresAt: info.expires_at });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate(() => joinRoom("optag-room"));
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  await page.fill("#msg", "operator line");
  await page.keyboard.press("Enter");
  const row = page.locator("#feed .row", { hasText: "operator line" });
  await expect(row.locator(".optag")).toHaveText("operator");
  await page.evaluate(() => setSidebarView("loca"));
  // This loca was founded by the Master, so the role is the appointed one
  // (the by-rank case is covered in tests/ws/rooms.rs).
  await expect(page.locator("#locaSummary")).toContainText("OPERATOR");
  await expect(page.locator("#locaSummary")).not.toContainText("PARTICIPANT");
});

// Record every frame the page handles, in order, before the page acts on it.
async function recordFrames(page) {
  await page.evaluate(() => {
    window.__frames = [];
    const handle = window.onFrame;
    window.onFrame = (f) => { window.__frames.push(JSON.parse(JSON.stringify(f))); return handle(f); };
  });
}

// The Web UI's handling of `renamed` before frames named their messages,
// verbatim from bc8ddcd web/assets/socket.js:207-212 (`state.msgs` passed in).
// An old tab still runs this until it is reloaded.
function previousOnRenamed(msgs, from, to) {
  if (!from || !to || from === to) return;
  for (const m of msgs) {
    if (m.sender_type === "user" && m.sender === from) m.sender = to;
    if (m.reply_to_sender === from) m.reply_to_sender = to;
  }
}

test("a rename relabels only the renamer's messages by id; the by-name frame relabels nothing", async ({ page, request }) => {
  const { provisionRooms } = require("./support/building");
  const room = "rename-own-room";
  await provisionRooms(request, [room]);
  const json = async (response) => { expect(response.ok()).toBe(true); return response.json(); };
  const memberSession = async (name, kind) => {
    const { token } = await json(await request.post("/members", { headers: MASTER, data: { name, kind } }));
    // Management URLs take the member's id (`mbid_`), never its key.
    const { member_id: id } = await json(await request.post(`/rooms/${room}/invites`, { headers: MASTER, data: { name } }));
    const { session_token } = await json(await request.post("/sessions", {
      headers: { "x-room-token": token }, data: { name, kind },
    }));
    return { token, id, session: session_token };
  };
  const say = async (session, text, replyTo) => (await json(await request.post(`/rooms/${room}/messages`, {
    headers: { "x-session-token": session },
    data: { sender: "ignored", sender_type: "user", text, reply_to: replyTo ?? null },
  }))).id;

  // A member wears "ui-own-shared" first, speaks, and leaves the Building.
  const member = await memberSession("ui-own-shared", "user");
  const memberWords = await say(member.session, "the member's words");
  expect((await request.delete(`/members/${member.id}`, { headers: MASTER })).ok()).toBe(true);

  // A Smaster takes the freed name and speaks under it; someone answers both.
  const { token: code } = await json(await request.post("/smasters", { headers: MASTER, data: { name: "ui-own-before" } }));
  const deputy = await adminSession(request, code, "x");
  await json(await request.put("/profile", {
    headers: { "x-session-token": deputy.session_token }, data: { display_name: "ui-own-shared" },
  }));
  const deputyWords = await say(deputy.session_token, "the smaster's words");
  const helper = await memberSession("ui-own-helper", "agent");
  const toMember = await say(helper.session, "answering the member", memberWords);
  const toDeputy = await say(helper.session, "answering the smaster", deputyWords);

  // The Master watches the loca.
  await openAs(page, await adminSession(request, "MASTER", "watcher"), "watcher");
  await page.evaluate((r) => joinRoom(r), room);
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  const senderOf = (id) => page.locator(`#feed .row[data-id="${id}"] .sender`).first();
  await expect(senderOf(memberWords)).toHaveText("ui-own-shared");
  await expect(senderOf(deputyWords)).toHaveText("ui-own-shared");
  const label = (id) => page.evaluate((id) => state.msgs.find(m => m.id === id)?.reply_to_sender ?? null, id);

  // The by-name frame is not evidence of who wrote what.
  await page.evaluate(() => onFrame({ t: "renamed", from: "ui-own-shared", to: "ui-own-hijack" }));
  await expect(senderOf(memberWords)).toHaveText("ui-own-shared");
  await expect(senderOf(deputyWords)).toHaveText("ui-own-shared");
  expect(await label(toMember)).toBe("ui-own-shared");
  expect(await label(toDeputy)).toBe("ui-own-shared");

  // The real rename: only the Smaster's words and the reply to them change.
  // The id frame arrives before the by-name one.
  await recordFrames(page);
  await json(await request.put("/profile", {
    headers: { "x-session-token": deputy.session_token }, data: { display_name: "ui-own-after" },
  }));
  await expect(senderOf(deputyWords)).toHaveText("ui-own-after");
  await expect.poll(() => page.evaluate(() => window.__frames
    .filter(f => f.t === "relabelled" || f.t === "renamed").map(f => f.t))).toEqual(["relabelled", "renamed"]);
  await expect(senderOf(memberWords)).toHaveText("ui-own-shared");
  expect(await label(toDeputy)).toBe("ui-own-after");
  expect(await label(toMember)).toBe("ui-own-shared");

  // A reload reads the same labels from the server.
  await page.reload();
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate((r) => joinRoom(r), room);
  await expect(senderOf(deputyWords)).toHaveText("ui-own-after");
  await expect(senderOf(memberWords)).toHaveText("ui-own-shared");
});

test("when both wearers' words have left the tail, only the renamer's reply label changes, in a new tab and an old one", async ({ page, request }) => {
  const { provisionRooms } = require("./support/building");
  const room = "rename-tail-room";
  await provisionRooms(request, [room]);
  const json = async (response) => { expect(response.ok()).toBe(true); return response.json(); };
  const say = async (session, text, replyTo) => (await json(await request.post(`/rooms/${room}/messages`, {
    headers: { "x-session-token": session },
    data: { sender: "ignored", sender_type: "user", text, reply_to: replyTo ?? null },
  }))).id;
  const memberSession = async (name, kind) => {
    const { token } = await json(await request.post("/members", { headers: MASTER, data: { name, kind } }));
    // Management URLs take the member's id (`mbid_`), never its key.
    const { member_id: id } = await json(await request.post(`/rooms/${room}/invites`, { headers: MASTER, data: { name } }));
    const { session_token } = await json(await request.post("/sessions", {
      headers: { "x-room-token": token }, data: { name, kind },
    }));
    return { token, id, session: session_token };
  };

  // Two people wear "ui-tail-shared" one after the other.
  const member = await memberSession("ui-tail-shared", "user");
  const memberWords = await say(member.session, "the member, long ago");
  expect((await request.delete(`/members/${member.id}`, { headers: MASTER })).ok()).toBe(true);
  const { token: code } = await json(await request.post("/smasters", { headers: MASTER, data: { name: "ui-tail-before" } }));
  const deputy = await adminSession(request, code, "x");
  await json(await request.put("/profile", {
    headers: { "x-session-token": deputy.session_token }, data: { display_name: "ui-tail-shared" },
  }));
  const deputyWords = await say(deputy.session_token, "the smaster, long ago");

  // Both lines leave the in-memory tail; answers to each are in it.
  const watcher = await adminSession(request, "MASTER", "watcher");
  for (let n = 0; n < 200; n++) await say(watcher.session_token, `filler ${n}`);
  const helper = await memberSession("ui-tail-helper", "agent");
  const toMember = await say(helper.session, "answering the member", memberWords);
  const toDeputy = await say(helper.session, "answering the smaster", deputyWords);

  await openAs(page, watcher, "watcher");
  await page.evaluate((r) => joinRoom(r), room);
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  const label = (id) => page.evaluate((id) => state.msgs.find(m => m.id === id)?.reply_to_sender ?? null, id);
  await expect.poll(() => label(toMember)).toBe("ui-tail-shared");
  expect(await label(toDeputy)).toBe("ui-tail-shared");
  const before = await page.evaluate(() => JSON.parse(JSON.stringify(state.msgs)));
  expect(before.some(m => m.id === memberWords || m.id === deputyWords)).toBe(false);

  await recordFrames(page);
  await json(await request.put("/profile", {
    headers: { "x-session-token": deputy.session_token }, data: { display_name: "ui-tail-after" },
  }));
  // Everything the rename sent has arrived once a later message has.
  await say(watcher.session_token, "after the rename");
  await expect.poll(() => page.evaluate(() => state.msgs.some(m => m.text === "after the rename"))).toBe(true);
  const frames = await page.evaluate(() => window.__frames);

  // (a) This page relabels exactly the reply that answers the renamer.
  expect(await label(toDeputy)).toBe("ui-tail-after");
  expect(await label(toMember)).toBe("ui-tail-shared");
  expect(frames.filter(f => f.t === "relabelled")).toEqual([
    { t: "relabelled", to: "ui-tail-after", ids: [], reply_ids: [toDeputy] },
  ]);

  // (b) A tab still running the previous Web UI applies `renamed` by name.
  // The server sends it none here, exactly as before, so that tab changes
  // nothing: no one else's reply label is touched.
  expect(frames.filter(f => f.t === "renamed")).toEqual([]);
  const old = JSON.parse(JSON.stringify(before));
  for (const f of frames) if (f.t === "renamed") previousOnRenamed(old, f.from, f.to);
  expect(old.find(m => m.id === toMember).reply_to_sender).toBe("ui-tail-shared");
  expect(old.find(m => m.id === toDeputy).reply_to_sender).toBe("ui-tail-shared");
});
