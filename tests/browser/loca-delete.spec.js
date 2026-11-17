const { test, expect } = require("@playwright/test");
const fs = require("node:fs");
const { provisionRooms } = require("./support/building");

// The Master deletes a closed loca from its properties, by the real buttons:
// Delete…, the loca's name typed, then one of two choices. "Download zip,
// then delete" saves the zip first and deletes only once it arrived;
// "Delete" deletes with no zip. Seal (beside it) keeps every record.

const MASTER = { "x-admin-token": "MASTER" };

async function masterSession(request, name) {
  const paired = await request.post("/pairings?ttl_hours=1", { headers: MASTER });
  const { pairing_code } = await paired.json();
  const signedIn = await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name, kind: "user" },
  });
  expect(signedIn.ok()).toBe(true);
  return signedIn.json();
}

async function openClosedLoca(page, request, room) {
  await provisionRooms(request, [room]);
  const info = await masterSession(request, `boss-${room}`);
  const said = await request.post(`/rooms/${room}/messages`, {
    headers: { "x-session-token": info.session_token },
    data: { sender: `boss-${room}`, sender_type: "user", text: `words of ${room}` },
  });
  expect(said.ok(), await said.text()).toBeTruthy();
  await page.addInitScript(({ token, expiresAt, name }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name }));
  }, { token: info.session_token, expiresAt: info.expires_at, name: `boss-${room}` });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate((room) => joinRoom(room), room);
  await expect.poll(() => page.evaluate(() => state.ws && state.ws.readyState)).toBe(1);

  await page.locator("#adminToggle").click();
  // An open loca offers no delete: it is closed first.
  await expect(page.locator("#closeLocaBtn")).toBeVisible();
  await expect(page.locator("#deleteLocaBtn")).toBeHidden();
  page.once("dialog", dialog => dialog.accept());
  await page.locator("#closeLocaBtn").click();
  await expect(page.locator("#deleteLocaBtn")).toBeVisible();
  await expect(page.locator("#deleteLocaBtn")).toBeEnabled();
  await expect(page.locator("#sealLocaBtn")).toBeVisible();
}

async function gone(request, room) {
  const listed = await (await request.get("/rooms", { headers: MASTER })).json();
  const names = listed.map(r => r.room ?? r.name ?? r);
  const history = await request.get(`/rooms/${room}/messages`, { headers: MASTER });
  return !names.includes(room) && history.status() !== 200;
}

test("the Master downloads a closed loca's zip, then it is deleted", async ({ page, request }) => {
  const room = `del-zip-${Date.now().toString(36)}`;
  await openClosedLoca(page, request, room);

  await page.locator("#deleteLocaBtn").click();
  await expect(page.locator("#locaDelete")).toBeVisible();
  await expect(page.locator("#locaDeleteRoom")).toHaveText(room);
  await expect(page.locator("#locaDeleteArchive")).toBeDisabled();
  await expect(page.locator("#locaDeleteOnly")).toBeDisabled();
  await page.locator("#locaDeleteName").fill(`${room}-x`);
  await expect(page.locator("#locaDeleteArchive")).toBeDisabled();
  await page.locator("#locaDeleteName").fill(room);
  await expect(page.locator("#locaDeleteArchive")).toBeEnabled();

  const saved = page.waitForEvent("download");
  await page.locator("#locaDeleteArchive").click();
  const download = await saved;
  expect(download.suggestedFilename()).toBe(`loca-${room}-archive.zip`);
  const zip = fs.readFileSync(await download.path());
  expect(zip.subarray(0, 2).toString()).toBe("PK");
  // Entry names sit uncompressed in a zip's directory.
  for (const entry of ["transcript.txt", "manifest.json", "data/messages.json"]) {
    expect(zip.includes(Buffer.from(entry)), entry).toBe(true);
  }

  await expect(page.locator("#locaDelete")).toBeHidden();
  await expect.poll(() => gone(request, room)).toBe(true);
});

test("Delete without a zip deletes the closed loca and saves nothing", async ({ page, request }) => {
  const room = `del-only-${Date.now().toString(36)}`;
  await openClosedLoca(page, request, room);
  let downloads = 0;
  page.on("download", () => { downloads += 1; });

  await page.locator("#deleteLocaBtn").click();
  await page.locator("#locaDeleteName").fill(room);
  await page.locator("#locaDeleteOnly").click();

  await expect(page.locator("#locaDelete")).toBeHidden();
  await expect.poll(() => gone(request, room)).toBe(true);
  expect(downloads).toBe(0);
});

test("Cancel deletes nothing", async ({ page, request }) => {
  const room = `del-cancel-${Date.now().toString(36)}`;
  await openClosedLoca(page, request, room);
  await page.locator("#deleteLocaBtn").click();
  await page.locator("#locaDeleteName").fill(room);
  await page.locator("#locaDeleteCancel").click();
  await expect(page.locator("#locaDelete")).toBeHidden();
  const history = await request.get(`/rooms/${room}/messages`, { headers: MASTER });
  expect(history.status()).toBe(200);
  expect(JSON.stringify(await history.json())).toContain(`words of ${room}`);
});
