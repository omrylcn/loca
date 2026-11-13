const { test, expect } = require("@playwright/test");

const MASTER = { "x-admin-token": "MASTER" };
// The bell rides the 5 s health poll: allow a couple of ticks.
const TICK = { timeout: 15_000 };

async function rankSession(request, name) {
  if (name === "MASTER") {
    const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
    return (await request.post("/sessions", { headers: { "x-pairing-code": pairing_code }, data: { name: "", kind: "user" } })).json();
  }
  const { token } = await (await request.post("/smasters", { headers: MASTER, data: { name } })).json();
  return (await request.post("/sessions", { headers: { "x-admin-token": token }, data: { name, kind: "user" } })).json();
}

async function openAs(browser, info, name) {
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.addInitScript(({ token, expiresAt, name }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name }));
  }, { token: info.session_token, expiresAt: info.expires_at, name });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await expect(page.locator("#notifBell")).toBeVisible(TICK);
  return page;
}

/// A plain Building member signs in through the door with its own key.
async function openMember(browser, request, name) {
  const admitted = await request.post("/members", { headers: MASTER, data: { name, kind: "user" } });
  expect(admitted.ok()).toBeTruthy();
  const { token } = await admitted.json();
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await page.goto("/");
  await page.locator("#name").fill(name);
  await page.locator("#roomToken").fill(token);
  await page.locator("#connectBtn").click();
  await expect(page.locator("#notifBell")).toBeVisible(TICK);
  return { page, token };
}

async function notify(request, headers, title, audience, body = "") {
  const r = await request.post("/notifications", { headers, data: { title, body, audience } });
  expect(r.status()).toBe(201);
  return r.json();
}

test("a member's bell shows the Master's notification; the panel lists it and reading clears the badge", async ({ browser, request }) => {
  const { page } = await openMember(browser, request, "ntf-ali");
  await expect(page.locator("#notifBadge")).toBeHidden();

  await notify(request, MASTER, "Water off at noon", { all: true }, "Line one\n<b>not bold</b>");
  await expect(page.locator("#notifBadge")).toHaveText("1", TICK);
  await expect(page.locator("#notifBell")).toHaveAttribute("aria-label", "Notifications, 1 unread");
  // A toast says so at once, and can be dismissed.
  const toast = page.locator("#notifToast");
  await expect(toast).toBeVisible();
  await expect(toast).toContainText("Water off at noon");
  await toast.locator("[data-notif-toast-close]").click();
  await expect(toast).toBeHidden();

  await page.locator("#notifBell").click();
  const panel = page.locator("#notifPanel");
  await expect(panel).toBeVisible();
  const item = panel.locator(".notifitem", { hasText: "Water off at noon" });
  await expect(item).toHaveClass(/unread/);
  await expect(item.locator(".notifmeta")).toContainText("Master");
  await expect(item.locator(".notifmeta")).toContainText("to everyone");
  // Plain text: the markup is shown, never rendered.
  await expect(item.locator(".notifbody")).toContainText("<b>not bold</b>");
  await expect(item.locator(".notifbody b")).toHaveCount(0);
  // A member never gets the composer.
  await expect(panel.locator(".notifcompose")).toHaveCount(0);

  await item.locator("[data-notif-read]").click();
  await expect(page.locator("#notifBadge")).toBeHidden();
  await expect(item).not.toHaveClass(/unread/);

  // Two more, then Mark all read.
  await notify(request, MASTER, "Second", { all: true });
  await notify(request, MASTER, "Third", { all: true });
  await expect(page.locator("#notifBadge")).toHaveText("2", TICK);
  await expect(panel.locator(".notifitem.unread")).toHaveCount(2);
  await panel.locator("[data-notif-readall]").click();
  await expect(page.locator("#notifBadge")).toBeHidden();
  await expect(panel.locator(".notifitem.unread")).toHaveCount(0);
  // Read state is the server's: a reload keeps it.
  await page.keyboard.press("Escape");
  await expect(panel).toBeHidden();
  await page.context().close();
});

test("a Smaster's composer offers only its own groups; the Master's offers everyone", async ({ browser, request }) => {
  const kaanInfo = await rankSession(request, "ntf-kaan");
  const otherInfo = await rankSession(request, "ntf-other");
  const loneInfo = await rankSession(request, "ntf-lone");
  const mk = async (session, name) => (await request.post("/groups", {
    headers: { "x-session-token": session }, data: { name },
  })).json();
  const team = await mk(kaanInfo.session_token, "ntf-team");
  await mk(otherInfo.session_token, "ntf-design");
  const member = await (await request.post("/members", { headers: MASTER, data: { name: "ntf-mem", kind: "user" } })).json();
  const profiles = await (await request.get("/profiles", { headers: MASTER })).json();
  const memId = profiles.find(p => p.display_name === "ntf-mem").id;
  expect((await request.put(`/groups/${team.id}/members/${memId}`, { headers: { "x-session-token": kaanInfo.session_token } })).ok()).toBeTruthy();
  expect(member.token).toBeTruthy();

  const kaan = await openAs(browser, kaanInfo, "ntf-kaan");
  await kaan.locator("#notifBell").click();
  const panel = kaan.locator("#notifPanel");
  await expect(panel.locator(".notifcompose summary")).toHaveText("New notification");
  await panel.locator(".notifcompose summary").click();
  const auds = panel.locator(".notifaud");
  await expect(auds).toHaveCount(1);
  await expect(auds.first()).toContainText("ntf-team");
  await expect(panel.locator(".notifauds")).not.toContainText("ntf-design");
  await expect(panel.locator("input[name=all]")).toHaveCount(0);

  // Inline errors, then a send.
  await panel.locator("#notifSend").click();
  await expect(panel.locator("#notifErr")).toHaveText("Give it a title.");
  await panel.locator("#notifTitle").fill("Deploy friday");
  await panel.locator("#notifSend").click();
  await expect(panel.locator("#notifErr")).toHaveText("Choose who receives it.");
  await auds.first().locator("input").check();
  await panel.locator("#notifText").fill("Freeze at 15:00.");
  await panel.locator("#notifSend").click();
  await expect(panel.locator(".notifstatus")).toHaveText("Sent to 1 person.");
  await expect(panel.locator(".notifitem", { hasText: "Deploy friday" })).toContainText("0 of 1 read");
  await kaan.context().close();

  // A Smaster with no group gets no composer at all.
  const lone = await openAs(browser, loneInfo, "ntf-lone");
  await lone.locator("#notifBell").click();
  await expect(lone.locator("#notifPanel .notifhead")).toBeVisible();
  await expect.poll(() => lone.evaluate(() => state.notif.audiences && state.notif.audiences.can_send)).toBe(false);
  await expect(lone.locator("#notifPanel .notifcompose")).toHaveCount(0);
  await lone.context().close();

  // The Master: everyone, and every group (disabled while Everyone is on).
  const master = await openAs(browser, await rankSession(request, "MASTER"), "Master");
  await master.locator("#notifBell").click();
  const mpanel = master.locator("#notifPanel");
  await mpanel.locator(".notifcompose summary").click();
  await expect(mpanel.locator("input[name=all]")).toBeChecked();
  await expect(mpanel.locator(".notifaud", { hasText: "ntf-team" }).locator("input")).toBeDisabled();
  await expect(mpanel.locator(".notifauds")).toContainText("ntf-design");
  await master.context().close();
});
