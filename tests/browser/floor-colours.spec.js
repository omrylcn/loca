const { test, expect } = require("@playwright/test");

// Each named floor wears one of six palette colours, chosen by its id, on
// its sidebar heading, its card in Building › Floors and people's floor
// chips. The Ground floor stays neutral. Every palette colour keeps 4.5:1
// against the page and panels of every theme.

const MASTER = { "x-admin-token": "MASTER" };

const THEMES = [
  { theme: "dark", scheme: "dark" }, { theme: "light", scheme: "light" },
  { theme: "ember", scheme: "dark" }, { theme: "contrast", scheme: "dark" },
  { theme: "night", scheme: "dark" }, { theme: "paper", scheme: "light" },
];

test("named floors carry a colour by id in the sidebar, the Floors tab and People; the Ground floor stays neutral", async ({ browser, request }) => {
  const tag = Date.now().toString(36);
  const { pairing_code } = await (await request.post("/pairings?ttl_hours=1", { headers: MASTER })).json();
  const signed = await (await request.post("/sessions", {
    headers: { "x-pairing-code": pairing_code }, data: { name: "fcol-master", kind: "user" },
  })).json();
  const as = { "x-session-token": signed.session_token };
  const floors = [];
  for (const name of [`fcol-a-${tag}`, `fcol-b-${tag}`]) {
    const floor = await (await request.post("/groups", { headers: as, data: { name } })).json();
    expect(floor.id, name).toBeTruthy();
    const made = await request.post("/rooms", { headers: as, data: { name: `${name}-loca`, group: floor.id } });
    expect(made.ok(), await made.text()).toBeTruthy();
    floors.push({ ...floor, name });
  }
  const ground = await request.post("/rooms", { headers: as, data: { name: `fcol-ground-${tag}` } });
  expect(ground.ok(), await ground.text()).toBeTruthy();
  // Somebody on the first floor, for People's floor chips.
  const member = `fcol-member-${tag}`;
  const admitted = await request.post("/members", { headers: MASTER, data: { name: member, kind: "agent" } });
  expect(admitted.ok(), await admitted.text()).toBeTruthy();

  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  await page.addInitScript(({ token, expiresAt }) => {
    localStorage.setItem("loca-gs-seen", "1");
    localStorage.setItem("loca-admin-session", JSON.stringify({ token, expiresAt, name: "fcol-master" }));
  }, { token: signed.session_token, expiresAt: signed.expires_at });
  await page.goto("/");
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate(() => refreshRooms());
  const memberId = await page.evaluate(async (member) => {
    const list = await (await fetch("/profiles", { headers: adminHeaders({}) })).json();
    return list.find(p => p.display_name === member)?.id;
  }, member);
  expect(memberId).toBeTruthy();
  const added = await request.put(`/groups/${floors[0].id}/members/${memberId}`, { headers: as });
  expect(added.ok(), await added.text()).toBeTruthy();

  const colourOf = (locator) => locator.evaluate(el => getComputedStyle(el).color);
  const muted = await page.evaluate(() => getComputedStyle(document.querySelector("#roomList summary.rgrouphead")).color);
  const heading = (name) => page.locator("#roomList details.floor > summary.rgrouphead", { hasText: name }).locator(".rgroupname");

  // Sidebar: each named floor has a palette colour; the Ground floor none.
  const groundName = heading("Ground floor");
  await expect(groundName).toBeVisible();
  expect(await groundName.evaluate(el => getComputedStyle(el.closest("summary")).getPropertyValue("--floor").trim())).toBe("");
  expect(await colourOf(groundName)).toBe(muted);
  const palette = await page.evaluate(() => [1, 2, 3, 4, 5, 6].map(i => {
    const probe = document.createElement("span");
    probe.style.color = `var(--floor-${i})`;
    document.body.appendChild(probe);
    const colour = getComputedStyle(probe).color;
    probe.remove();
    return colour;
  }));
  expect(new Set(palette).size).toBe(6);
  const sidebarColour = [];
  for (const floor of floors) {
    const colour = await colourOf(heading(floor.name));
    expect(palette, floor.name).toContain(colour);
    sidebarColour.push(colour);
  }

  // The same colour on the floor's card and on People's floor chips.
  await page.evaluate(() => switchTab("people"));
  await page.locator("#btabGroups").click();
  for (const [i, floor] of floors.entries()) {
    const card = page.locator("#groupsPane .grpcard", { hasText: floor.name });
    await expect(card).toBeVisible();
    expect(await colourOf(card.locator(".grpname b"))).toBe(sidebarColour[i]);
  }
  await page.locator("#btabPeople").click();
  const chip = page.locator("#peopleList .prow", { hasText: member }).locator(".grpchip", { hasText: floors[0].name });
  await expect(chip).toBeVisible();
  expect(await colourOf(chip)).toBe(sidebarColour[0]);

  // Stable: after a reload each floor has the colour it had.
  await page.reload();
  await expect.poll(() => page.evaluate(() => state.authStatus)).toBe("authenticated");
  await page.evaluate(() => refreshRooms());
  for (const [i, floor] of floors.entries()) {
    await expect(heading(floor.name)).toBeVisible();
    expect(await colourOf(heading(floor.name))).toBe(sidebarColour[i]);
  }

  // Every palette colour reads at 4.5:1 on each theme's page and panels.
  const weak = await page.evaluate((themes) => {
    const rgb = (css) => css.match(/[\d.]+/g).slice(0, 3).map(Number);
    const lum = ([r, g, b]) => {
      const f = (c) => { c /= 255; return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4; };
      return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
    };
    const ratio = (a, b) => { const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p); return (x + 0.05) / (y + 0.05); };
    const root = document.documentElement;
    const resolve = (token) => {
      const probe = document.createElement("span");
      probe.style.color = `var(${token})`;
      document.body.appendChild(probe);
      const colour = rgb(getComputedStyle(probe).color);
      probe.remove();
      return colour;
    };
    const found = [];
    for (const { theme, scheme } of themes) {
      root.dataset.theme = theme;
      root.dataset.scheme = scheme;
      const surfaces = ["--bg", "--panel", "--panel2"].map(resolve);
      for (let i = 1; i <= 6; i++) {
        const ink = resolve(`--floor-${i}`);
        const worst = Math.min(...surfaces.map(s => ratio(ink, s)));
        if (worst < 4.5) found.push(`${theme} --floor-${i} ${worst.toFixed(2)}`);
      }
    }
    return found;
  }, THEMES);
  expect(weak).toEqual([]);
  await page.close();
});
