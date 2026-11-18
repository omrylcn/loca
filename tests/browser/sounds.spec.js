// Sounds: four short chimes, each person's own choice in Profile → Sounds.
// A headless browser cannot be heard, so every decision to play is written
// to window.__soundLog (sounds.js does so only when a test put the array
// there) and the tests read that log: which kind played, and when nothing did.
const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

const MASTER = { "x-admin-token": "MASTER" };
const ROOM = "sounds-room";
const OTHER_ROOM = "sounds-elsewhere";
// The throttle is one sound per kind every 1.5 s; wait just past it.
const PAST_THROTTLE = 1_600;
// The bell and the unread counts ride the 5 s health tick.
const TICK = { timeout: 15_000 };

let serial = 0;
const fresh = prefix => `${prefix}-${Date.now().toString(36)}${serial++}`;

async function smasterSession(request, name) {
  const made = await request.post("/smasters", { headers: MASTER, data: { name } });
  expect(made.ok(), await made.text()).toBeTruthy();
  const { token } = await made.json();
  const signedIn = await request.post("/sessions", { headers: { "x-admin-token": token }, data: { name, kind: "user" } });
  expect(signedIn.ok()).toBeTruthy();
  return signedIn.json();
}

async function openAs(browser, info, name) {
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.addInitScript(({ token, expiresAt, name }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name }));
    window.__soundLog = [];
  }, { token: info.session_token, expiresAt: info.expires_at, name });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  return page;
}

async function enter(page, room) {
  await page.evaluate(room => joinRoom(room), room);
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);
  await expect.poll(() => page.evaluate(() => state.principalId)).toBeTruthy();
}

// Loca in the background (another tab, a minimised window) or in front.
const setHidden = (page, hidden) => page.evaluate(hidden => {
  Object.defineProperty(document, "hidden", { configurable: true, get: () => hidden });
}, hidden);
const played = page => page.evaluate(() => window.__soundLog.filter(entry => !entry.preview).map(entry => entry.kind));
const say = (request, session, room, sender, text) => request.post(`/rooms/${room}/messages`, {
  headers: { "x-session-token": session }, data: { sender, sender_type: "user", text },
});

async function openSounds(page) {
  await page.locator("#whoami .profilemenu > summary").click();
  const sounds = page.locator("#profileSecSounds");
  await expect(sounds).toBeVisible();
  return sounds;
}
const toggle = (sounds, name) => sounds.getByRole("switch", { name, exact: true });
const savedSounds = page => page.waitForResponse(response =>
  response.url().endsWith("/profile/preferences") && response.request().method() === "PUT" && response.ok());

test.beforeEach(async ({ request }) => {
  await provisionRooms(request, [ROOM, OTHER_ROOM]);
});

test("the Sounds section shows its defaults, saves each change to the profile and keeps it on another device", async ({ browser, request }) => {
  const name = fresh("snd-ada");
  const info = await smasterSession(request, name);
  const page = await openAs(browser, info, name);
  await enter(page, ROOM);
  const sounds = await openSounds(page);
  await expect(sounds.getByRole("heading", { name: "Sounds" })).toBeVisible();
  await expect(page.locator('[data-profile-jump="profileSecSounds"]')).toHaveText("Sounds");

  // The defaults: on, but only for what is addressed to you, at medium, and
  // only while Loca is not the window in front.
  await expect(toggle(sounds, "Sounds")).toBeChecked();
  await expect(toggle(sounds, "Messages")).not.toBeChecked();
  await expect(toggle(sounds, "Mentions & replies to me")).toBeChecked();
  await expect(toggle(sounds, "Private messages")).toBeChecked();
  await expect(toggle(sounds, "Notifications")).toBeChecked();
  await expect(toggle(sounds, "Only when Loca is in the background")).toBeChecked();
  const volume = sounds.getByRole("radiogroup", { name: "Volume" });
  await expect(volume.getByRole("radio", { name: "Medium" })).toBeChecked();
  await expect(sounds.getByRole("button", { name: /^Play / })).toHaveCount(4);
  // Four distinct sounds, none longer than 300 ms.
  const lengths = await page.evaluate(() => ["message", "mention", "dm", "notice"].map(soundLength));
  for (const seconds of lengths) expect(seconds).toBeLessThanOrEqual(0.3);

  // Each change is saved to the profile at once.
  let put = savedSounds(page);
  await toggle(sounds, "Messages").click();
  await put;
  await expect(toggle(sounds, "Messages")).toBeChecked();
  put = savedSounds(page);
  await volume.getByRole("radio", { name: "High" }).check();
  await put;
  put = savedSounds(page);
  await toggle(sounds, "Only when Loca is in the background").click();
  await put;
  const stored = await (await request.get("/profile/preferences", { headers: { "x-session-token": info.session_token } })).json();
  expect(stored).toMatchObject({
    sound: "on", sound_message: "on", sound_mention: "on", sound_dm: "on", sound_notice: "on",
    sound_volume: "high", sound_background: "off",
  });

  // Another device knows nothing locally: the profile's copy is what shows.
  await page.evaluate(() => localStorage.removeItem("loca-sounds"));
  await page.reload();
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await enter(page, ROOM);
  const again = await openSounds(page);
  await expect(toggle(again, "Messages")).toBeChecked();
  await expect(toggle(again, "Only when Loca is in the background")).not.toBeChecked();
  await expect(again.getByRole("radiogroup", { name: "Volume" }).getByRole("radio", { name: "High" })).toBeChecked();

  // Play previews a kind whatever the switches say.
  await again.getByRole("button", { name: "Play the private message sound" }).click();
  expect(await page.evaluate(() => window.__soundLog)).toContainEqual(expect.objectContaining({ kind: "dm", preview: true }));
  await page.context().close();
});

test("a mention and a reminder to you play the mention sound; your own words, plain chatter, other people's lines and history play nothing", async ({ browser, request }) => {
  const name = fresh("snd-bea");
  const page = await openAs(browser, await smasterSession(request, name), name);
  await enter(page, ROOM);
  await setHidden(page, true);
  const me = await page.evaluate(() => state.name);

  await page.evaluate(({ me, room }) => {
    const now = Date.now();
    onFrame({ t: "msg", message: { id: 990001, room, sender: me, sender_type: "user", text: `my own line @${me}`, ts: now } });
    onFrame({ t: "msg", message: { id: 990002, room, sender: "snd-bob", sender_type: "user", text: "hello everyone", ts: now } });
    onFrame({ t: "msg", message: { id: 990003, room, sender: "loca", sender_type: "user", target: "all", kind: "announce", text: "snd-bob set @lead snd-cem.", ts: now } });
    onFrame({ t: "history", messages: [{ id: 990004, room, sender: "snd-bob", sender_type: "user", text: `@${me} said before you came`, ts: now - 60_000 }] });
    onFrame({ t: "attention", attention: { id: "snd-att-other", room, reason: "goal_reminder", owner: "snd-cem", status: "open", attempt: 1, audience: { kind: "lead" } } });
  }, { me, room: ROOM });
  expect(await played(page)).toEqual([]);

  // A real mention, said by someone else at the server, plays (b).
  const bob = fresh("snd-bob");
  const bobInfo = await smasterSession(request, bob);
  const said = await say(request, bobInfo.session_token, ROOM, bob, `@${me} have a look`);
  expect(said.status(), await said.text()).toBeLessThan(300);
  await expect.poll(() => played(page)).toEqual(["mention"]);

  // A reminder addressed to you uses the same sound.
  await page.waitForTimeout(PAST_THROTTLE);
  await page.evaluate(({ me, room }) => {
    onFrame({ t: "attention", attention: { id: "snd-att-mine", room, reason: "task_reminder", owner: me, status: "open", attempt: 1, audience: { kind: "person", name: me } } });
  }, { me, room: ROOM });
  expect(await played(page)).toEqual(["mention", "mention"]);
  await page.context().close();
});

test("only in the background by default: a mention with Loca in front is silent until the person says otherwise", async ({ browser, request }) => {
  const name = fresh("snd-can");
  const page = await openAs(browser, await smasterSession(request, name), name);
  await enter(page, ROOM);
  const me = await page.evaluate(() => state.name);
  const mention = id => page.evaluate(({ me, room, id }) => {
    onFrame({ t: "msg", message: { id, room, sender: "snd-bob", sender_type: "user", text: `@${me} look`, ts: Date.now() } });
  }, { me, room: ROOM, id });

  await setHidden(page, false);
  await mention(991001);
  expect(await played(page)).toEqual([]);
  await setHidden(page, true);
  await mention(991002);
  expect(await played(page)).toEqual(["mention"]);

  const sounds = await openSounds(page);
  const put = savedSounds(page);
  await toggle(sounds, "Only when Loca is in the background").click();
  await put;
  await setHidden(page, false);
  await page.waitForTimeout(PAST_THROTTLE);
  await mention(991003);
  expect(await played(page)).toEqual(["mention", "mention"]);
  await page.context().close();
});

test("at most one sound of a kind every 1.5 s, each kind on its own, and nothing with Sounds off", async ({ browser, request }) => {
  const name = fresh("snd-dee");
  const page = await openAs(browser, await smasterSession(request, name), name);
  await enter(page, ROOM);
  await setHidden(page, true);
  const me = await page.evaluate(() => state.name);
  const frame = (id, text) => page.evaluate(({ room, id, text }) => {
    onFrame({ t: "msg", message: { id, room, sender: "snd-bob", sender_type: "user", text, ts: Date.now() } });
  }, { room: ROOM, id, text });

  // Plain messages are off by default.
  await frame(992001, "just chatting");
  expect(await played(page)).toEqual([]);
  const sounds = await openSounds(page);
  let put = savedSounds(page);
  await toggle(sounds, "Messages").click();
  await put;

  await frame(992002, `@${me} one`);
  await frame(992003, `@${me} two`);
  await frame(992004, `@${me} three`);
  await frame(992005, "chatter, a kind of its own");
  await frame(992006, "more chatter, too soon");
  expect(await played(page)).toEqual(["mention", "message"]);
  await page.waitForTimeout(PAST_THROTTLE);
  await frame(992007, `@${me} four`);
  expect(await played(page)).toEqual(["mention", "message", "mention"]);

  put = savedSounds(page);
  await toggle(sounds, "Sounds").click();
  await put;
  await page.waitForTimeout(PAST_THROTTLE);
  await frame(992008, `@${me} five`);
  await frame(992009, "chatter");
  expect(await played(page)).toEqual(["mention", "message", "mention"]);
  await page.context().close();
});

test("a private message and a Building notification each play their own sound; a loca you only could enter stays quiet", async ({ browser, request }) => {
  const saName = fresh("snd-sa");
  const sbName = fresh("snd-sb");
  const saInfo = await smasterSession(request, saName);
  const sa = await openAs(browser, saInfo, saName);
  const sbInfo = await smasterSession(request, sbName);
  const sb = await openAs(browser, sbInfo, sbName);
  const refreshed = () => sa.evaluate(async () => {
    while (roomsRefreshing) await new Promise(r => setTimeout(r, 20));
    await refreshRooms();
  });

  // sa opens a conversation with sb, then goes back to a loca.
  await sa.evaluate(name => openDm(name), sbName);
  await expect.poll(() => sa.evaluate(() => isDm(state.room))).toBe(true);
  const dm = await sa.evaluate(() => state.room);
  await enter(sa, ROOM);
  await setHidden(sa, true);
  await refreshed();

  // sb writes in it: sa hears (c) once its unread count sees the word.
  const said = await say(request, sbInfo.session_token, dm, sbName, "psst");
  expect(said.status(), await said.text()).toBeLessThan(300);
  await refreshed();
  await expect.poll(() => played(sa)).toEqual(["dm"]);

  // A loca a Smaster may enter but has not sat in: a mention there is quiet.
  await refreshed();
  const there = await say(request, sbInfo.session_token, OTHER_ROOM, sbName, `@${saName} over here`);
  expect(there.status(), await there.text()).toBeLessThan(300);
  await refreshed();
  await refreshed();
  expect(await played(sa)).toEqual(["dm"]);
  // Once sa has sat down there, the next mention in it plays.
  await sa.evaluate(room => setSeatedHere(room, true), OTHER_ROOM);
  const seated = await say(request, sbInfo.session_token, OTHER_ROOM, sbName, `@${saName} now you are here`);
  expect(seated.status(), await seated.text()).toBeLessThan(300);
  await refreshed();
  await expect.poll(() => played(sa)).toEqual(["dm", "mention"]);

  // The Master's notification to everyone rings the bell (d). The bell's
  // first read (which is silent: nothing in it is new) is already done.
  await expect.poll(() => sa.evaluate(() => state.notif.loaded), TICK).toBe(true);
  const sent = await request.post("/notifications", { headers: MASTER, data: { title: "Sound check", body: "", audience: { all: true } } });
  expect(sent.status()).toBe(201);
  await expect.poll(() => played(sa), TICK).toEqual(["dm", "mention", "notice"]);
  await sa.context().close();
  await sb.context().close();
});
