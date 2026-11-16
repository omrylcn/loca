const { test, expect } = require("@playwright/test");

// The create panel's preset: "Loca" (the panel as it always was) or
// "Lounge" (chat only, every seat, the chat icon). A preset only fills the
// panel's existing fields; nothing about it is stored or sent, every field
// stays editable, and the server decides every value.

const MASTER = { "x-admin-token": "MASTER" };

async function masterPage(browser, request) {
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const info = await (await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" },
  })).json();
  const page = await browser.newPage();
  await page.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "Master" }));
  }, { token: info.session_token, expiresAt: info.expires_at });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  return page;
}

function recordCreates(page) {
  const posted = [];
  page.on("request", req => {
    if (req.method() === "POST" && new URL(req.url()).pathname === "/rooms") posted.push(req.postDataJSON());
  });
  return posted;
}

async function serverSettings(request, room) {
  return (await request.get(`/rooms/${room}/settings`, { headers: MASTER })).json();
}

async function serverExtensions(request, room) {
  return (await (await request.get(`/rooms/${room}/extensions`, { headers: MASTER })).json()).enabled;
}

async function ticked(page) {
  return page.locator("#extCreateDefault input:not(:disabled), #extCreateAdd input")
    .evaluateAll(rows => rows.filter(r => r.checked).map(r => r.value));
}

async function openPanel(page, name) {
  await page.locator("#newRoomName").fill(name);
  await page.locator("#newRoomBtn").click();
  await expect(page.locator("#extCreate")).toBeVisible();
  // The catalog has arrived once the default rows are drawn.
  await expect(page.locator("#extCreateDefault input")).toHaveCount(4);
}

test("Loca is the default: the panel and its request are as before, and the preset is never sent", async ({ browser, request }) => {
  const page = await masterPage(browser, request);
  const posted = recordCreates(page);
  await openPanel(page, "preset-plain");
  await expect(page.locator("#locaPreset")).toBeVisible();
  await expect(page.locator("#locaPreset input[value=loca]")).toBeChecked();
  await expect(page.locator("#locaPreset input[value=lounge]")).not.toBeChecked();
  await expect(page.locator("#locaDesc")).toHaveAttribute("placeholder", "One short paragraph. Agents read it when they enter.");
  await expect(page.locator("#locaSeats")).toHaveValue("7");
  expect(await ticked(page)).toEqual(["notes", "journal", "wiki"]);
  await expect(page.locator("#locaIconOpts input[value='']")).toBeChecked();
  // The submit button holds the focus as before: Enter goes straight through.
  await expect(page.locator("#extCreateSubmit")).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page.locator("#extCreate")).toBeHidden();
  await expect.poll(() => page.evaluate(() => state.room)).toBe("preset-plain");
  expect(posted).toEqual([{ name: "preset-plain", extensions: ["notes", "journal", "wiki"] }]);
  const settings = await serverSettings(request, "preset-plain");
  for (const key of ["description", "seats", "icon", "color", "labels", "preset", "kind"]) expect(settings).not.toHaveProperty(key);
  expect(await serverExtensions(request, "preset-plain")).toEqual(["notes", "journal", "wiki"]);
  await page.close();
});

test("Lounge fills chat only, every seat and the chat icon; the loca opened has exactly those settings", async ({ browser, request }) => {
  const health = await (await request.get("/health")).json();
  const ceiling = health.loca_capacity_max;
  expect(ceiling).toBeGreaterThan(health.loca_capacity);
  const page = await masterPage(browser, request);
  const posted = recordCreates(page);
  await openPanel(page, "preset-lounge");
  await page.locator("#locaPreset .presetopt", { hasText: "Lounge" }).click();
  await expect(page.locator("#locaPreset input[value=lounge]")).toBeChecked();
  expect(await ticked(page)).toEqual([]);
  await expect(page.locator("#extCreateTabs")).toHaveText("Tabs in this loca: Chat");
  await expect(page.locator("#locaSeats")).toHaveValue(String(ceiling));
  await expect(page.locator("#locaIconOpts input[value=chat]")).toBeChecked();
  await expect(page.locator("#locaColorOpts input[value='']")).toBeChecked();
  await expect(page.locator("#locaDesc")).toHaveAttribute("placeholder", "What people talk about here. Agents read it when they enter.");
  await expect(page.locator("#locaDesc")).toHaveValue("");
  await page.locator("#extCreateSubmit").click();
  await expect(page.locator("#extCreate")).toBeHidden();
  await expect.poll(() => page.evaluate(() => state.room)).toBe("preset-lounge");

  // Only fields the server already knows, with the values the panel showed.
  expect(posted).toEqual([{ name: "preset-lounge", extensions: [], seats: ceiling, icon: "chat" }]);
  const settings = await serverSettings(request, "preset-lounge");
  expect([settings.seats, settings.icon]).toEqual([ceiling, "chat"]);
  for (const key of ["description", "color", "labels", "preset", "kind"]) expect(settings).not.toHaveProperty(key);
  expect(await serverExtensions(request, "preset-lounge")).toEqual([]);
  // Chat only means the server refuses the extensions, not only the tabs.
  expect((await request.get("/rooms/preset-lounge/wiki", { headers: MASTER })).status()).toBe(403);
  expect((await request.get("/rooms/preset-lounge/notes", { headers: MASTER })).status()).toBe(403);
  await expect.poll(() => page.locator(".tabs [role=tab]:visible").allTextContents()).toEqual(["Chat"]);
  await page.close();
});

test("after Lounge every field stays editable, and what is sent is what the fields hold", async ({ browser, request }) => {
  const page = await masterPage(browser, request);
  const posted = recordCreates(page);
  await openPanel(page, "preset-edited");
  await page.locator("#locaPreset .presetopt", { hasText: "Lounge" }).click();
  await expect(page.locator("#locaIconOpts input[value=chat]")).toBeChecked();
  await page.locator("#locaDesc").fill("Talk about the week.");
  await page.locator("#locaSeats").fill("12");
  await page.locator("#locaIconOpts .identopt[title=music]").click();
  await page.locator("#locaColorOpts .identopt[title=Sky]").click();
  await page.locator("#extCreateDefault .extrow", { hasText: "Notes" }).click();
  await expect(page.locator("#extCreateTabs")).toHaveText("Tabs in this loca: Chat · Notes");
  await page.locator("#extCreateSubmit").click();
  await expect(page.locator("#extCreate")).toBeHidden();
  await expect.poll(() => page.evaluate(() => state.room)).toBe("preset-edited");
  expect(posted).toEqual([{
    name: "preset-edited", extensions: ["notes"], description: "Talk about the week.", seats: 12, icon: "music", color: "sky",
  }]);
  const settings = await serverSettings(request, "preset-edited");
  expect([settings.description, settings.seats, settings.icon, settings.color])
    .toEqual(["Talk about the week.", 12, "music", "sky"]);
  expect(await serverExtensions(request, "preset-edited")).toEqual(["notes"]);

  // Back to Loca: its fields return, what the preset does not fill stays,
  // and a reopened panel starts from Loca again.
  await openPanel(page, "preset-switched");
  await page.locator("#locaDesc").fill("Kept.");
  await page.locator("#locaPreset .presetopt", { hasText: "Lounge" }).click();
  await page.locator("#locaPreset .presetopt", { hasText: "Loca" }).first().click();
  await expect(page.locator("#locaPreset input[value=loca]")).toBeChecked();
  expect(await ticked(page)).toEqual(["notes", "journal", "wiki"]);
  await expect(page.locator("#locaSeats")).toHaveValue("7");
  await expect(page.locator("#locaIconOpts input[value='']")).toBeChecked();
  await expect(page.locator("#locaDesc")).toHaveValue("Kept.");
  await page.locator("#locaPreset .presetopt", { hasText: "Lounge" }).click();
  await page.locator("#extCreateCancel").click();
  await openPanel(page, "preset-switched");
  await expect(page.locator("#locaPreset input[value=loca]")).toBeChecked();
  expect(await ticked(page)).toEqual(["notes", "journal", "wiki"]);
  await expect(page.locator("#locaSeats")).toHaveValue("7");
  await expect(page.locator("#locaIconOpts input[value='']")).toBeChecked();
  await page.locator("#locaPreset .presetopt", { hasText: "Lounge" }).click();
  await page.locator("#extCreateCancel").click();

  // The identity panel of an existing loca has no preset, and nothing of a
  // Lounge left on the create panel.
  await page.locator("#sideLocaTab").click();
  await page.locator("#locaSummary #identityOpen").click();
  await expect(page.locator("#extCreateTitle")).toHaveText("Loca identity");
  await expect(page.locator("#locaPreset")).toBeHidden();
  await expect(page.locator("#locaDesc")).toHaveAttribute("placeholder", "One short paragraph. Agents read it when they enter.");
  await expect(page.locator("#locaIconOpts input[value=music]")).toBeChecked();
  await page.close();
});

test("the server decides: a Lounge with more seats than the ceiling is refused and nothing is opened", async ({ browser, request }) => {
  const ceiling = (await (await request.get("/health")).json()).loca_capacity_max;
  const page = await masterPage(browser, request);
  await openPanel(page, "preset-over");
  await page.locator("#locaPreset .presetopt", { hasText: "Lounge" }).click();
  await page.locator("#locaSeats").fill(String(ceiling + 1));
  await page.locator("#extCreateSubmit").click();
  await expect(page.locator("#extCreateError")).toContainText(`seats must be between 2 and ${ceiling}`);
  await expect(page.locator("#extCreate")).toBeVisible();
  await expect(page.locator("#locaSeats")).toHaveValue(String(ceiling + 1));
  expect((await request.get("/rooms/preset-over/settings", { headers: MASTER })).status()).toBe(404);
  await page.close();
});

test("an older client's plain create is unchanged: no extensions field keeps the legacy five, no identity", async ({ request }) => {
  const made = await request.post("/rooms", { headers: MASTER, data: { name: "preset-old-client" } });
  expect(made.status()).toBe(201);
  const settings = await serverSettings(request, "preset-old-client");
  for (const key of ["description", "seats", "icon", "color", "labels", "preset", "kind"]) expect(settings).not.toHaveProperty(key);
  expect(await serverExtensions(request, "preset-old-client")).toEqual(["notes", "focus", "journal", "wiki", "live"]);
});
