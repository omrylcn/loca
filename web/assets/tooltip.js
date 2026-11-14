"use strict";
// Header tooltips. Any element with `data-tip` gets a small styled bubble on
// hover (after a short delay) or keyboard focus (at once). `data-tip-detail`
// adds a muted second line. One bubble lives on <body> with position:fixed so
// the header's overflow never clips it. Touch input never opens a bubble: a
// tap is a click, and the aria-label already names the control.
(function () {
  const DELAY_MS = 280;
  let bubble = null;
  let owner = null;
  let timer = 0;

  function ensureBubble() {
    if (bubble) return bubble;
    bubble = document.createElement("div");
    bubble.className = "tip";
    bubble.id = "tipBubble";
    bubble.setAttribute("role", "tooltip");
    bubble.hidden = true;
    document.body.appendChild(bubble);
    return bubble;
  }

  function place(el) {
    const b = ensureBubble();
    const r = el.getBoundingClientRect();
    const gap = 8;
    const bw = b.offsetWidth, bh = b.offsetHeight;
    let left = r.left + r.width / 2 - bw / 2;
    left = Math.max(8, Math.min(left, window.innerWidth - bw - 8));
    let top = r.bottom + gap;
    let below = true;
    if (top + bh > window.innerHeight - 8) { top = r.top - gap - bh; below = false; }
    b.style.left = `${Math.round(left)}px`;
    b.style.top = `${Math.round(top)}px`;
    b.classList.toggle("above", !below);
    const arrow = r.left + r.width / 2 - left;
    b.style.setProperty("--tip-arrow", `${Math.round(Math.max(10, Math.min(arrow, bw - 10)))}px`);
  }

  function show(el) {
    const text = el.dataset.tip;
    if (!text || el.classList.contains("hidden") || !el.isConnected) return;
    const b = ensureBubble();
    const detail = el.dataset.tipDetail;
    b.innerHTML = `<b>${esc(text)}</b>${detail ? `<span>${esc(detail)}</span>` : ""}`;
    b.hidden = false;
    owner = el;
    el.setAttribute("aria-describedby", "tipBubble");
    place(el);
  }

  function hide() {
    clearTimeout(timer);
    if (owner) owner.removeAttribute("aria-describedby");
    owner = null;
    if (bubble) bubble.hidden = true;
  }

  // Text changes while the bubble is open (live toggled, connection dropped)
  // refresh it in place.
  window.refreshTip = function (el) { if (el && el === owner) show(el); };

  document.addEventListener("pointerover", (e) => {
    if (e.pointerType === "touch") return;
    const el = e.target.closest && e.target.closest("[data-tip]");
    if (!el || el === owner) return;
    hide();
    timer = setTimeout(() => show(el), DELAY_MS);
  });
  document.addEventListener("pointerout", (e) => {
    const el = e.target.closest && e.target.closest("[data-tip]");
    if (!el) return;
    if (e.relatedTarget && el.contains(e.relatedTarget)) return;
    hide();
  });
  document.addEventListener("focusin", (e) => {
    const el = e.target.closest && e.target.closest("[data-tip]");
    if (el && el.matches(":focus-visible")) show(el); else hide();
  });
  document.addEventListener("focusout", hide);
  document.addEventListener("pointerdown", hide);
  document.addEventListener("keydown", (e) => { if (e.key === "Escape") hide(); });
  window.addEventListener("scroll", hide, true);
  window.addEventListener("resize", hide);
})();
