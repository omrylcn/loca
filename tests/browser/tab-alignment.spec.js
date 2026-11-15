const { test, expect } = require("@playwright/test");

// In the desktop (Tauri/WebKit) build the Chat label did not appear on the same line
// as the others; in the web (Chromium) build it was aligned. The cause was the tabs' STRUCTURAL
// asymmetry: Chat has no `.dot`, the others do. Because the dot is an inline
// box, the label's vertical place was left to the baseline/strut calculation, and
// that calculation differs from engine to engine. On top of that the `font` shorthand
// reset line-height to `normal` and left the height to the font's metrics.
//
// This spec measures NOT THE DEFECT but the CONDITIONS that make it possible: in Chromium
// the alignment was already right, so an "is it aligned" assert alone could NOT
// CATCH this regression. The real protection is that line-height is fixed
// and that the label is centred by flex; both are asserted here.
//
// LIMIT: WebKit could not be installed on this machine (system libraries missing), so
// it was NOT REPRODUCED in the engine that actually breaks. This spec locks the conditions;
// it does not prove the desktop rendering.

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    try { localStorage.setItem("loca-gs-seen", "1"); } catch (e) {}
  });
});

async function tabStrip(page) {
  await page.goto("/");
  await page.evaluate(() => {
    document.querySelectorAll(".hidden").forEach((e) => e.classList.remove("hidden"));
    // The initial Building view is locked and the room tab strip lives inside a
    // display:none `.main.global`. Represent an opened loca before measuring;
    // otherwise every tab bounding box is zero and the width asserts measure
    // display:none instead of the reserved-dot layout.
    document.body.classList.remove("locked");
    const main = document.querySelector(".main");
    main?.classList.remove("global");
    main?.style.setProperty("display", "flex", "important");
    document.querySelector(".tabs")?.style.setProperty("display", "flex", "important");
  });
  return page.locator(".tabs .tab");
}

test("a tab label's vertical place depends on flex, NOT on the line box", async ({ page }) => {
  await tabStrip(page);
  const computed = await page.evaluate(() => {
    const cs = getComputedStyle(document.querySelector(".tabs .tab"));
    return { lineHeight: cs.lineHeight, display: cs.display, alignItems: cs.alignItems };
  });
  // `normal` = depends on the font's metrics; differs from engine to engine.
  expect(computed.lineHeight).not.toBe("normal");
  expect(computed.display).toContain("flex");
  expect(computed.alignItems).toBe("center");
});

test("every tab label in the strip sits on the same vertical line", async ({ page }) => {
  const t = await tabStrip(page);
  const places = await t.evaluateAll((els) =>
    els.map((el) => {
      const tn = [...el.childNodes].find((n) => n.nodeType === 3 && n.textContent.trim());
      const rg = document.createRange();
      rg.selectNode(tn);
      const b = rg.getBoundingClientRect();
      return { label: el.textContent.trim(), top: Math.round(b.top), bottom: Math.round(b.bottom) };
    })
  );
  expect(places.length).toBeGreaterThan(1);
  const first = places[0];
  for (const s of places) {
    expect(s.top, `${s.label} top edge differs from ${first.label}`).toBe(first.top);
    expect(s.bottom, `${s.label} bottom edge differs from ${first.label}`).toBe(first.bottom);
  }
});

test("the dot KEEPS ITS PLACE while hidden, so lighting it does not make the tab jump", async ({ page }) => {
  const t = await tabStrip(page);
  const measure = () => t.evaluateAll((els) => els.map((el) => Math.round(el.getBoundingClientRect().width)));
  // First check that the dot REALLY keeps its place. Without this line
  // a `display:none` sabotage left the test GREEN: with the dot taking no place in
  // either state the width "did not change" and the assert passed for nothing.
  const dotWidths = await t.evaluateAll((els) =>
    els.map((el) => { const d = el.querySelector(".dot");
      return d ? Math.round(d.getBoundingClientRect().width) : null; })
  );
  expect(dotWidths.filter((w) => w !== null).length).toBe(dotWidths.length - 1);
  for (const w of dotWidths) if (w !== null) expect(w, "the hidden dot keeps no place").toBeGreaterThan(0);

  const unlit = await measure();
  // Lock that the fixture is actually visible: a zero-width tab means the
  // opened-loca setup regressed and the equality below would pass vacuously.
  for (const w of unlit) expect(w, "the tab fixture is not visible").toBeGreaterThan(0);
  await t.evaluateAll((els) => els.forEach((el) => {
    const d = el.querySelector(".dot");
    if (d) d.classList.add("on");
  }));
  const lit = await measure();
  expect(lit).toEqual(unlit);
});
