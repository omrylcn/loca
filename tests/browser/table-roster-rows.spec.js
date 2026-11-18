const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

// "At the table" in the 320px sidebar: one line per person. The row used to
// fold in two, with the lead diamond, release and three emoji buttons
// (mute, kick, ban) on a second line; on a machine without an emoji font
// those were empty boxes. Now the row keeps release, one mute toggle and
// the DM icon, and kick, ban and the lead title sit behind "⋯"; kick and ban
// ask in words first.

const MASTER = { "x-admin-token": "MASTER" };
const ROOM = "roster-rows";
const LONG = "rr-agent-with-a-rather-long-synthetic-name";
const PEOPLE = [LONG, "rr-helper", "rr-quiet"];
// Pictographic emoji, and the variation selector that turns a symbol into one.
const EMOJI = /[\p{Extended_Pictographic}\u{FE0F}]/u;

async function seatPeople(request) {
  await provisionRooms(request, [ROOM]);
  for (const name of PEOPLE) {
    const admitted = await request.post("/members", { headers: MASTER, data: { name, kind: "agent" } });
    expect([200, 201]).toContain(admitted.status());
    // Each test seats them again; one already seated keeps that seat.
    const invited = await request.post(`/rooms/${ROOM}/invites`, { headers: MASTER, data: { name } });
    const text = await invited.text();
    expect(invited.ok() || text.includes("already holds"), text).toBeTruthy();
  }
  // A muted seat and a lead: the row carries both states.
  const muted = await request.post(`/rooms/${ROOM}/moderate`, { headers: MASTER, data: { action: "mute", name: "rr-quiet" } });
  expect(muted.ok(), await muted.text()).toBeTruthy();
  const lead = await request.post(`/rooms/${ROOM}/lead`, { headers: MASTER, data: { lead: "rr-helper" } });
  expect(lead.ok(), await lead.text()).toBeTruthy();
}

async function masterPage(browser, request, viewport) {
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const info = await (await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" },
  })).json();
  const page = await browser.newPage({ viewport });
  await page.addInitScript(({ token, expiresAt, room }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "" }));
    localStorage.setItem("loca-seated-rooms", JSON.stringify([room]));
  }, { token: info.session_token, expiresAt: info.expires_at, room: ROOM });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await enter(page);
  return page;
}

async function memberPage(browser, request, name, viewport) {
  const { token } = await (await request.post("/members", { headers: MASTER, data: { name, kind: "user" } })).json();
  const invited = await request.post(`/rooms/${ROOM}/invites`, { headers: MASTER, data: { name } });
  expect(invited.ok(), await invited.text()).toBeTruthy();
  const page = await browser.newPage({ viewport });
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await page.goto("/");
  await page.locator("#entryKind").selectOption("human");
  await page.locator("#name").fill(name);
  await page.locator("#roomToken").fill(token);
  await page.locator("#connectBtn").click();
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await enter(page);
  return page;
}

async function enter(page) {
  await page.evaluate(room => joinRoom(room), ROOM);
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  await page.evaluate(() => {
    if (window.innerWidth <= 760) setMobileSidebar(true);
    setSidebarView("loca");
  });
}

const seat = (page, name) => page.locator("#onlineList .omem").filter({ hasText: name });

// Every roster row that is taller than one line, or reaches past the sidebar.
function brokenRows(page) {
  return page.evaluate(() => {
    const side = document.querySelector(".side").getBoundingClientRect();
    return [...document.querySelectorAll("#onlineList .omem")].map(row => {
      const r = row.getBoundingClientRect();
      const parts = [...row.children].filter(c => c.getBoundingClientRect().width > 0);
      const mids = parts.map(c => { const b = c.getBoundingClientRect(); return (b.top + b.bottom) / 2; });
      const oneLine = mids.every(m => Math.abs(m - mids[0]) < 6) && r.height <= 34;
      const inside = r.right <= side.right + 0.5 && row.scrollWidth <= row.clientWidth + 1
        && parts.every(c => c.getBoundingClientRect().right <= r.right + 0.5);
      return oneLine && inside ? null : `${row.textContent.trim().slice(0, 40)} (${Math.round(r.height)}px)`;
    }).filter(Boolean);
  });
}

for (const viewport of [{ width: 1280, height: 900 }, { width: 390, height: 844 }]) {
  test(`At the table at ${viewport.width}: every seat is one line inside the sidebar`, async ({ browser, request }) => {
    await seatPeople(request);
    const page = await masterPage(browser, request, viewport);
    for (const name of PEOPLE) await expect(seat(page, name)).toHaveCount(1);
    await expect(seat(page, "rr-quiet").locator("[data-mod='unmute']")).toHaveCount(1);
    await expect(seat(page, LONG).locator("button.release")).toBeVisible();
    await expect(page.locator("#onlineList .omem.self")).toHaveCount(1);
    expect(await brokenRows(page)).toEqual([]);
    // The lead and the muted state stay visible on the line.
    await expect(seat(page, "rr-helper").locator(".leadtag")).toBeVisible();
    await expect(seat(page, "rr-quiet").locator("[data-mod='unmute']")).toHaveAttribute("aria-pressed", "true");
    await expect(seat(page, LONG)).toHaveClass(/away/);
    await page.close();
  });
}

test("no emoji in the roster's buttons: mute is one SVG toggle, the rest sit behind ⋯", async ({ browser, request }) => {
  await seatPeople(request);
  const page = await masterPage(browser, request, { width: 1280, height: 900 });
  await expect(seat(page, LONG)).toHaveCount(1);
  const mute = seat(page, LONG).locator("[data-mod='mute']");
  await expect(mute).toHaveAttribute("aria-pressed", "false");
  await expect(mute.locator("svg")).toHaveCount(1);
  // Kick and ban are no longer on the row.
  await expect(seat(page, LONG).locator("[data-seat-ask]:visible")).toHaveCount(0);
  await seat(page, LONG).locator("[data-seat-menu]").click();
  await expect(page.locator("#seatMenu")).toBeVisible();
  const glyphs = await page.evaluate(re => [...document.querySelectorAll("#onlineList button, #onlineList .mflag, #seatMenu button")]
    .map(b => b.textContent.trim()).filter(t => new RegExp(re, "u").test(t)), EMOJI.source);
  expect(glyphs).toEqual([]);
  await expect(page.locator("#seatMenu [role='menuitem'] svg")).toHaveCount(3);
  await page.close();
});

test("the ⋯ menu holds what this viewer may do: the Master's, and nothing for a plain member", async ({ browser, request }) => {
  await seatPeople(request);
  const master = await masterPage(browser, request, { width: 1280, height: 900 });
  await expect(seat(master, LONG)).toHaveCount(1);
  await seat(master, LONG).locator("[data-seat-menu]").click();
  await expect(master.locator("#seatMenu [role='menuitem']")).toHaveText(["Make lead", "Kick…", "Ban…"]);
  await master.keyboard.press("Escape");
  await expect(master.locator("#seatMenu")).toBeHidden();
  // The lead's own menu ends the title instead of giving it.
  await seat(master, "rr-helper").locator("[data-seat-menu]").click();
  await expect(master.locator("#seatMenu [role='menuitem']")).toHaveText(["End lead", "Kick…", "Ban…"]);
  await master.keyboard.press("Escape");
  // On one's own seat only the lead title is offered.
  await master.locator("#onlineList .omem.self [data-seat-menu]").click();
  await expect(master.locator("#seatMenu [role='menuitem']")).toHaveText(["Make lead"]);
  await master.keyboard.press("Escape");

  const member = await memberPage(browser, request, "rr-plain", { width: 1280, height: 900 });
  await expect(seat(member, LONG)).toHaveCount(0); // a member sees who is connected
  await expect(seat(member, "rr-plain")).toHaveCount(1);
  await expect(member.locator("#onlineList [data-seat-menu]")).toHaveCount(0);
  await expect(member.locator("#onlineList button.release, #onlineList [data-mod]")).toHaveCount(0);
  // The Master, seated and connected, is someone the member may write to.
  await expect(member.locator("#onlineList .omem:not(.self) .dmbtn").first()).toBeVisible();
  for (const page of [master, member]) await page.close();
});

test("kick asks a yes/no question in words; No changes nothing, Yes kicks", async ({ browser, request }) => {
  await seatPeople(request);
  const page = await masterPage(browser, request, { width: 390, height: 844 });
  page.on("dialog", dialog => { throw new Error(`a native dialog opened: ${dialog.message()}`); });
  const posts = [];
  page.on("request", r => { if (r.method() === "POST" && r.url().includes("/moderate")) posts.push(r.postData()); });

  await seat(page, LONG).locator("[data-seat-menu]").click();
  await page.locator("#seatMenu [role='menuitem']", { hasText: "Kick" }).click();
  const ask = page.locator("#onlineList [role='alertdialog']");
  await expect(ask).toContainText(`Kick ${LONG} from this loca?`);
  await expect(ask.locator("button")).toHaveText(["No", "Yes, kick"]);
  await ask.locator("button", { hasText: "No" }).click();
  await expect(ask).toHaveCount(0);
  await expect(seat(page, LONG)).toHaveCount(1);
  expect(posts).toEqual([]);
  const invites = await (await request.get(`/rooms/${ROOM}/invites`, { headers: MASTER })).json();
  expect(invites.map(i => i.name)).toContain(LONG);

  await seat(page, LONG).locator("[data-seat-menu]").click();
  await page.locator("#seatMenu [role='menuitem']", { hasText: "Kick" }).click();
  await page.locator("#onlineList [role='alertdialog'] button", { hasText: "Yes, kick" }).click();
  await expect(seat(page, LONG)).toHaveCount(0);
  expect(posts.map(p => JSON.parse(p))).toEqual([{ action: "kick", name: LONG }]);
  await page.close();
});
