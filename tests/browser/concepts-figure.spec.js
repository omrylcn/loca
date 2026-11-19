const { test, expect } = require("@playwright/test");

// The map of the Building (Building → Floor → Loca, who is who) in Getting
// Started. It is docs/concepts-at-a-glance.svg drawn inline, so it takes the
// page's theme, accent and font; test_concepts_figure.py keeps the two copies
// the same. Here: it is where a newcomer looks, it has a text alternative, it
// fits a phone, and its words can be read in both themes.

const MAP = "#gsMap svg.locamap";

function contrast(a, b) {
  const lum = ([r, g, b]) => {
    const c = [r, g, b].map(v => {
      const s = v / 255;
      return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
    });
    return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
  };
  const [hi, lo] = [lum(a), lum(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

// Every text colour on the map and the colour of what it is drawn on (the
// boxes are outlines, so every word sits on the map's own background).
async function mapColours(page) {
  return page.evaluate(sel => {
    const svg = document.querySelector(sel);
    const rgb = value => (value.match(/\d+(\.\d+)?/g) || []).slice(0, 3).map(Number);
    const bg = rgb(getComputedStyle(svg.querySelector(".lm-bg")).fill);
    const texts = [...svg.querySelectorAll("text, tspan")].map(el => ({
      text: el.textContent,
      fill: rgb(getComputedStyle(el).fill),
    }));
    const strokes = [...svg.querySelectorAll(".lm-box, .lm-arrow")].map(el => rgb(getComputedStyle(el).stroke));
    const boxFills = [...svg.querySelectorAll(".lm-box")].map(el => getComputedStyle(el).fill);
    return { bg, texts, strokes, boxFills };
  }, MAP);
}

test("Help shows the client view the map, with its title, description and words", async ({ page }) => {
  await page.goto("/");
  await page.locator("#helpBtn").click();
  const client = page.locator("#gsOverlay .gs-client");
  await expect(client).toBeVisible();
  // It sits at the top of the client view, before the steps.
  await expect(client.locator(MAP)).toBeVisible();
  const order = await page.evaluate(() => {
    const map = document.getElementById("gsMap");
    const steps = document.querySelector(".gs-client .gssteps");
    return Boolean(map.compareDocumentPosition(steps) & Node.DOCUMENT_POSITION_FOLLOWING);
  });
  expect(order).toBe(true);

  // A real text alternative: the image's name is its <title>, its
  // description the <desc>, and the same words are in the page beside it.
  const img = page.getByRole("img", { name: "Building, Floor, Loca: who is who" });
  await expect(img).toBeVisible();
  const desc = await page.evaluate(sel => {
    const svg = document.querySelector(sel);
    const ref = svg.getAttribute("aria-describedby");
    const node = ref && document.getElementById(ref);
    return node && node.closest("svg") === svg ? node.textContent : "";
  }, MAP);
  for (const term of ["Master", "Smasters", "Lobby", "Ground floor", "Floor", "Operator", "Lead", "Messages", "invite", "caretaker"]) {
    expect(desc).toContain(term);
  }
  const words = client.locator("#gsMapWords");
  await expect(words.locator("summary")).toHaveText("The map in words");
  await words.locator("summary").click();
  for (const term of ["Building", "Smasters", "Lobby", "Ground floor", "Floor", "caretaker", "Messages", "Operator", "Lead", "Chat"]) {
    await expect(words).toContainText(term);
  }
  await expect(words).toContainText("cannot revoke a member, appoint a Smaster or revoke an invite the Master gave");
});

test("Host: the map is on the Welcome tab, and Help brings it back", async ({ page }) => {
  await page.addInitScript(() => {
    window.__LOCA_HOST__ = true;
  });
  await page.goto("/");
  const welcome = page.locator("[data-gspanel='welcome']");
  await expect(welcome.locator(MAP)).toBeVisible();
  // Above the old two sentences: the map comes before the "A Building is…"
  // paragraph.
  const above = await page.evaluate(() => {
    const map = document.getElementById("gsMap");
    const para = [...document.querySelectorAll("[data-gspanel='welcome'] p")].find(p => p.textContent.startsWith("A Building is"));
    return Boolean(para && map.compareDocumentPosition(para) & Node.DOCUMENT_POSITION_FOLLOWING);
  });
  expect(above).toBe(true);
  // Only one map in the page, whichever view opened it.
  await expect(page.locator("svg.locamap")).toHaveCount(1);

  // Another tab hides it; closing and reopening from Help, it is one tab away.
  await page.locator(".gstab", { hasText: "Skill Library" }).click();
  await expect(page.locator(MAP)).toBeHidden();
  await page.locator("#gsGot").click();
  await page.locator("#helpBtn").click();
  await page.locator(".gstab", { hasText: "Welcome" }).click();
  await expect(welcome.locator(MAP)).toBeVisible();
  await expect(page.locator("svg.locamap")).toHaveCount(1);
});

test("At 390 px the map fits without sideways scrolling, in every font", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/");
  await page.locator("#helpBtn").click();
  await expect(page.locator(MAP)).toBeVisible();
  for (const font of ["mono", "sans", "serif"]) {
    await page.evaluate(f => { document.documentElement.dataset.fontFamily = f; }, font);
    const fit = await page.evaluate(sel => {
      const svg = document.querySelector(sel);
      const card = document.querySelector(".gscard");
      const r = svg.getBoundingClientRect();
      const c = card.getBoundingClientRect();
      // Each line of text stays inside the box it is drawn in.
      const spill = [];
      for (const text of svg.querySelectorAll("text")) {
        const box = text.parentElement.querySelector(":scope > rect");
        if (!box) continue;
        const t = text.getBBox();
        const b = box.getBBox();
        if (t.x < b.x || t.x + t.width > b.x + b.width) spill.push(text.textContent);
      }
      return {
        pageScroll: document.documentElement.scrollWidth - window.innerWidth,
        cardScroll: card.scrollWidth - card.clientWidth,
        inside: r.left >= c.left && r.right <= c.right,
        scale: r.width / svg.viewBox.baseVal.width,
        spill,
      };
    }, MAP);
    expect(fit.pageScroll, font).toBeLessThanOrEqual(0);
    expect(fit.cardScroll, font).toBeLessThanOrEqual(0);
    expect(fit.inside, font).toBe(true);
    // Drawn near its own size: its 11 px text stays at least 10 px on a
    // phone, the size of the panel's own smallest text.
    expect(fit.scale, font).toBeGreaterThanOrEqual(0.9);
    expect(fit.spill, font).toEqual([]);
  }
  await page.evaluate(() => { document.documentElement.dataset.fontFamily = "mono"; });
  await page.setViewportSize({ width: 390, height: 1500 });
  await page.locator("#gsMap").screenshot({ path: test.info().outputPath("map-phone.png") });
});

test("Its words have at least 4.5:1 contrast in every theme, with every accent", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 1100 });
  await page.goto("/");
  await page.locator("#helpBtn").click();
  await expect(page.locator(MAP)).toBeVisible();
  for (const theme of ["dark", "light", "loca", "calm", "console"]) {
    for (const accent of ["teal", "blue", "violet", "amber", "rose"]) {
      // As theme.js applies a look: the theme with its light or dark scheme.
      await page.evaluate(([t, a]) => applyAppearance({ theme: t, accent: a }), [theme, accent]);
      const { bg, texts, strokes, boxFills } = await mapColours(page);
      expect(texts.length).toBeGreaterThan(30);
      for (const t of texts) {
        expect(contrast(t.fill, bg), `${theme}/${accent}: "${t.text}"`).toBeGreaterThanOrEqual(4.5);
      }
      // The outlines that show what is inside what: 3:1 for graphics.
      for (const s of strokes) expect(contrast(s, bg), `${theme}/${accent} outline`).toBeGreaterThanOrEqual(3);
      expect(new Set(boxFills)).toEqual(new Set(["none"]));
    }
    await page.evaluate(() => { document.documentElement.dataset.accent = "teal"; });
    await page.locator("#gsMap").screenshot({ path: test.info().outputPath(`map-${theme}.png`) });
  }
});
