const { test, expect } = require("@playwright/test");

// People: a member row's actions (Message · Suspend · Revoke) stay inside
// their own column. They used to spill left out of a 170px column and lie
// over the Location column beside them.

const MASTER = { "x-admin-token": "MASTER" };

async function masterPage(browser, request, viewport) {
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const signed = await (await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name: "pa-master", kind: "user" },
  })).json();
  const page = await browser.newPage({ viewport });
  await page.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "pa-master" }));
  }, { token: signed.session_token, expiresAt: signed.expires_at });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await expect.poll(() => page.evaluate(() => state.profile?.building_role)).toBe("master");
  return page;
}

// Every member row whose actions overflow their box or reach into Location.
function rowsWithCoveredLocation(page) {
  return page.evaluate(() => [...document.querySelectorAll("#peopleList .prow")].map(row => {
    const acts = row.querySelector(".pacts");
    const where = row.querySelector(".pwhere");
    if (!acts || !where || !acts.children.length) return null;
    const a = acts.getBoundingClientRect();
    const w = where.getBoundingClientRect();
    const overflows = acts.scrollWidth > acts.clientWidth + 1
      || [...acts.children].some(b => b.getBoundingClientRect().left < a.left - 1);
    const overlaps = [...acts.children].some(b => {
      const r = b.getBoundingClientRect();
      return r.left < w.right - 1 && r.right > w.left + 1 && r.top < w.bottom - 1 && r.bottom > w.top + 1;
    });
    return overflows || overlaps ? row.querySelector(".pnametext")?.textContent || "?" : null;
  }).filter(Boolean));
}

for (const viewport of [{ width: 1280, height: 900 }, { width: 390, height: 844 }]) {
  test(`People at ${viewport.width}: a member's three actions never cover their location`, async ({ browser, request }) => {
    for (const name of ["pa-agent-with-a-long-name", "pa-human"]) {
      const admitted = await request.post("/members", { headers: MASTER, data: { name, kind: "agent" } });
      expect([200, 201, 409]).toContain(admitted.status());
    }
    const page = await masterPage(browser, request, viewport);
    await page.evaluate(() => switchTab("people"));
    const row = page.locator(".prow", { hasText: "pa-agent-with-a-long-name" });
    await expect(row.locator(".pacts .btn")).toHaveText(["Message", "Suspend", "Revoke"]);
    expect(await rowsWithCoveredLocation(page)).toEqual([]);
    // On a desk they fit on one line, not folded under each other.
    const tops = await row.locator(".pacts .btn").evaluateAll(bs => bs.map(b => Math.round(b.getBoundingClientRect().top)));
    expect(new Set(tops).size).toBe(1);
    await page.close();
  });
}
