const { test, expect } = require("@playwright/test");

for (const width of [390, 1280]) {
  test(`sign-in fields share theme and fit at ${width}px`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width, height: 900 });
    await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
    await page.goto("/");
    const kind = page.getByLabel("Sign in as", { exact: true });
    await expect(kind).toBeVisible();
    await kind.focus();
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");
    await expect(kind).toHaveValue("smaster");
    await page.keyboard.press("Escape");
    await page.keyboard.press("Tab");
    await expect(page.locator("#name")).toBeFocused();
    await expect(page.locator("#name")).toHaveAttribute("placeholder", "your display name — leave empty to keep it");
    await expect(page.locator("#joinRequestBtn")).toBeHidden();
    await expect(page.locator("#connectBtn")).toHaveText("sign in as Smaster");
    const sizes = await page.evaluate(() => {
      const selector = document.querySelector("#entryKind");
      const input = document.querySelector("#roomToken");
      return {
        select: selector.getBoundingClientRect().width,
        input: input.getBoundingClientRect().width,
        selectHeight: selector.getBoundingClientRect().height,
        themed: getComputedStyle(selector).backgroundColor === getComputedStyle(input).backgroundColor,
        fits: document.documentElement.scrollWidth <= innerWidth,
      };
    });
    expect(sizes.select).toBe(sizes.input);
    expect(sizes.selectHeight).toBeGreaterThanOrEqual(40);
    expect(sizes.themed).toBe(true);
    expect(sizes.fits).toBe(true);
    await testInfo.attach(`smaster-${width}`, { body: await page.screenshot(), contentType: "image/png" });
    await page.locator("#masterEntryButton").click();
    await expect(page.getByLabel("Master pairing code", { exact: true })).toBeVisible();
    await expect(kind).toBeHidden();
    await page.locator("#masterEntryButton").click();
    await expect(kind).toHaveValue("smaster");
    await expect(kind).toBeVisible();
  });
}

// The desktop client's default window is 1100x760 (a 1100x733 page), and a
// short laptop screen is no taller. The card is then taller than the window.
test("in a short window every control of the sign-in card is within the mouse's reach", async ({ page }) => {
  await page.setViewportSize({ width: 1100, height: 733 });
  await page.addInitScript(() => localStorage.setItem("loca-gs-seen", "1"));
  await page.goto("/");
  const card = page.locator("aside.side");
  const join = page.locator("#joinRequestBtn");
  await expect(page.locator("#connectBtn")).toBeVisible();
  // The sign-in page is drawn once the page knows it is at the door; before
  // that the same form sits in the plain sidebar (seen on a busy server: the
  // styles read too early were the sidebar's, not the card's).
  await expect(page.locator("body")).toHaveClass(/\blocked\b/);
  await expect(page.locator("body")).not.toHaveClass(/auth-pending/);
  const taller = await card.evaluate(el => el.scrollHeight > el.clientHeight + 1);
  if (!taller) {
    // Everything already fits: then the last control must simply be inside.
    const [c, last] = await Promise.all([card.boundingBox(), page.locator("aside.side .conn > :last-child").boundingBox()]);
    expect(last.y + last.height).toBeLessThanOrEqual(c.y + c.height + 1);
    return;
  }
  // Taller than its box: it must be a scroll container (a wheel or a drag of
  // its bar moves it), and scrolled to its end the last control is inside.
  // The wheel itself is not sent: one wheel event is not a reliable way to
  // reach the end on every runner.
  expect(["auto", "scroll"]).toContain(await card.evaluate(el => getComputedStyle(el).overflowY));
  await card.evaluate(el => el.scrollTo(0, el.scrollHeight));
  const lastVisible = page.locator("aside.side .conn button:visible").last();
  const [c, b] = await Promise.all([card.boundingBox(), lastVisible.boundingBox()]);
  expect(b.y + b.height).toBeLessThanOrEqual(c.y + c.height + 1);
  expect(await join.count()).toBe(1);
});
