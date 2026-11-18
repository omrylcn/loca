// The Profile panel: who you are here (You), where (Building, this loca,
// your floors), how you are signed in (Session and access), and the look
// (Theme, Reading). Each section shows what the server answered and nothing
// it did not; each new choice applies at once and survives a reload.
const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");
const { MASTER, smasterKey } = require("./support/switch");

const ROOM = "profile-panel-room";
const DEFAULTS = {
  theme: "dark", accent: "teal", density: "comfortable", font_size: "normal", font_family: "mono",
  accent_custom: "", layout: "bubbles", time_format: "auto", measure: "normal", own_tint: "on", motion: "system",
  // Sounds (Profile → Sounds) share the document, at their defaults.
  sound: "on", sound_message: "off", sound_mention: "on", sound_dm: "on", sound_notice: "on",
  sound_volume: "medium", sound_background: "on",
};

async function newPage(browser, viewport) {
  const context = await browser.newContext(viewport ? { viewport } : {});
  await context.addInitScript(() => {
    try { localStorage.setItem("loca-gs-seen", "1"); } catch (e) {}
  });
  return { context, page: await context.newPage() };
}

async function signInAsMaster(page, request) {
  const { pairing_code: code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  await page.goto("/");
  await page.locator("#masterEntryButton").click();
  await page.locator("#pairingCode").fill(code);
  await page.evaluate(room => window.doConnect(room), ROOM);
  await expect(page.locator("#whoami")).toContainText("MASTER");
}

async function signInAsMember(page, request, name) {
  const admitted = await request.post("/members", { headers: MASTER, data: { name, kind: "user" } });
  expect(admitted.ok(), await admitted.text()).toBeTruthy();
  const { token } = await admitted.json();
  expect((await request.post(`/rooms/${ROOM}/invites`, { headers: MASTER, data: { name } })).ok()).toBeTruthy();
  await page.goto("/");
  await page.locator("#entryKind").selectOption("human");
  await page.locator("#name").fill(name);
  await page.locator("#roomToken").fill(token);
  await page.locator("#connectBtn").click();
  await expect.poll(() => page.evaluate(() => state.profile?.principal?.display_name)).toBe(name);
  await expect.poll(() => page.evaluate(() => state.room)).toBe(ROOM);
  await expect.poll(() => page.evaluate(() => state.profile?.loca?.room)).toBe(ROOM);
}

async function openPanel(page) {
  await page.locator("#whoami .profilemenu > summary").click();
  await expect(page.locator("#whoami .profilepanel")).toBeVisible();
}

const section = (page, id) => page.locator(`#whoami #${id}`);
const html = page => page.locator("html");
const savedPrefs = page => page.waitForResponse(response =>
  response.url().endsWith("/profile/preferences") && response.request().method() === "PUT" && response.ok());

test.beforeEach(async ({ request }) => {
  await provisionRooms(request, [ROOM]);
  await request.put("/profile/preferences", { headers: MASTER, data: DEFAULTS });
});

test.afterEach(async ({ request }) => {
  await request.put("/profile/preferences", { headers: MASTER, data: DEFAULTS });
  await request.put("/profile/card", { headers: MASTER, data: { full_name: null, icon: null, avatar_color: null, avatar_shape: null } });
});

test("a member's panel says who, where and how they are signed in, from the server's answer", async ({ browser, request }) => {
  const name = `pp-member-${Date.now() % 100000}`;
  const { context, page } = await newPage(browser);
  await signInAsMember(page, request, name);
  await openPanel(page);
  for (const id of ["profileSecYou", "profileSecPlace", "profileSecAccess", "profileSecTheme", "profileSecReading"]) {
    await expect(section(page, id)).toBeVisible();
  }
  const you = section(page, "profileSecYou");
  await expect(you.locator(".profileconnected b").first()).toHaveText(name);
  await expect(you.locator(".profileconnected")).toContainText("Human");
  await expect(you.locator(".profileconnected .rankbadge")).toHaveCount(0);
  // A member's @name is not renamed here: said, not offered.
  await expect(page.locator("#profileNameInput")).toHaveCount(0);
  await expect(you).toContainText("Only the Master and Smasters rename their own @name");

  const place = section(page, "profileSecPlace");
  const building = place.locator(".placebuilding");
  await expect(building.locator("b.role")).toHaveText("MEMBER");
  await expect(building).toContainText("you sit at the locas you are invited to");
  const loca = place.locator(".placeloca");
  await expect(loca.locator(".placename")).toHaveText(ROOM);
  await expect(loca).toContainText("Seated at the table");
  await expect(loca.locator(".loca-role")).toHaveText("PARTICIPANT");
  await expect(place.locator(".grpchip")).toHaveText(["Ground floor"]);
  await expect(place).toContainText("Ground floor only");

  // A floor the server lists is a chip, named as the sidebar names floors.
  const group = await (await request.post("/groups", { headers: MASTER, data: { name: `Pp${Date.now() % 100000}` } })).json();
  const me = await page.evaluate(() => state.principalId);
  expect((await request.put(`/groups/${group.id}/members/${me}`, { headers: MASTER })).ok()).toBeTruthy();
  await page.evaluate(() => fetchProfile(true));
  await expect(place.locator(".grpchip")).toHaveText(["Ground floor", `${group.name} floor`]);
  await expect(place).not.toContainText("Ground floor only");

  // The session: through which key, until when, and how far that is.
  const access = section(page, "profileSecAccess");
  const expires = await page.evaluate(() => state.profile.session.expires_at);
  expect(expires).toBeGreaterThan(Date.now());
  await expect(access.locator(".sessioncard")).toContainText(new RegExp(`Bounded session, connected as ${name} through .+, until .+\\(in \\d+ (hours|days|minutes)\\)`));
  const current = access.locator(".credentialrow", { hasText: "current" });
  await expect(current).toHaveCount(1);
  await expect(current).toContainText("This browser's key");
  await expect(current).toContainText(/Added .+ · last used/);
  await expect(current.getByRole("button", { name: "revoke & end session" })).toBeVisible();
  await context.close();
});

test("a Smaster is told what a rename does; the Master's roles are explained", async ({ browser, request }) => {
  const key = await smasterKey(request, `pp-smaster-${Date.now() % 100000}`);
  const smaster = await newPage(browser);
  await smaster.page.goto("/");
  await smaster.page.locator("#entryKind").selectOption("smaster");
  await smaster.page.locator("#name").fill("");
  await smaster.page.locator("#roomToken").fill(key);
  await smaster.page.locator("#connectBtn").click();
  await expect.poll(() => smaster.page.evaluate(() => state.profile?.building_role)).toBe("smaster");
  await openPanel(smaster.page);
  await expect(smaster.page.locator("#profileNameInput")).toBeVisible();
  await expect(section(smaster.page, "profileSecYou")).toContainText("Your past messages follow you to the new name");
  const building = section(smaster.page, "profileSecPlace").locator(".placebuilding");
  await expect(building.locator("b.role")).toHaveText("SMASTER");
  await expect(building).toContainText("Ending a membership is the Master's");
  await smaster.context.close();

  const master = await newPage(browser);
  await signInAsMaster(master.page, request);
  await openPanel(master.page);
  const place = section(master.page, "profileSecPlace");
  await expect(place.locator(".placebuilding").locator("b.role")).toHaveText("MASTER");
  await expect(place.locator(".placebuilding")).toContainText("the final word");
  // The Master founded this loca, so it holds the seat by appointment here
  // (rank-not-operator.spec.js covers a loca where it holds it by rank).
  const loca = place.locator(".placeloca");
  await expect(loca.locator(".loca-role")).toHaveText("OPERATOR");
  await expect(loca).toContainText("appointed");
  await expect(loca).toContainText("Appointed here: you set the mode and turn order, and you moderate.");
  await expect(loca).toContainText("Building management access");
  await master.context.close();
});

test("the avatar studio previews a pick, saves colour, shape and icon, and a refused value changes nothing", async ({ browser, request }) => {
  const { context, page } = await newPage(browser);
  await signInAsMaster(page, request);
  await openPanel(page);
  const preview = page.locator("#profileAvatarPreview .avatar");
  await expect(preview).toHaveClass(/av-circle/);
  await expect(preview.locator(".avinitials")).toHaveText("MA");

  await page.locator("#profileFullName").fill("Ada Lovelace");
  await page.locator('#profileCard input[data-avatar-color][value="plum"]').check();
  await page.locator('#profileCard input[data-avatar-shape][value="hexagon"]').check();
  // Before Save: the big avatar already shows the pick, the server not yet.
  await expect(preview).toHaveClass(/av-hexagon/);
  await expect(preview).toHaveClass(/av-filled/);
  await expect(preview.locator(".avinitials")).toHaveText("AL");
  await expect(page.locator("#profileCardNote")).toContainText("Not saved yet");
  expect((await (await request.get("/profile", { headers: MASTER })).json()).principal.avatar_color).toBeUndefined();

  await page.locator('#profileCard input[data-profile-icon][value="fox"]').check();
  await expect(preview.locator("svg.picon")).toHaveCount(1);
  await page.locator("#profileCardSave").click();
  await expect(page.locator("#profileCardNote")).toHaveText("Saved.");
  const saved = (await (await request.get("/profile", { headers: MASTER })).json()).principal;
  expect([saved.full_name, saved.icon, saved.avatar_color, saved.avatar_shape]).toEqual(["Ada Lovelace", "fox", "plum", "hexagon"]);
  // The account button wears it too, and a reload keeps it.
  await page.reload();
  await expect(page.locator("#whoami summary .avatar.av-hexagon.av-filled svg.picon")).toHaveCount(1);

  // Closed lists: an unknown colour or shape is refused and changes nothing.
  for (const data of [{ avatar_color: "#ff00ff" }, { avatar_shape: "star" }, { icon: "dragon" }]) {
    const refused = await request.put("/profile/card", { headers: MASTER, data });
    expect(refused.status(), JSON.stringify(data)).toBe(400);
  }
  const after = (await (await request.get("/profile", { headers: MASTER })).json()).principal;
  expect([after.icon, after.avatar_color, after.avatar_shape]).toEqual(["fox", "plum", "hexagon"]);
  await context.close();
});

test("a theme, an accent and every reading choice apply at once and survive a reload", async ({ browser, request }) => {
  const { context, page } = await newPage(browser);
  await signInAsMaster(page, request);
  await openPanel(page);
  const look = page.locator("#profileAppearance");
  const pick = async (label) => {
    const write = savedPrefs(page);
    await look.getByRole("radio", { name: label, exact: true }).check();
    await write;
  };
  await pick("Paper");
  await expect(html(page)).toHaveAttribute("data-theme", "paper");
  await expect(html(page)).toHaveAttribute("data-scheme", "light");
  expect(await page.evaluate(() => getComputedStyle(document.body).backgroundColor)).toBe("rgb(246, 241, 231)");
  await pick("Indigo");
  expect(await page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue("--accent").trim())).toBe("#3d4bc4");

  const mine = page.locator("#appearancePreview .row.mine");
  await pick("Flat");
  await expect(html(page)).toHaveAttribute("data-layout", "flat");
  expect(await mine.evaluate(el => getComputedStyle(el).justifyContent)).toBe("flex-start");
  await pick("24-hour");
  await expect(mine.locator(".time")).toHaveText(/^\d{2}:\d{2}$/);
  await pick("Relative");
  await expect(mine.locator(".time")).toHaveText("1m ago");
  await expect(page.locator("#appearancePreview .row").first().locator(".time")).toHaveText("4m ago");
  await pick("Narrow");
  await expect(html(page)).toHaveAttribute("data-measure", "narrow");
  await pick("Plain");
  await expect(html(page)).toHaveAttribute("data-own-tint", "off");
  await pick("Reduced");
  await expect(html(page)).toHaveAttribute("data-motion", "reduced");
  expect(await page.evaluate(() => getComputedStyle(document.querySelector(".profilechevron")).transitionDuration)).toBe("0s");

  const stored = await (await request.get("/profile/preferences", { headers: MASTER })).json();
  expect(stored).toMatchObject({ theme: "paper", accent: "indigo", layout: "flat", time_format: "relative",
    measure: "narrow", own_tint: "off", motion: "reduced" });
  await page.reload();
  for (const [attr, value] of [["data-theme", "paper"], ["data-accent", "indigo"], ["data-layout", "flat"],
    ["data-time-format", "relative"], ["data-measure", "narrow"], ["data-own-tint", "off"], ["data-motion", "reduced"]]) {
    await expect(html(page), attr).toHaveAttribute(attr, value);
  }
  await openPanel(page);
  await expect(look.getByRole("radio", { name: "Relative", exact: true })).toBeChecked();

  // Reset: every choice back to its default, here and on the server.
  const write = savedPrefs(page);
  await page.locator("#appearanceReset").click();
  await write;
  await expect(html(page)).toHaveAttribute("data-theme", "dark");
  await expect(html(page)).toHaveAttribute("data-layout", "bubbles");
  await expect(look.getByRole("radio", { name: "Cards", exact: true })).toBeChecked();
  expect(await (await request.get("/profile/preferences", { headers: MASTER })).json()).toEqual(DEFAULTS);

  // The server's lists are closed: an unknown value is refused, nothing moves.
  for (const data of [{ layout: "grid" }, { theme: "solarized" }, { accent_custom: "red" }, { time_format: "H24" }]) {
    expect((await request.put("/profile/preferences", { headers: MASTER, data })).status(), JSON.stringify(data)).toBe(400);
  }
  expect(await (await request.get("/profile/preferences", { headers: MASTER })).json()).toEqual(DEFAULTS);
  await context.close();
});

test("a custom accent too dark for a dark theme is lightened, said so, and kept after a reload", async ({ browser, request }) => {
  const { context, page } = await newPage(browser);
  await signInAsMaster(page, request);
  await openPanel(page);
  const write = savedPrefs(page);
  await page.locator("#accentCustomInput").evaluate(input => {
    input.value = "#1a1a40";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    input.dispatchEvent(new Event("change", { bubbles: true }));
  });
  await write;
  await expect(html(page)).toHaveAttribute("data-accent", "custom");
  const used = () => page.evaluate(() => document.documentElement.style.getPropertyValue("--accent"));
  const accent = await used();
  expect(accent).toMatch(/^#[0-9a-f]{6}$/);
  expect(accent).not.toBe("#1a1a40");
  expect(await page.evaluate(a => contrastRatio(a, "#161b23"), accent)).toBeGreaterThanOrEqual(4.5);
  await expect(page.locator("#accentNote")).toContainText("lightened");
  await expect(page.locator("#profileAppearance").getByRole("radio", { name: "Custom colour" })).toBeChecked();
  expect((await (await request.get("/profile/preferences", { headers: MASTER })).json()).accent_custom).toBe("#1a1a40");
  await page.reload();
  await expect(html(page)).toHaveAttribute("data-accent", "custom");
  expect(await used()).toBe(accent);
  // Another accent clears the custom colour from the page.
  await openPanel(page);
  const back = savedPrefs(page);
  await page.locator("#profileAppearance").getByRole("radio", { name: "Teal", exact: true }).check();
  await back;
  expect(await used()).toBe("");
  await context.close();
});

test("the panel is a sheet that fits a phone, and Escape closes it (guard)", async ({ browser, request }) => {
  const { context, page } = await newPage(browser, { width: 390, height: 844 });
  await signInAsMaster(page, request);
  await page.evaluate(() => setMobileSidebar(true));
  await openPanel(page);
  const box = await page.locator("#whoami .profilepanel").boundingBox();
  expect(box.x).toBeLessThanOrEqual(1);
  expect(box.width).toBeGreaterThanOrEqual(388);
  expect(await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth)).toBeLessThanOrEqual(0);
  const panel = await page.evaluate(() => { const p = document.querySelector("#whoami .profilepanel"); return p.scrollWidth - p.clientWidth; });
  expect(panel).toBeLessThanOrEqual(0);
  await page.keyboard.press("Escape");
  await page.locator("#whoami .profilemenu > summary").focus();
  await page.locator("#whoami .profilemenu > summary").press("Enter");
  await expect(page.locator("#whoami details.profilemenu")).toHaveAttribute("open", "");
  await page.locator("#profileFullName").focus();
  await page.keyboard.press("Escape");
  await expect(page.locator("#whoami details.profilemenu")).not.toHaveAttribute("open", "");
  await context.close();
});
