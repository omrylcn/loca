const { test, expect } = require("@playwright/test");

// Masaustu (Tauri/WebKit) surumde Chat etiketi otekilerle ayni hizada
// gorunmuyordu; web (Chromium) surumde hizaliydi. Sebep sekmelerin YAPISAL
// asimetrisiydi: Chat'te `.dot` yok, otekilerde var. Nokta satir ici bir
// kutu oldugu icin etiketin dikey yeri baseline/strut hesabina kaliyordu ve
// o hesap motordan motora degisir. Ustune `font` kisayolu line-height'i
// `normal`e sifirlayip yuksekligi yazi tipi metriklerine birakiyordu.
//
// Bu spec KUSURU DEGIL, kusuru mumkun kilan KOSULLARI olcer: Chromium'da
// hiza zaten dogruydu, dolayisiyla "hizali mi" assert'i tek basina bu
// regresyonu YAKALAYAMAZDI. Asil koruma line-height'in sabitlenmis olmasi
// ve etiketin flex ile ortalanmasidir; ikisi de burada assert ediliyor.
//
// SINIR: WebKit bu makinede kurulamadi (sistem kutuphaneleri eksik), yani
// asil kiran motorda YENIDEN URETILMEDI. Bu spec kosullari kilitler,
// masaustu render'ini kanitlamaz.

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    try { localStorage.setItem("loca-gs-seen", "1"); } catch (e) {}
  });
});

async function sekmeler(page) {
  await page.goto("/");
  await page.evaluate(() => {
    document.querySelectorAll(".hidden").forEach((e) => e.classList.remove("hidden"));
    // The initial Building view deliberately hides the room tab strip. Make
    // the fixture represent an opened loca before measuring layout; otherwise
    // every bounding box is zero and the test measures display:none.
    document.querySelector(".main")?.classList.remove("global");
    document.querySelector(".tabs")?.style.setProperty("display", "flex", "important");
  });
  return page.locator(".tabs .tab");
}

test("sekme etiketinin dikey yeri satir kutusuna DEGIL flex'e baglidir", async ({ page }) => {
  await sekmeler(page);
  const stil = await page.evaluate(() => {
    const cs = getComputedStyle(document.querySelector(".tabs .tab"));
    return { lineHeight: cs.lineHeight, display: cs.display, alignItems: cs.alignItems };
  });
  // `normal` = yazi tipi metriklerine bagli; motordan motora degisir.
  expect(stil.lineHeight).not.toBe("normal");
  expect(stil.display).toContain("flex");
  expect(stil.alignItems).toBe("center");
});

test("dort sekmenin etiketi ayni dikey hizada", async ({ page }) => {
  const t = await sekmeler(page);
  const yer = await t.evaluateAll((els) =>
    els.map((el) => {
      const tn = [...el.childNodes].find((n) => n.nodeType === 3 && n.textContent.trim());
      const rg = document.createRange();
      rg.selectNode(tn);
      const b = rg.getBoundingClientRect();
      return { ad: el.textContent.trim(), top: Math.round(b.top), bottom: Math.round(b.bottom) };
    })
  );
  expect(yer.length).toBe(4);
  const ilk = yer[0];
  for (const s of yer) {
    expect(s.top, `${s.ad} ust hizasi ${ilk.ad} ile ayni degil`).toBe(ilk.top);
    expect(s.bottom, `${s.ad} alt hizasi ${ilk.ad} ile ayni degil`).toBe(ilk.bottom);
  }
});

test("nokta gorunmezken de YER TUTAR, yaninca sekme genisligi ziplamaz", async ({ page }) => {
  const t = await sekmeler(page);
  const olc = () => t.evaluateAll((els) => els.map((el) => Math.round(el.getBoundingClientRect().width)));
  // Once noktanin GERCEKTEN yer tuttugunu dogrula. Bu satir olmadan
  // `display:none` sabotaji testi YESIL birakiyordu: nokta iki durumda da
  // yer tutmayinca genislik "degismiyor" ve assert bosa donuyordu.
  const noktaGen = await t.evaluateAll((els) =>
    els.map((el) => { const d = el.querySelector(".dot");
      return d ? Number.parseFloat(getComputedStyle(d).width) : null; })
  );
  expect(noktaGen.filter((w) => w !== null).length).toBe(3);
  for (const w of noktaGen) if (w !== null) expect(w, "nokta gorunmezken yer tutmuyor").toBeGreaterThan(0);

  const kapali = await olc();
  for (const w of kapali) expect(w, "sekme fixture gorunur degil").toBeGreaterThan(0);
  await t.evaluateAll((els) => els.forEach((el) => {
    const d = el.querySelector(".dot");
    if (d) d.classList.add("on");
  }));
  const acik = await olc();
  expect(acik).toEqual(kapali);
});
