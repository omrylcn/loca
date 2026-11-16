const { test, expect } = require("@playwright/test");

// The name field of the "Open a new loca" panel. A new loca's name is ASCII
// letters, digits, ".", "_" and "-", starting with a letter or digit (the
// server's rule, crates/server/src/hub/loca_name.rs). A typed space becomes
// "-" as it is typed; any other character the server would refuse stays in
// the field and is named in a hint before Create, never dropped.

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

async function openPanel(page) {
  await page.locator("#newRoomBtn").click();
  await expect(page.locator("#extCreate")).toBeVisible();
  await expect(page.locator("#extCreateDefault input")).toHaveCount(4);
}

test("a space typed in the name becomes '-' as it is typed, and the loca opens under that name", async ({ browser, request }) => {
  const page = await masterPage(browser, request);
  const posted = [];
  page.on("request", req => {
    if (req.method() === "POST" && new URL(req.url()).pathname === "/rooms") posted.push(req.postDataJSON());
  });
  await openPanel(page);
  const name = page.locator("#extCreateName");
  await name.click();
  await name.pressSequentially("names team room");
  await expect(name).toHaveValue("names-team-room");
  // A space in the middle of the text, typed with the caret there, too.
  await name.press("Home");
  await name.press("ArrowRight");
  await name.press("Space");
  await expect(name).toHaveValue("n-ames-team-room");
  await name.fill("");
  await name.pressSequentially("names team room");
  // Spaces are not refused, so no refusal hint; the note says where a
  // display name with spaces goes.
  await expect(page.locator("#extCreateNameHint")).toBeVisible();
  await expect(page.locator("#extCreateNameHint")).not.toHaveClass(/bad/);
  await expect(page.locator("#extCreateNameHint")).toContainText("title");
  await page.locator("#extCreateSubmit").click();
  await expect(page.locator("#extCreate")).toBeHidden();
  await expect.poll(() => page.evaluate(() => state.room)).toBe("names-team-room");
  expect(posted.map(body => body.name)).toEqual(["names-team-room"]);
  await page.close();
});

test("a name typed in the sidebar box carries over with its spaces turned into '-'", async ({ browser, request }) => {
  const page = await masterPage(browser, request);
  await page.locator("#newRoomName").fill("names side bar");
  await openPanel(page);
  await expect(page.locator("#extCreateName")).toHaveValue("names-side-bar");
  await page.close();
});

test("a character the server would refuse stays in the field and is named in a hint before Create", async ({ browser, request }) => {
  const page = await masterPage(browser, request);
  await openPanel(page);
  const name = page.locator("#extCreateName");
  const hint = page.locator("#extCreateNameHint");
  await name.click();
  await name.pressSequentially("names#çay");
  // Kept, not dropped.
  await expect(name).toHaveValue("names#çay");
  await expect(hint).toBeVisible();
  await expect(hint).toHaveClass(/bad/);
  await expect(hint).toContainText("#");
  await expect(hint).toContainText("ç");
  await expect(hint).toContainText("title");
  // A name that starts with "-" is refused too.
  await name.fill("");
  await name.pressSequentially("-names");
  await expect(hint).toHaveClass(/bad/);
  // Fixing it clears the hint.
  await name.fill("");
  await name.pressSequentially("names-fixed");
  await expect(hint).toBeHidden();
  // Create anyway with a bad character: the server's answer is shown and
  // the panel stays open with the name as typed.
  await name.fill("names@bad");
  await expect(hint).toHaveClass(/bad/);
  await page.locator("#extCreateSubmit").click();
  await expect(page.locator("#extCreateError")).toContainText("title");
  await expect(page.locator("#extCreate")).toBeVisible();
  await expect(name).toHaveValue("names@bad");
  await page.close();
});

test("Loca identity shows an old name as it is: the name field is not rewritten and has no hint", async ({ browser, request }) => {
  // An old loca whose name has a space cannot be opened any more; the
  // identity panel of a valid one must still show its id untouched.
  const created = await request.post("/rooms", { headers: MASTER, data: { name: "names-identity" } });
  expect(created.status()).toBe(201);
  const page = await masterPage(browser, request);
  await page.evaluate(() => joinRoom("names-identity"));
  await expect.poll(() => page.evaluate(() => state.room)).toBe("names-identity");
  await page.evaluate(() => openLocaIdentity());
  await expect(page.locator("#extCreate")).toBeVisible();
  await expect(page.locator("#extCreateName")).toHaveValue("names-identity");
  await expect(page.locator("#extCreateNameHint")).toBeHidden();
  await page.close();
});
