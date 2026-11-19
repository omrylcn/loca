const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

// Above the message box, one line says who a message will reach before it
// is sent: nobody named means everyone reads it and nobody is asked to
// answer; a target, an @name in the words, or a reply names who is asked.
// It only shows what the server's addressing already does.

const MASTER = { "x-admin-token": "MASTER" };
const ROOM = "address-line";

test.beforeEach(async ({ page, request }) => {
  await provisionRooms(request, [ROOM]);
  await page.addInitScript(() => { try { localStorage.setItem("loca-gs-seen", "1"); } catch (e) {} });
});

async function seated(page, request, name) {
  await request.post("/members", { headers: MASTER, data: { name, kind: "user" } });
  const { token } = await (await request.post(`/rooms/${ROOM}/invites`, { headers: MASTER, data: { name, kind: "user" } })).json();
  await page.goto("/");
  await page.locator("#name").fill(name);
  await page.locator("#roomToken").fill(token);
  await page.evaluate((room) => window.doConnect(room), ROOM);
  await expect(page.locator("#curRoom")).toHaveText(ROOM);
}

test("the line says who a message reaches, from the target, an @name or a reply", async ({ browser, request }) => {
  const stamp = Date.now().toString(36);
  const other = await browser.newPage();
  await other.addInitScript(() => { try { localStorage.setItem("loca-gs-seen", "1"); } catch (e) {} });
  await seated(other, request, `bo${stamp}`);
  const page = await browser.newPage();
  await page.addInitScript(() => { try { localStorage.setItem("loca-gs-seen", "1"); } catch (e) {} });
  await seated(page, request, `ada${stamp}`);
  const line = page.locator("#addressLine");

  await expect(line).toHaveText("To everyone · nobody is asked to answer");
  await page.locator("#msg").fill(`@bo${stamp} can you look?`);
  await expect(line).toHaveText(`→ @bo${stamp}`);
  await page.locator("#msg").fill("");
  await expect(line).toHaveText("To everyone · nobody is asked to answer");

  // Nobody is picked from a list beside the box: the @name in the words is
  // how someone is called.
  await expect(page.locator("#target")).toBeHidden();
  await page.locator("#msg").fill("@all a question for every agent");
  await expect(line).toHaveText("→ @all · every agent is asked");
  await page.locator("#msg").fill("");

  // Sending clears the words, and the line follows.
  await page.locator("#msg").fill(`@bo${stamp} thanks`);
  await page.locator("#sendBtn").click();
  await expect(page.locator("#msg")).toHaveValue("");
  await expect(line).toHaveText("To everyone · nobody is asked to answer");
  await other.close();
  await page.close();
});
