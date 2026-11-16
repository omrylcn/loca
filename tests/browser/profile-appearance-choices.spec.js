// The appearance choices added after theme/accent/density/font_size: a font,
// four text sizes and three densities. Each one is checked by the computed
// style of a known element, not only by the attribute that carries it.
const { test, expect } = require("@playwright/test");
const { provisionRooms } = require("./support/building");

const ROOM = "appearance-choices-room";
const DEFAULTS = {
  theme: "dark", accent: "teal", density: "comfortable", font_size: "normal", font_family: "mono",
};

async function resetMasterAppearance(request) {
  const reset = await request.put("/profile/preferences", {
    headers: { "x-admin-token": "MASTER" }, data: DEFAULTS,
  });
  expect(reset.ok()).toBeTruthy();
}

async function signInAsMaster(page, request) {
  const paired = await request.post("/pairings?ttl_hours=1", {
    headers: { "x-admin-token": "MASTER" },
  });
  const { pairing_code: pairingCode } = await paired.json();
  await page.goto("/");
  await page.locator("#masterEntryButton").click();
  await page.locator("#pairingCode").fill(pairingCode);
  await page.evaluate(room => window.doConnect(room), ROOM);
  await expect(page.locator("#whoami")).toContainText("MASTER");
}

async function newPage(browser, viewport) {
  const context = await browser.newContext(viewport ? { viewport } : {});
  await context.addInitScript(() => {
    try { localStorage.setItem("loca-gs-seen", "1"); } catch (e) {}
  });
  return { context, page: await context.newPage() };
}

const html = page => page.locator("html");
const saved = page => page.waitForResponse(response =>
  response.url().endsWith("/profile/preferences") && response.request().method() === "PUT" && response.ok());

// What each choice is expected to change, read from the live page.
const look = page => page.evaluate(() => {
  const css = el => getComputedStyle(el);
  const sample = document.querySelector("#appearancePreview .body");
  return {
    bodyFont: css(document.body).fontFamily,
    composerFont: css(document.getElementById("msg")).fontFamily,
    composerSize: css(document.getElementById("msg")).fontSize,
    feedPadTop: css(document.getElementById("feed")).paddingTop,
    feedGap: css(document.getElementById("feed")).rowGap,
    sampleSize: sample ? css(sample).fontSize : null,
    sampleFont: sample ? css(sample).fontFamily : null,
  };
});

test.beforeEach(async ({ request }) => {
  await provisionRooms(request, [ROOM]);
  await resetMasterAppearance(request);
});

test.afterEach(async ({ request }) => {
  await resetMasterAppearance(request);
});

test("each new font, text size and density changes what it should, previewed in the panel", async ({ browser, request }) => {
  const { context, page } = await newPage(browser);
  await signInAsMaster(page, request);
  await expect(html(page)).toHaveAttribute("data-font-family", "mono");
  await page.locator("#whoami .profilemenu > summary").click();
  const appearance = page.locator("#profileAppearance");
  await expect(appearance.locator("#appearancePreview")).toBeVisible();

  const start = await look(page);
  expect(start.bodyFont).toContain("ui-monospace");
  expect(start.composerSize).toBe("13px");
  expect(start.sampleSize).toBe("13px");
  expect(start.feedPadTop).toBe("14px");

  const fonts = { Sans: "system-ui", Serif: "ui-serif", Mono: "ui-monospace" };
  for (const [label, face] of Object.entries(fonts)) {
    const write = saved(page);
    await appearance.getByRole("radio", { name: label, exact: true }).check();
    await write;
    const now = await look(page);
    expect(now.bodyFont, label).toContain(face);
    expect(now.composerFont, label).toContain(face);
    expect(now.sampleFont, label).toContain(face);
  }

  const sizes = { Small: "12px", Larger: "17px", Large: "15px", Normal: "13px" };
  for (const [label, px] of Object.entries(sizes)) {
    const write = saved(page);
    await appearance.getByRole("radio", { name: label, exact: true }).check();
    await write;
    const now = await look(page);
    expect(now.composerSize, label).toBe(px);
    expect(now.sampleSize, label).toBe(px);
  }

  const densities = { Spacious: ["20px", "12px"], Compact: ["8px", "3px"], Comfortable: ["14px", "7px"] };
  for (const [label, [top, gap]] of Object.entries(densities)) {
    const write = saved(page);
    await appearance.getByRole("radio", { name: label, exact: true }).check();
    await write;
    const now = await look(page);
    expect(now.feedPadTop, label).toBe(top);
    expect(now.feedGap, label).toBe(gap);
  }

  // Keyboard: arrows move within a radio group and the choice applies.
  const write = saved(page);
  await appearance.getByRole("radio", { name: "Mono", exact: true }).focus();
  await page.keyboard.press("ArrowRight");
  await expect(appearance.getByRole("radio", { name: "Sans", exact: true })).toBeChecked();
  await expect(html(page)).toHaveAttribute("data-font-family", "sans");
  await write;

  // Commands and paths stay monospace whatever the face.
  const commandFont = await page.evaluate(() => {
    const probe = document.createElement("div");
    probe.className = "gspath";
    document.body.appendChild(probe);
    const font = getComputedStyle(probe).fontFamily;
    probe.remove();
    return font;
  });
  expect(commandFont).toContain("ui-monospace");
  await context.close();
});

test("the new choices persist across reload, paint from the cache first, and follow the person to another browser", async ({ browser, request }) => {
  const first = await newPage(browser);
  await signInAsMaster(first.page, request);
  await first.page.locator("#whoami .profilemenu > summary").click();
  const appearance = first.page.locator("#profileAppearance");
  for (const label of ["Serif", "Larger", "Spacious"]) {
    const write = saved(first.page);
    await appearance.getByRole("radio", { name: label, exact: true }).check();
    await write;
  }
  const stored = await request.get("/profile/preferences", { headers: { "x-admin-token": "MASTER" } });
  expect(await stored.json()).toEqual({
    theme: "dark", accent: "teal", density: "spacious", font_size: "larger", font_family: "serif",
    // The reading choices added since, at their defaults.
    accent_custom: "", layout: "bubbles", time_format: "auto", measure: "normal", own_tint: "on", motion: "system",
  });

  // Reload with the server copy held back: first paint uses the cache.
  let releaseProfile;
  const profileGate = new Promise(resolve => { releaseProfile = resolve; });
  await first.page.route("**/profile/preferences", async route => {
    await profileGate;
    await route.continue();
  });
  await first.page.reload();
  await expect(html(first.page)).toHaveAttribute("data-font-family", "serif");
  await expect(html(first.page)).toHaveAttribute("data-font-size", "larger");
  await expect(html(first.page)).toHaveAttribute("data-density", "spacious");
  const beforeServer = await look(first.page);
  expect(beforeServer.bodyFont).toContain("ui-serif");
  expect(beforeServer.composerSize).toBe("17px");
  expect(beforeServer.feedPadTop).toBe("20px");
  const profileResponse = first.page.waitForResponse(response =>
    response.url().endsWith("/profile/preferences") && response.request().method() === "GET");
  releaseProfile();
  await profileResponse;
  await first.page.unroute("**/profile/preferences");
  await expect(first.page.locator("#whoami")).toContainText("MASTER");
  await first.page.locator("#whoami .profilemenu > summary").click();
  await expect(first.page.locator("#profileAppearance").getByRole("radio", { name: "Serif", exact: true })).toBeChecked();
  await expect(first.page.locator("#profileAppearance").getByRole("radio", { name: "Larger", exact: true })).toBeChecked();
  await expect(first.page.locator("#profileAppearance").getByRole("radio", { name: "Spacious", exact: true })).toBeChecked();

  // A second browser starts from the defaults and adopts the saved look.
  const second = await newPage(browser);
  await second.page.goto("/");
  await expect(html(second.page)).toHaveAttribute("data-font-family", "mono");
  await signInAsMaster(second.page, request);
  await expect(html(second.page)).toHaveAttribute("data-font-family", "serif");
  await expect(html(second.page)).toHaveAttribute("data-font-size", "larger");
  await expect(html(second.page)).toHaveAttribute("data-density", "spacious");
  const there = await look(second.page);
  expect(there.bodyFont).toContain("ui-serif");
  expect(there.composerSize).toBe("17px");
  expect(there.feedPadTop).toBe("20px");

  await first.context.close();
  await second.context.close();
});

test("an old cached look and an old stored row keep their meaning", async ({ browser, request }) => {
  // The four-key shape an older client cached and an older server stored.
  const old = { theme: "light", accent: "violet", density: "compact", font_size: "large" };
  const put = await request.put("/profile/preferences", { headers: { "x-admin-token": "MASTER" }, data: old });
  expect(put.ok()).toBeTruthy();
  expect(await put.json()).toEqual({ ...old, font_family: "mono", accent_custom: "", layout: "bubbles", time_format: "auto", measure: "normal", own_tint: "on", motion: "system" });

  const { context, page } = await newPage(browser);
  await context.addInitScript(cached => {
    try { localStorage.setItem("loca-appearance", JSON.stringify(cached)); } catch (e) {}
  }, old);
  await page.goto("/");
  await expect(html(page)).toHaveAttribute("data-theme", "light");
  await expect(html(page)).toHaveAttribute("data-density", "compact");
  await expect(html(page)).toHaveAttribute("data-font-size", "large");
  await expect(html(page)).toHaveAttribute("data-font-family", "mono");
  await signInAsMaster(page, request);
  const now = await look(page);
  expect(now.composerSize).toBe("15px");
  expect(now.feedPadTop).toBe("8px");
  expect(now.bodyFont).toContain("ui-monospace");
  await context.close();
});

test("the appearance block fits a phone and never scrolls sideways", async ({ browser, request }) => {
  const { context, page } = await newPage(browser, { width: 390, height: 844 });
  await signInAsMaster(page, request);
  await page.evaluate(() => setMobileSidebar(true));
  await page.locator("#whoami .profilemenu > summary").click();
  const appearance = page.locator("#profileAppearance");
  await expect(appearance.locator("#appearancePreview")).toBeVisible();
  for (const label of ["Mono", "Sans", "Serif", "Small", "Larger", "Compact", "Spacious"]) {
    await expect(appearance.getByRole("radio", { name: label, exact: true })).toBeVisible();
  }
  // A tap on the chip (the radio covers it) chooses it.
  const write = saved(page);
  await appearance.getByRole("radio", { name: "Spacious", exact: true }).click();
  await write;
  await expect(html(page)).toHaveAttribute("data-density", "spacious");
  const fits = await page.evaluate(() => {
    const panel = document.getElementById("profileAppearance");
    return {
      page: document.documentElement.scrollWidth - window.innerWidth,
      panel: panel.scrollWidth - panel.clientWidth,
      right: panel.getBoundingClientRect().right - window.innerWidth,
    };
  });
  expect(fits.page).toBeLessThanOrEqual(0);
  expect(fits.panel).toBeLessThanOrEqual(0);
  expect(fits.right).toBeLessThanOrEqual(0);
  await context.close();
});
