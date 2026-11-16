const { test, expect } = require("@playwright/test");

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

async function visibleTabs(page) {
  return page.locator(".tabs [role=tab]:visible").allTextContents();
}

test("the Vault tab: those who run the loca write the rules, a participant reads them live", async ({ browser, request }) => {
  const room = "vault-tab";
  const created = await request.post("/rooms", { headers: MASTER, data: { name: room, extensions: ["journal", "vault"] } });
  expect([201, 409]).toContain(created.status());

  const page = await masterPage(browser, request);
  await page.evaluate((room) => joinRoom(room), room);
  await expect.poll(() => page.evaluate(() => state.ws?.readyState)).toBe(1);
  await expect.poll(() => visibleTabs(page)).toEqual(["Chat", "Journal", "Vault"]);
  await page.locator("#tabVault").click();
  await expect(page.locator("#vaultStatus")).toHaveText("No rules yet.");
  await expect(page.locator("#vaultEdit")).toBeVisible();
  await page.locator("#vaultText").fill("# Working rules\n\n- run the checks before saying done");
  await page.locator("#vaultSave").click();
  await expect(page.locator("#vaultEditStatus")).toHaveText("Saved · revision 1");
  await expect(page.locator("#vaultBody h1")).toHaveText("Working rules");
  await expect(page.locator("#vaultStatus")).toContainText("Revision 1 · written by Master");
  const stored = await (await request.get(`/rooms/${room}/vault`, { headers: MASTER })).json();
  expect([stored.rev, stored.body]).toEqual([1, "# Working rules\n\n- run the checks before saying done"]);

  // A participant reads the rules and has no editor; the server refuses its write.
  const name = "vault-reader";
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
  await expect.poll(() => visibleTabs(reader)).toEqual(["Chat", "Journal", "Vault"]);
  await reader.locator("#tabVault").click();
  await expect(reader.locator("#vaultBody li")).toHaveText("run the checks before saying done");
  await expect(reader.locator("#vaultEdit")).toBeHidden();
  const session = await (await request.post("/sessions", {
    headers: { "x-room-token": invite.token }, data: { name, kind: "user", loca: room },
  })).json();
  expect((await request.put(`/rooms/${room}/vault`, {
    headers: { "x-session-token": session.session_token }, data: { body: "mine now", base_rev: 1 },
  })).status()).toBe(403);

  // A new revision reaches the reader's open tab through the `vault` frame.
  await page.locator("#vaultText").fill("# Working rules\n\n- ask before touching deploy/");
  await page.locator("#vaultSave").click();
  await expect(page.locator("#vaultEditStatus")).toHaveText("Saved · revision 2");
  await expect(reader.locator("#vaultBody li")).toHaveText("ask before touching deploy/");
  await expect(reader.locator("#vaultStatus")).toContainText("Revision 2");

  // Switched off, the tab closes for the reader and the server refuses the rules.
  expect((await request.delete(`/rooms/${room}/extensions/vault`, { headers: MASTER })).ok()).toBeTruthy();
  await expect.poll(() => visibleTabs(reader)).toEqual(["Chat", "Journal"]);
  await expect.poll(() => reader.evaluate(() => state.tab)).toBe("chat");
  expect((await request.get(`/rooms/${room}/vault`, { headers: { "x-session-token": session.session_token } })).status()).toBe(403);
  await reader.close();
  await page.close();
});
