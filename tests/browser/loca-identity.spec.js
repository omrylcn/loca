const { test, expect } = require("@playwright/test");

// A loca's identity (PRINCIPLES "A loca's identity"): the create panel sets
// it, the same panel edits it from This Loca (read-only for a participant),
// the sidebar entry and the loca header wear it, and a coloured icon reads
// in both themes.

const MASTER = { "x-admin-token": "MASTER" };

async function masterPage(browser, request, viewport) {
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const info = await (await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" },
  })).json();
  const page = await browser.newPage(viewport ? { viewport } : {});
  await page.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "Master" }));
  }, { token: info.session_token, expiresAt: info.expires_at });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  return page;
}

async function serverSettings(request, room) {
  return (await request.get(`/rooms/${room}/settings`, { headers: MASTER })).json();
}

function roomRow(page, room) {
  return page.locator("#roomList .room", { has: page.locator(".rname", { hasText: new RegExp(`^${room}$`) }) });
}

test("Enter straight through opens the same loca as before; the panel sets description, seats, icon, colour and labels", async ({ browser, request }) => {
  const page = await masterPage(browser, request);
  const posted = [];
  page.on("request", req => { if (req.method() === "POST" && new URL(req.url()).pathname === "/rooms") posted.push(req.postDataJSON()); });

  // Name, +, Enter: nothing but the name and the default extensions is sent.
  await page.locator("#newRoomName").fill("id-plain");
  await page.locator("#newRoomBtn").click();
  const dialog = page.locator("#extCreate");
  await expect(dialog).toBeVisible();
  await expect(page.locator("#extCreateTitle")).toHaveText("Open a new loca");
  await expect(page.locator("#locaDesc")).toHaveValue("");
  // The Building's default, and its ceiling as the most a loca may choose.
  await expect(page.locator("#locaSeats")).toHaveValue("7");
  await expect(page.locator("#locaSeats")).toHaveAttribute("max", "32");
  await expect(page.locator("#extCreateDefault input")).toHaveCount(4);
  await page.keyboard.press("Enter");
  await expect(dialog).toBeHidden();
  await expect.poll(() => page.evaluate(() => state.room)).toBe("id-plain");
  expect(posted).toEqual([{ name: "id-plain", extensions: ["notes", "journal", "wiki"] }]);
  const plain = await serverSettings(request, "id-plain");
  for (const key of ["description", "seats", "icon", "color", "labels"]) expect(plain).not.toHaveProperty(key);
  await expect(roomRow(page, "id-plain").locator(".locamark")).toHaveCount(0);
  await expect(page.locator("#roomMark")).toBeHidden();

  // The whole panel, by keyboard where it matters.
  await page.locator("#newRoomName").fill("id-new");
  await page.locator("#newRoomBtn").click();
  await page.locator("#locaDesc").fill("Ship release 14\nwithout losing a word.");
  await expect(page.locator("#locaDesc")).toHaveValue("Ship release 14 without losing a word.");
  await expect(page.locator("#locaDescCount")).toHaveText("38 / 280");
  await page.locator("#locaSeats").fill("12");
  await page.locator("#locaLabels").fill("backend, q4, Backend");
  // The icon list is a radio group: arrows move through it.
  await page.locator("#locaIconOpts input[value='']").focus();
  await page.keyboard.press("ArrowRight");
  await expect(page.locator("#locaIconOpts input[value=book]")).toBeChecked();
  await page.locator("#locaIconOpts .identopt[title=rocket]").click();
  await page.locator("#locaColorOpts .identopt[title=Sky]").click();
  await page.locator("#extCreateSubmit").click();
  await expect(dialog).toBeHidden();
  await expect.poll(() => page.evaluate(() => state.room)).toBe("id-new");
  expect(posted[1]).toEqual({
    name: "id-new", extensions: ["notes", "journal", "wiki"],
    description: "Ship release 14 without losing a word.", seats: 12, icon: "rocket", color: "sky",
    labels: ["backend", "q4", "Backend"],
  });
  const created = await serverSettings(request, "id-new");
  expect([created.description, created.seats, created.icon, created.color, created.labels])
    .toEqual(["Ship release 14 without losing a word.", 12, "rocket", "sky", ["backend", "q4"]]);

  // The sidebar entry wears the mark; its tooltip says what it is for.
  const row = roomRow(page, "id-new");
  await expect(row.locator(".locamark svg")).toHaveCount(1);
  await expect(row.locator(".locamark")).toHaveAttribute("data-loca-color", "sky");
  await expect(row).toHaveAttribute("title", "id-new\nShip release 14 without losing a word.\nLabels: backend · q4");
  // The header too: the mark before the name, the description beside it.
  await expect(page.locator("#roomMark svg")).toHaveCount(1);
  await expect(page.locator("#roomMark")).toHaveAttribute("data-loca-color", "sky");
  await expect(page.locator("#roomDesc")).toHaveText("Ship release 14 without losing a word.");
  // This Loca shows the identity; labels are not badges.
  await page.locator("#sideLocaTab").click();
  const block = page.locator("#locaSummary .locaident");
  await expect(block.locator(".locadesc")).toHaveText("Ship release 14 without losing a word.");
  await expect(block.locator(".localabel")).toHaveText(["backend", "q4"]);
  await expect(block.locator(".locaseats")).toHaveText("12 seats");
  await expect(page.locator("#locaSummary .rankbadge, #locaSummary .optag, #locaSummary .leadtag")).toHaveCount(0);
  const label = await block.locator(".localabel").first().evaluate(el => {
    const css = getComputedStyle(el);
    return [css.borderTopStyle, css.textTransform, css.fontWeight, css.borderTopLeftRadius];
  });
  expect(label).toEqual(["dashed", "none", "400", "2px"]);
  const badge = await page.evaluate(() => {
    const probe = document.createElement("span");
    probe.className = "rankbadge";
    document.body.append(probe);
    const css = getComputedStyle(probe);
    const out = [css.borderTopStyle, css.textTransform];
    probe.remove();
    return out;
  });
  expect(badge).toEqual(["solid", "uppercase"]);
  await page.close();
});

test("the same panel edits from This Loca; a refused save keeps the draft and shows the server's sentence", async ({ browser, request }) => {
  const room = "id-edit";
  const made = await request.post("/rooms", { headers: MASTER, data: { name: room, description: "Before." } });
  expect([201, 409]).toContain(made.status());
  const page = await masterPage(browser, request);
  await page.evaluate((room) => joinRoom(room), room);
  await expect.poll(() => page.evaluate(() => state.ws?.readyState)).toBe(1);
  await page.locator("#sideLocaTab").click();
  const open = page.locator("#locaSummary #identityOpen");
  await expect(open).toHaveText("Edit");
  await open.click();
  const dialog = page.locator("#extCreate");
  await expect(dialog).toBeVisible();
  await expect(page.locator("#extCreateTitle")).toHaveText("Loca identity");
  await expect(page.locator("#extCreateName")).toBeDisabled();
  await expect(page.locator("#extCreateName")).toHaveValue(room);
  await expect(page.locator("#extCreateExtensions")).toBeHidden();
  await expect(page.locator("#extCreateSubmit")).toHaveText("Save");
  await expect(page.locator("#locaDesc")).toHaveValue("Before.");
  await expect(page.locator("#locaDesc")).toBeFocused();

  await page.locator("#locaDesc").fill("After.");
  await page.locator("#locaLabels").fill("Master, ops");
  await page.locator("#extCreateSubmit").click();
  await expect(page.locator("#extCreateError")).toContainText("grants nothing");
  await expect(dialog).toBeVisible();
  await expect(page.locator("#locaDesc")).toHaveValue("After.");
  await expect(page.locator("#locaLabels")).toHaveValue("Master, ops");
  expect((await serverSettings(request, room)).description).toBe("Before.");

  await page.locator("#locaLabels").fill("ops");
  await page.locator("#locaColorOpts .identopt[title=Green]").click();
  await page.locator("#locaDesc").press("Enter");
  await expect(dialog).toBeHidden();
  const saved = await serverSettings(request, room);
  expect([saved.description, saved.labels, saved.color, saved.seats]).toEqual(["After.", ["ops"], "green", undefined]);
  await expect(page.locator("#locaSummary .locadesc")).toHaveText("After.");
  await expect(roomRow(page, room).locator(".locamark")).toHaveAttribute("data-loca-color", "green");
  const journal = await (await request.get(`/rooms/${room}/journal`, { headers: MASTER })).json();
  expect(journal.map(entry => entry.text)).toContain("identity: description changed; colour green; labels ops");

  // A new loca after an edit starts from the create panel again.
  await page.locator("#sideBuildingTab").click();
  await page.locator("#newRoomBtn").click();
  await expect(page.locator("#extCreateTitle")).toHaveText("Open a new loca");
  await expect(page.locator("#extCreateName")).toBeEnabled();
  await expect(page.locator("#locaDesc")).toHaveValue("");
  await expect(page.locator("#extCreateExtensions")).toBeVisible();
  await page.keyboard.press("Escape");
  await page.close();
});

test("a participant reads the identity and cannot change it", async ({ browser, request }) => {
  const room = "id-read";
  const made = await request.post("/rooms", {
    headers: MASTER, data: { name: room, description: "For reading.", icon: "book", labels: ["docs"] },
  });
  expect([201, 409]).toContain(made.status());
  const name = "id-reader";
  expect((await request.post("/members", { headers: MASTER, data: { name, kind: "user" } })).ok()).toBeTruthy();
  const invite = await (await request.post(`/rooms/${room}/invites`, { headers: MASTER, data: { name } })).json();
  const reader = await browser.newPage();
  await reader.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await reader.goto("/");
  await reader.locator("#name").fill(name);
  await reader.locator("#roomToken").fill(invite.token);
  await reader.evaluate((room) => window.doConnect(room), room);
  await expect(reader.locator("#curRoom")).toHaveText(room);
  await expect.poll(() => reader.evaluate(() => state.ws?.readyState)).toBe(1);
  await reader.locator("#sideLocaTab").click();
  await expect(reader.locator("#locaSummary .locadesc")).toHaveText("For reading.");
  await expect(reader.locator("#locaSummary .localabel")).toHaveText(["docs"]);
  const view = reader.locator("#locaSummary #identityOpen");
  await expect(view).toHaveText("View");
  await view.click();
  await expect(reader.locator("#extCreate")).toBeVisible();
  await expect(reader.locator("#extCreateSubmit")).toBeHidden();
  await expect(reader.locator("#extCreateCancel")).toHaveText("Close");
  await expect(reader.locator("#locaDesc")).toBeDisabled();
  await expect(reader.locator("#locaDesc")).toHaveValue("For reading.");
  await expect(reader.locator("#locaIconOpts input[value=book]")).toBeChecked();
  await expect(reader.locator("#locaIconOpts input[value=book]")).toBeDisabled();
  await expect(reader.locator("#locaIdentityNote")).toContainText("Only this loca's operator");
  await expect(reader.locator("#extCreateCancel")).toBeFocused();
  await reader.keyboard.press("Escape");
  await expect(reader.locator("#extCreate")).toBeHidden();
  await reader.close();
});

test("a coloured icon reads in both themes, and the panel fits a phone", async ({ browser, request }) => {
  const room = "id-colour";
  const made = await request.post("/rooms", { headers: MASTER, data: { name: room, icon: "flask", color: "yellow" } });
  expect([201, 409]).toContain(made.status());
  const page = await masterPage(browser, request, { width: 390, height: 780 });
  await page.evaluate((room) => joinRoom(room), room);
  await expect.poll(() => page.evaluate(() => state.ws?.readyState)).toBe(1);
  await expect(page.locator("#roomMark svg")).toHaveCount(1);
  const reading = () => page.evaluate(() => {
    const parse = (value) => value.match(/[\d.]+/g).slice(0, 3).map(Number);
    const lum = ([r, g, b]) => {
      const c = [r, g, b].map(v => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; });
      return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    };
    const mark = getComputedStyle(document.getElementById("roomMark")).color;
    const ground = getComputedStyle(document.querySelector(".top")).backgroundColor;
    const body = getComputedStyle(document.body).backgroundColor;
    const bg = /rgba\(.*,\s*0\)$/.test(ground) || ground === "transparent" ? body : ground;
    const [a, b] = [lum(parse(mark)), lum(parse(bg))].sort((x, y) => y - x);
    return { mark, ratio: (a + 0.05) / (b + 0.05) };
  });
  await page.evaluate(() => applyAppearance({ theme: "dark" }));
  const dark = await reading();
  await page.evaluate(() => applyAppearance({ theme: "light" }));
  const light = await reading();
  await page.evaluate(() => applyAppearance({ theme: "loca" }));
  expect((await reading()).ratio).toBeGreaterThanOrEqual(3);
  expect(dark.mark).not.toBe(light.mark);
  expect(dark.ratio).toBeGreaterThanOrEqual(3);
  expect(light.ratio).toBeGreaterThanOrEqual(3);

  // At phone width the panel scrolls inside itself and never sideways.
  await page.evaluate(() => openCreateLoca("phone-loca"));
  await expect(page.locator("#extCreate")).toBeVisible();
  const card = page.locator("#extCreateForm");
  const sizes = await card.evaluate(el => [el.scrollWidth, el.clientWidth]);
  expect(sizes[0]).toBeLessThanOrEqual(sizes[1]);
  await page.locator("#locaColorOpts .identopt[title=Purple]").scrollIntoViewIfNeeded();
  await expect(page.locator("#locaColorOpts .identopt[title=Purple]")).toBeInViewport();
  await page.keyboard.press("Escape");
  await page.close();
});

// A loca's description and labels are for those who may enter it. Once the
// session that could read them is over, or the person is in another loca,
// nothing of them stays in the page.
async function identityFieldsAreEmpty(page) {
  await expect(page.locator("#extCreate")).toBeHidden();
  await expect(page.locator("#locaTitle")).toHaveValue("");
  await expect(page.locator("#locaDesc")).toHaveValue("");
  await expect(page.locator("#locaLabels")).toHaveValue("");
  await expect(page.locator("#extCreateName")).toHaveValue("");
}

test("the identity panel closes and empties when its session ends, open or still opening", async ({ browser, request }) => {
  const room = "id-private";
  const made = await request.post("/rooms", {
    headers: MASTER, data: { name: room, description: "Only for this table.", labels: ["confidential"] },
  });
  expect([201, 409]).toContain(made.status());

  // Open, then the session ends.
  let page = await masterPage(browser, request);
  await page.evaluate((room) => joinRoom(room), room);
  await expect.poll(() => page.evaluate(() => state.ws?.readyState)).toBe(1);
  await page.locator("#sideLocaTab").click();
  await page.locator("#locaSummary #identityOpen").click();
  await expect(page.locator("#extCreate")).toBeVisible();
  await expect(page.locator("#locaDesc")).toHaveValue("Only for this table.");
  await expect(page.locator("#locaLabels")).toHaveValue("confidential");
  await page.evaluate(() => signOutHere());
  await expect(page.locator("body")).toHaveClass(/locked/);
  await identityFieldsAreEmpty(page);
  await page.close();

  // Still opening (the Building's numbers are being read) when it ends.
  page = await masterPage(browser, request);
  await page.evaluate((room) => joinRoom(room), room);
  await expect.poll(() => page.evaluate(() => state.ws?.readyState)).toBe(1);
  await page.locator("#sideLocaTab").click();
  await expect(page.locator("#locaSummary #identityOpen")).toBeVisible();
  let release;
  const held = new Promise(resolve => { release = resolve; });
  await page.route("**/health", async route => { await held; await route.continue(); });
  await page.locator("#locaSummary #identityOpen").click();
  await page.evaluate(() => signOutHere());
  await expect(page.locator("body")).toHaveClass(/locked/);
  release();
  await page.waitForTimeout(500);
  await identityFieldsAreEmpty(page);
  await page.close();
});

test("the identity panel of one loca does not stay open over another", async ({ browser, request }) => {
  for (const [name, description] of [["id-here", "This one."], ["id-there", "The other one."]]) {
    const made = await request.post("/rooms", { headers: MASTER, data: { name, description } });
    expect([201, 409]).toContain(made.status());
  }
  const page = await masterPage(browser, request);
  await page.evaluate(() => joinRoom("id-here"));
  await expect.poll(() => page.evaluate(() => state.ws?.readyState)).toBe(1);
  await page.locator("#sideLocaTab").click();
  await page.locator("#locaSummary #identityOpen").click();
  await expect(page.locator("#locaDesc")).toHaveValue("This one.");
  await page.evaluate(() => joinRoom("id-there"));
  await identityFieldsAreEmpty(page);
  await page.close();
});

test("a title is the name people see; the id stays the address and keeps pins and read position", async ({ browser, request }) => {
  const room = "id-titled";
  const made = await request.post("/rooms", { headers: MASTER, data: { name: room, description: "Ship it." } });
  expect([201, 409]).toContain(made.status());
  // Someone seated there, for the Building's location column, who says a
  // line (so there is a read position to keep).
  const seated = "id-titled-seat";
  const admitted = await request.post("/members", { headers: MASTER, data: { name: seated, kind: "user" } });
  const key = admitted.ok() ? (await admitted.json()).token : null;
  if (key) {
    expect((await request.post(`/rooms/${room}/invites`, { headers: MASTER, data: { name: seated } })).ok()).toBeTruthy();
    const session = await (await request.post("/sessions", { headers: { "x-room-token": key }, data: { name: seated, kind: "user" } })).json();
    const said = await request.post(`/rooms/${room}/messages`, {
      headers: { "x-session-token": session.session_token }, data: { sender: seated, sender_type: "user", text: "before the title" },
    });
    expect(said.ok(), await said.text()).toBeTruthy();
  }
  const page = await masterPage(browser, request);
  const puts = [];
  page.on("request", req => {
    if (req.method() === "PUT" && new URL(req.url()).pathname === `/rooms/${room}/settings`) puts.push(req.postDataJSON());
  });
  await page.evaluate((room) => joinRoom(room), room);
  await expect.poll(() => page.evaluate(() => state.ws?.readyState)).toBe(1);
  // Pinned and read up to that line, both kept under the id.
  await expect.poll(() => page.evaluate((room) => Number(state.readCursors[room] || 0), room)).toBeGreaterThan(0);
  await page.evaluate((room) => updateRoomPreference("pin", room), room);
  const readBefore = await page.evaluate((room) => state.readCursors[room], room);

  // The create panel opens a loca under its name: no title field there.
  await page.locator("#newRoomBtn").click();
  await expect(page.locator("#locaTitleField")).toBeHidden();
  await page.keyboard.press("Escape");

  // This Loca › Loca identity: the name is the id, the title its own field.
  await page.locator("#sideLocaTab").click();
  await page.locator("#locaSummary #identityOpen").click();
  await expect(page.locator("#extCreateName")).toHaveValue(room);
  await expect(page.locator("#locaTitleField")).toBeVisible();
  await expect(page.locator("#locaTitle")).toHaveValue("");
  await page.locator("#locaTitle").fill("Release Fifteen");
  await expect(page.locator("#locaTitleCount")).toHaveText("15 / 64");
  await page.locator("#extCreateSubmit").click();
  await expect(page.locator("#extCreate")).toBeHidden();
  expect(puts).toEqual([{ title: "Release Fifteen" }]);
  expect((await serverSettings(request, room)).title).toBe("Release Fifteen");

  // The title everywhere the name was; the id one step away.
  await expect(page.locator("#curRoom")).toHaveText("Release Fifteen");
  await expect(page.locator("#curRoom")).toHaveAttribute("title", `id: ${room}`);
  await expect(page.locator("#locaSummary .locasummaryhead b")).toHaveText("Release Fifteen");
  await expect(page.locator("#locaSummary .locasummaryhead .locaid")).toHaveText(`id ${room}`);
  await page.locator("#sideBuildingTab").click();
  const row = page.locator("#roomList .room", { has: page.locator(".rname", { hasText: /^Release Fifteen$/ }) });
  await expect(row).toHaveCount(1);
  await expect(row).toHaveAttribute("title", `Release Fifteen\nid: ${room}\nShip it.`);
  await page.evaluate(() => switchTab("people"));
  const chip = page.locator("#peopleList .locachip", { hasText: "#Release Fifteen" });
  await expect(chip).toHaveCount(1);
  await expect(chip).toHaveAttribute("title", `seated in Release Fifteen (id: ${room})`);
  await page.evaluate(() => switchTab("chat"));
  await expect(page.locator("#curRoom")).toHaveText("Release Fifteen");

  // An edit that does not touch the title does not send it, and it stays.
  await page.locator("#sideLocaTab").click();
  await page.locator("#locaSummary #identityOpen").click();
  await expect(page.locator("#locaTitle")).toHaveValue("Release Fifteen");
  await page.locator("#locaDesc").fill("Ship it soon.");
  await page.locator("#extCreateSubmit").click();
  await expect(page.locator("#extCreate")).toBeHidden();
  expect(puts[1]).toEqual({ description: "Ship it soon." });
  expect((await serverSettings(request, room)).title).toBe("Release Fifteen");

  // After a reload the pin and the read position are still there: they are
  // kept under the id, which did not move.
  await page.reload();
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate((room) => joinRoom(room), room);
  await expect.poll(() => page.evaluate(() => state.ws?.readyState)).toBe(1);
  expect(await page.evaluate((room) => loadRoomPreferences().pinned.includes(room), room)).toBe(true);
  expect(await page.evaluate((room) => state.readCursors[room], room)).toBe(readBefore);
  await expect(page.locator("#roomList .rname", { hasText: /^Release Fifteen$/ })).toHaveCount(1);

  // Cleared, the id shows again.
  await page.locator("#sideLocaTab").click();
  await page.locator("#locaSummary #identityOpen").click();
  await page.locator("#locaTitle").fill("");
  await page.locator("#extCreateSubmit").click();
  await expect(page.locator("#extCreate")).toBeHidden();
  await expect(page.locator("#curRoom")).toHaveText(room);
  await expect(page.locator("#curRoom")).toHaveAttribute("title", "");
  await expect(page.locator("#locaSummary .locasummaryhead .locaid")).toHaveCount(0);
  await page.close();
});
