const { test, expect } = require("@playwright/test");

test("Master suspends, resumes and revokes a real Building membership", async ({ page, request }) => {
  const headers = { "x-admin-token": "MASTER" };
  const paired = await request.post("/pairings?ttl_hours=1", { headers });
  const { pairing_code } = await paired.json();
  const signedIn = await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name: "operator", kind: "user" },
  });
  const info = await signedIn.json();
  const admitted = await request.post("/members", { headers, data: { name: "ui-lifecycle-worker", kind: "agent" } });
  expect(admitted.ok()).toBe(true);
  await page.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "operator" }));
  }, { token: info.session_token, expiresAt: info.expires_at });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate(() => switchTab("people"));
  await expect.poll(() => page.evaluate(() => state.profile?.building_role)).toBe("master");
  const action = page.locator('[data-suspend-member][data-member-name="ui-lifecycle-worker"]');
  await expect(action).toHaveText("Suspend");
  page.once("dialog", dialog => dialog.accept());
  await action.click();
  await expect(action).toHaveText("Resume");
  page.once("dialog", dialog => dialog.accept());
  await action.click();
  await expect(action).toHaveText("Suspend");
  page.once("dialog", dialog => dialog.accept());
  await page.locator('[data-revoke-member][data-member-name="ui-lifecycle-worker"]').click();
  await expect(action).toHaveCount(0);
});

test("Building membership actions are Master-only and require confirmation", async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await page.goto("/");
  await page.evaluate(() => {
    state.people = [{ name: "fixture-agent", membership_id: "mbid_fixture", kind: "agent", locas: [], online: false }];
    state.profile = { building_role: "smaster" };
    renderPeople();
    document.querySelector("#peopleList").classList.remove("hidden");
  });
  await expect(page.locator("[data-revoke-member]")).toHaveCount(0);
  await expect(page.locator("[data-suspend-member]")).toHaveCount(0);
  await page.evaluate(() => {
    state.profile.building_role = "master";
    renderPeople();
  });
  const button = page.locator("[data-revoke-member]");
  await expect(button).toHaveCount(1);
  await expect(button).toHaveAttribute("data-revoke-member", "mbid_fixture");
  await expect(page.locator("[data-suspend-member]")).toHaveText("Suspend");
  await page.evaluate(() => { state.people[0].suspended = true; renderPeople(); });
  await expect(page.locator("[data-suspend-member]")).toHaveText("Resume");
  await expect(page.locator("#peopleList")).toContainText("Suspended");
  // Trigger the real delegated handler without requiring a room seat.
  let revoked = false;
  await page.route("**/members/mbid_fixture", route => {
    revoked = true;
    return route.fulfill({ status: 200, body: "revoked" });
  });
  page.once("dialog", dialog => dialog.dismiss());
  await button.dispatchEvent("click");
  expect(revoked).toBe(false);
  page.once("dialog", async dialog => {
    expect(dialog.message()).toContain("does not stop");
    await dialog.accept();
  });
  await button.dispatchEvent("click");
  await expect.poll(() => revoked).toBe(true);
});
