// One tab, two people in turn: helpers for tests that let the first person
// start something, switch to the second, and then let the first one's
// unfinished work come due.
const { expect } = require("@playwright/test");

const MASTER = { "x-admin-token": "MASTER" };

async function smasterKey(request, name) {
  const { token } = await (await request.post("/smasters", { headers: MASTER, data: { name } })).json();
  return token;
}

// A fresh browser at the door, nobody signed in. Dialogs are recorded and
// dismissed; a test that means to say yes to the next one sets answer.yes.
async function freshDoor(browser) {
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  const dialogs = [];
  const answer = { yes: false };
  page.on("dialog", dialog => {
    dialogs.push(dialog.message());
    const yes = answer.yes;
    answer.yes = false;
    (yes ? dialog.accept() : dialog.dismiss()).catch(() => {});
  });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("unauthenticated");
  return { context, page, dialogs, answer };
}

async function signInWithKey(page, key) {
  if (await page.locator("#connBox").isHidden()) await page.locator("#connGear").click();
  await page.locator("#entryKind").selectOption("smaster");
  // A name typed at a Smaster's door is the name it asks to be called.
  await page.locator("#name").fill("");
  await page.locator("#roomToken").fill(key);
  await page.locator("#connectBtn").click();
}

async function expectSignedInAs(page, name) {
  await expect(page.locator("body")).not.toHaveClass(/locked/);
  await expect.poll(() => page.evaluate(() => state.profile?.principal?.display_name)).toBe(name);
  await expect.poll(() => page.evaluate(() => state.profile?.building_role)).toBe("smaster");
}

async function signOut(page) {
  if (await page.locator("#connBox").isHidden()) await page.locator("#connGear").click();
  await page.locator("#leaveBtn").click();
  await page.locator("#leaveYes").click();
  await expect(page.locator("#doorline")).toContainText("you left loca.");
}

// The first person leaves and the second signs in; resolves with the second
// person's identity as the page holds it.
async function switchTo(page, key, name) {
  await signOut(page);
  await signInWithKey(page, key);
  await expectSignedInAs(page, name);
  const mine = await page.evaluate(() => ({ session: state.session, name: state.name }));
  expect(mine.name).toBe(name);
  expect(mine.session).toBeTruthy();
  return mine;
}

async function settle(page, ms = 800) {
  await page.evaluate(wait => new Promise(resolve => setTimeout(resolve, wait)), ms);
}

// The page still belongs, whole, to the person who signed in second.
async function expectStill(page, mine) {
  const now = await page.evaluate(() => ({
    session: state.session, name: state.name, auth: state.authStatus, admin: state.adminSession,
    locked: document.body.classList.contains("locked"),
    nameField: document.getElementById("name").value,
    seat: JSON.parse(localStorage.getItem("loca-seat") || "null")?.name,
    profile: state.profile?.principal?.display_name,
  }));
  expect(now).toEqual({
    session: mine.session, name: mine.name, auth: "authenticated", admin: true, locked: false,
    nameField: mine.name, seat: mine.name, profile: mine.name,
  });
  expect(await page.evaluate(async () => (await fetch("/profile", { headers: adminHeaders({}) })).status)).toBe(200);
}

module.exports = { MASTER, smasterKey, freshDoor, signInWithKey, expectSignedInAs, signOut, switchTo, settle, expectStill };
