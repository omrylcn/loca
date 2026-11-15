const { test, expect } = require("@playwright/test");
const { MASTER, smasterKey, freshDoor } = require("./support/switch");

// Signed out, the sign-in form is the page. Signed in, it is gone: the
// account menu says who you are and on which server, and its one action is
// Sign out, which ends this browser's session and nothing else.

async function signInAsSmaster(page, key) {
  await page.locator("#entryKind").selectOption("smaster");
  await page.locator("#name").fill("");
  await page.locator("#roomToken").fill(key);
  await page.locator("#connectBtn").click();
  await expect(page.locator("body")).not.toHaveClass(/locked/);
  await expect.poll(() => page.evaluate(() => state.profile?.building_role)).toBe("smaster");
}

async function openAccountMenu(page) {
  await expect(page.locator("#whoami .profilemenu > summary")).toBeVisible();
  await page.locator("#whoami .profilemenu > summary").click();
  await expect(page.locator("#whoami .profilepanel")).toBeVisible();
}

async function whoamiStatus(request, token) {
  return (await request.get("/whoami", { headers: { "x-session-token": token } })).status();
}

test("a signed-out visitor sees the sign-in view, not a form behind a gear", async ({ browser }) => {
  const { context, page } = await freshDoor(browser);
  await expect(page.locator("body")).toHaveClass(/locked/);
  await expect(page.getByRole("heading", { name: "Sign in" })).toBeVisible();
  await expect(page.locator("#connBox")).toBeVisible();
  await expect(page.locator("#server")).toBeEditable();
  await expect(page.locator("#roomToken")).toBeEditable();
  await expect(page.locator("#connectBtn")).toBeVisible();
  await expect(page.locator(".main")).toBeHidden();
  await expect(page.locator("#connGear")).toHaveCount(0);
  // A first visit held no session, so none "ended".
  await expect(page.locator("#doorline")).not.toContainText("session ended");
  await expect(page.locator("#doorline")).toContainText("sign in");
  await context.close();
});

test("signing in replaces the sign-in view with the app, and nothing brings the form back", async ({ browser, request }) => {
  const key = await smasterKey(request, "siv-in");
  const { context, page } = await freshDoor(browser);
  await signInAsSmaster(page, key);
  await expect(page.locator(".main")).toBeVisible();
  await expect(page.locator("#connBox")).toBeHidden();
  await expect(page.locator("#whoami")).toBeVisible();
  // No control in the signed-in page opens a "connect again" form.
  await expect(page.locator("#connGear")).toHaveCount(0);
  await expect(page.locator('#sidebar [title="connection settings"]')).toHaveCount(0);
  await openAccountMenu(page);
  await expect(page.locator("#connBox")).toBeHidden();
  await context.close();
});

test("the account menu shows who you are and the server, and offers Sign out", async ({ browser, request, baseURL }) => {
  const key = await smasterKey(request, "siv-menu");
  const { context, page } = await freshDoor(browser);
  await signInAsSmaster(page, key);
  await openAccountMenu(page);
  const panel = page.locator("#whoami .profilepanel");
  await expect(panel).toContainText("siv-menu");
  await expect(panel).toContainText("Human");
  await expect(panel.locator(".profileconnected .rankbadge")).toHaveText("Smaster");
  await expect(page.locator("#accountServer")).toHaveText(new URL(baseURL).origin);
  await expect(panel.getByRole("button", { name: "Sign out" })).toBeVisible();
  // Leaving is asked, never done on one click.
  await panel.getByRole("button", { name: "Sign out" }).click();
  await expect(page.locator("#leaveConfirm")).toBeVisible();
  await expect(page.locator("#leaveConfirm")).toContainText("Your membership and your key stay");
  await page.locator("#leaveNo").click();
  await expect(page.locator("#leaveConfirm")).toBeHidden();
  await expect(page.locator("body")).not.toHaveClass(/locked/);
  await context.close();
});

test("a redraw of the account menu keeps it open with the Sign out confirmation still asked", async ({ browser, request }) => {
  const key = await smasterKey(request, "siv-redraw");
  const { context, page } = await freshDoor(browser);
  await signInAsSmaster(page, key);
  await openAccountMenu(page);
  await page.locator("#leaveBtn").click();
  await expect(page.locator("#leaveConfirm")).toBeVisible();
  // The profile is read again (as a lead change or a journal entry on the
  // Building view does) while the question is on screen.
  await page.evaluate(() => fetchProfile());
  await page.evaluate(() => renderProfile());
  await expect(page.locator("#whoami details.profilemenu")).toHaveJSProperty("open", true);
  await expect(page.locator("#leaveConfirm")).toBeVisible();
  await page.locator("#leaveYes").click();
  await expect(page.locator("body")).toHaveClass(/locked/);
  // The next person's menu starts closed.
  await signInAsSmaster(page, key);
  await expect(page.locator("#whoami .profilemenu > summary")).toBeVisible();
  await expect(page.locator("#whoami details.profilemenu")).toHaveJSProperty("open", false);
  await context.close();
});

test("Sign out ends this browser's session at the server, keeps the membership, and shows the sign-in view", async ({ browser, request }) => {
  const key = await smasterKey(request, "siv-out");
  const { context, page } = await freshDoor(browser);
  await signInAsSmaster(page, key);
  const session = await page.evaluate(() => state.session);
  expect(await whoamiStatus(request, session)).toBe(200);

  await openAccountMenu(page);
  await page.locator("#leaveBtn").click();
  await page.locator("#leaveYes").click();
  await expect(page.locator("body")).toHaveClass(/locked/);
  await expect(page.getByRole("heading", { name: "Sign in" })).toBeVisible();
  await expect(page.locator("#doorline")).toContainText("you signed out.");
  await expect(page.locator("#doorline")).toContainText("your key signs you in again");
  await expect(page.locator("#connectBtn")).toHaveText("sign in as Smaster");
  await expect.poll(() => whoamiStatus(request, session), { timeout: 10_000 }).toBe(401);
  expect(await page.evaluate(() => state.session)).toBeNull();

  // The membership and its key are untouched: the same key signs in again.
  await signInAsSmaster(page, key);
  await expect.poll(() => page.evaluate(() => state.profile?.principal?.display_name)).toBe("siv-out");
  await context.close();
});

test("the Master pairing entry and the keyless join request stay reachable, before and after Sign out", async ({ browser, request }) => {
  const { context, page } = await freshDoor(browser);
  await expect(page.locator("#joinRequestBtn")).toBeVisible();
  await page.locator("#masterEntryButton").click();
  await expect(page.getByLabel("Master pairing code", { exact: true })).toBeVisible();
  await expect(page.locator("#connectBtn")).toHaveText("sign in as Master");

  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  await page.locator("#pairingCode").fill(pairing_code);
  await page.locator("#connectBtn").click();
  await expect(page.locator("body")).not.toHaveClass(/locked/);
  await expect.poll(() => page.evaluate(() => state.profile?.building_role)).toBe("master");

  await openAccountMenu(page);
  await expect(page.locator("#leaveConfirm")).toBeHidden();
  await page.locator("#leaveBtn").click();
  await expect(page.locator("#leaveConfirm")).toContainText("a fresh pairing code signs you in again");
  await page.locator("#leaveYes").click();
  await expect(page.locator("body")).toHaveClass(/locked/);
  await expect(page.locator("#doorline")).toContainText("a fresh pairing code signs you in again");
  await expect(page.locator("#connectBtn")).toHaveText("sign in as Master");
  await expect(page.getByLabel("Master pairing code", { exact: true })).toBeVisible();
  await page.locator("#masterEntryButton").click();
  await page.locator("#entryKind").selectOption("human");
  await expect(page.locator("#joinRequestBtn")).toBeVisible();
  await expect(page.locator("#connectBtn")).toHaveText("sign in as Human");
  await context.close();
});

test("the account menu is not redrawn under the person using it", async ({ browser, request }) => {
  const key = await smasterKey(request, "siv-inuse");
  const { context, page } = await freshDoor(browser);
  await signInAsSmaster(page, key);
  await openAccountMenu(page);

  // A full name half typed, and the caret in it.
  const fullName = page.locator("#profileFullName");
  await fullName.click();
  await fullName.pressSequentially("Ada Lov");
  await page.evaluate(() => { document.getElementById("profileFullName").dataset.mark = "typed-here"; });

  // What follows the server (a profile read again, a plain redraw) waits.
  await page.evaluate(() => fetchProfile());
  await page.evaluate(() => renderProfile());
  await expect(fullName).toHaveValue("Ada Lov");
  await expect(fullName).toHaveAttribute("data-mark", "typed-here");
  await expect(fullName).toBeFocused();
  await page.keyboard.type("elace");
  await expect(fullName).toHaveValue("Ada Lovelace");

  // A key made here is shown once; a redraw must not take it off the screen.
  await page.locator("#whoami details.profileaccess > summary").click();
  await page.locator("#credentialLabel").fill("siv laptop");
  await page.locator("#credentialCreate").click();
  await expect(page.locator("#credentialSecret code")).toBeVisible();
  const shown = await page.locator("#credentialSecret code").textContent();
  await page.evaluate(() => fetchProfile());
  await expect(page.locator("#credentialSecret code")).toHaveText(shown);
  // The person's own action did redraw: the new key is listed.
  await expect(page.locator("#whoami .profilepanel")).toContainText("siv laptop");

  // Closing the menu lets the waiting redraw happen: nothing unsaved stays.
  await page.locator("main.main").click({ position: { x: 5, y: 5 }, force: true });
  await expect(page.locator("#whoami details.profilemenu")).not.toHaveAttribute("open", "");
  await openAccountMenu(page);
  await expect(page.locator("#credentialSecret code")).toHaveCount(0);
  await context.close();
});
