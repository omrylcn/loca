"use strict";
// A person's avatar: an icon (or their initials) on a colour, in a shape.
// Display only (docs/concepts.md "Full name and profile icon"): the @name is
// who someone is. The icon, colour and shape are closed lists the server
// validates (crates/server/src/hub/profile_card.rs); each is drawn here, once.
// Icons are line drawings on a 24-unit grid in one stroke style, so they
// read the same at the size of a letter and at the size of a face.

const PROFILE_ICONS = {
  // Marks (the first twelve: what every older client also knows).
  dot: `<circle cx="12" cy="12" r="4.5" fill="currentColor"/>`,
  ring: `<circle cx="12" cy="12" r="7.5"/>`,
  square: `<rect x="5" y="5" width="14" height="14" rx="2.5"/>`,
  diamond: `<path d="M12 3.5 20.5 12 12 20.5 3.5 12Z"/>`,
  triangle: `<path d="M12 4.5 20.5 19h-17Z"/>`,
  hexagon: `<path d="M12 3.5l7.4 4.25v8.5L12 20.5l-7.4-4.25v-8.5Z"/>`,
  star: `<path d="M12 3.6l2.5 5.3 5.8.7-4.3 4 1.1 5.7L12 16.5l-5.1 2.8 1.1-5.7-4.3-4 5.8-.7Z"/>`,
  spark: `<path d="M12 3c.8 4.6 3.4 7.2 9 9-5.6 1.8-8.2 4.4-9 9-.8-4.6-3.4-7.2-9-9 5.6-1.8 8.2-4.4 9-9Z"/>`,
  leaf: `<path d="M5 19C5 10 10 5 19 5c0 9-5 14-14 14ZM5 19l8-8"/>`,
  wave: `<path d="M3 10c3-4 6-4 9 0s6 4 9 0M3 15.5c3-4 6-4 9 0s6 4 9 0"/>`,
  moon: `<path d="M19 14.5A8 8 0 1 1 9.5 5a6.5 6.5 0 0 0 9.5 9.5Z"/>`,
  sun: `<circle cx="12" cy="12" r="4"/><path d="M12 2.5V5M12 19v2.5M2.5 12H5M19 12h2.5M5.3 5.3l1.8 1.8M16.9 16.9l1.8 1.8M5.3 18.7l1.8-1.8M16.9 7.1l1.8-1.8"/>`,
  // Work and days.
  laptop: `<path d="M5 6.5h14a1 1 0 0 1 1 1V16H4V7.5a1 1 0 0 1 1-1ZM2.5 16h19l-1.2 2.5H3.7Z"/>`,
  glasses: `<circle cx="7" cy="14" r="3.5"/><circle cx="17" cy="14" r="3.5"/><path d="M10.5 14c.9-1 2.1-1 3 0M3.6 13 5 7.5M20.4 13 19 7.5"/>`,
  coffee: `<path d="M5 9.5h11v4.5a5 5 0 0 1-5 5h-1a5 5 0 0 1-5-5ZM16 11h1.5a2.5 2.5 0 0 1 0 5H16M8.5 3.5c-.8 1 .8 2 0 3.5M12 3.5c-.8 1 .8 2 0 3.5"/>`,
  headphones: `<path d="M4 15.5V12a8 8 0 0 1 16 0v3.5M4 15h3.5v5.5H5.5A1.5 1.5 0 0 1 4 19ZM20 15h-3.5v5.5h2A1.5 1.5 0 0 0 20 19Z"/>`,
  umbrella: `<path d="M3 12.5a9 9 0 0 1 18 0ZM12 12.5v6a2 2 0 0 1-4 0M12 3.5V2.5"/>`,
  bicycle: `<circle cx="6" cy="16" r="3.5"/><circle cx="18" cy="16" r="3.5"/><path d="M6 16l3.5-6.5h5.5L18 16M9.5 9.5 12.5 16H6M8 7h3M15 9.5l-1-3h2.5"/>`,
  // Animals.
  cat: `<path d="M5 9.5V4l4.2 3.3h5.6L19 4v5.5c1 1.4 1.5 2.9 1.5 4.5 0 3.9-3.8 6.5-8.5 6.5S3.5 17.9 3.5 14c0-1.6.5-3.1 1.5-4.5ZM11 16l1 .8 1-.8"/><circle cx="9" cy="13" r=".9" fill="currentColor"/><circle cx="15" cy="13" r=".9" fill="currentColor"/>`,
  fox: `<path d="M3.5 4 9 8.5h6L20.5 4l-1.3 8.2L12 20l-7.2-7.8ZM4.8 12.2 9 13.5M19.2 12.2 15 13.5"/><circle cx="9.6" cy="11.2" r=".9" fill="currentColor"/><circle cx="14.4" cy="11.2" r=".9" fill="currentColor"/><circle cx="12" cy="16.6" r=".9" fill="currentColor"/>`,
  owl: `<path d="M6 8.5C6 7.4 6 5 5 3.8c2 0 3.6 1 4.6 1.9a7 7 0 0 1 4.8 0c1-.9 2.6-1.9 4.6-1.9-1 1.2-1 3.6-1 4.7V15a6 6 0 0 1-12 0ZM11.2 14 12 15.3l.8-1.3"/><circle cx="9.4" cy="11.3" r="2.2"/><circle cx="14.6" cy="11.3" r="2.2"/>`,
  bird: `<path d="M3.5 16.5c3 0 5-1.3 6-3.5a5.5 5.5 0 0 1 10.3-3.4L21.5 9l-1.2 2.4c-.6 4.8-4.4 8.1-9.3 8.1-3 0-5.5-1.2-7.5-3ZM9.8 13.5c1.6 1.7 4 2.2 6.2 1.2"/><circle cx="16.2" cy="9.6" r=".9" fill="currentColor"/>`,
  fish: `<path d="M20.5 12c-2 3.3-4.8 5.5-8.5 5.5S5.8 15.3 4.5 12C5.8 8.7 8.3 6.5 12 6.5s6.5 2.2 8.5 5.5ZM4.5 12 2 8.5v7ZM9.5 9.5c1 1.5 1 3.5 0 5"/><circle cx="16" cy="11" r=".9" fill="currentColor"/>`,
  turtle: `<path d="M4 15.5a7 7 0 0 1 14 0ZM7 15.5v2.5M15 15.5v2.5M18 13h1.8a1.7 1.7 0 0 0 0-3.4c-1 0-1.8.8-1.8 1.7M8.5 15.5l1.3-4h4.4l1.3 4"/>`,
  // Nature.
  mountain: `<path d="M2.5 19.5 9 8l4.2 7.2 2.6-4 5.7 8.3ZM7.2 11.2 9 13l1.6-1.8"/>`,
  tree: `<path d="M12 3 6.5 10.5h3l-4 6.5h13l-4-6.5h3ZM12 17v4"/>`,
  flower: `<path d="M12 4c-2.4 2-5 3-5 6.5a5 5 0 0 0 10 0C17 7 14.4 6 12 4ZM12 4v4.5M12 15.5V21M12 18.5c-1.8-1.8-3.7-2-5.5-1.5M12 19.5c1.8-1.8 3.7-2 5.5-1.5"/>`,
  planet: `<circle cx="12" cy="12" r="5.5"/><path d="M6.4 13.9c-2.6 1.4-4 2.9-3.6 3.8.7 1.5 6.2-.1 12.3-3.6s10-7.6 9.2-9c-.4-.8-2.2-.7-4.6.2"/>`,
  bolt: `<path d="M13 2.5 5 13.5h6l-1 8 8-11h-6Z"/>`,
  drop: `<path d="M12 3.5c3 4 6 7.2 6 10.5a6 6 0 0 1-12 0c0-3.3 3-6.5 6-10.5Z"/>`,
};

// None of these names is a loca's icon (LOCA_ICONS in identity.js and the
// protocol crate): a person and a loca are never drawn alike.
// The order and the groups the picker shows them in. `initials` is drawn
// from the person's own name, not from this table.
const AVATAR_ICON_GROUPS = [
  ["Marks", ["dot", "ring", "square", "diamond", "triangle", "hexagon", "star", "spark", "wave", "moon", "sun", "bolt"]],
  ["Days", ["laptop", "glasses", "coffee", "headphones", "umbrella", "bicycle"]],
  ["Animals", ["cat", "fox", "owl", "bird", "fish", "turtle"]],
  ["Nature", ["leaf", "tree", "flower", "mountain", "planet", "drop"]],
];

// Background colours: one shade in both themes, each with the ink that
// keeps the icon and the initials readable on it (4.5:1 or better).
const AVATAR_COLORS = {
  teal: ["#0d7a6f", "#ffffff"], green: ["#2b7d46", "#ffffff"], lime: ["#a6d65a", "#1b2608"],
  sky: ["#5bc2ea", "#062433"], blue: ["#2f5fd0", "#ffffff"], indigo: ["#4b4fc4", "#ffffff"],
  violet: ["#7442c8", "#ffffff"], plum: ["#93378a", "#ffffff"], rose: ["#c2335f", "#ffffff"],
  red: ["#b8322a", "#ffffff"], orange: ["#f08a3e", "#2a1203"], amber: ["#f2b33d", "#2a1d00"],
  sand: ["#e3d1ac", "#2d2414"], slate: ["#4a5568", "#ffffff"],
};
const AVATAR_SHAPES = ["circle", "rounded", "square", "hexagon"];

function profileIconSvg(icon) {
  if (!Object.hasOwn(PROFILE_ICONS, icon)) return "";
  return `<svg class="picon" viewBox="0 0 24 24" aria-hidden="true" focusable="false">${PROFILE_ICONS[icon]}</svg>`;
}

// One or two letters: the first letters of the first and last word of the
// full name, or the start of the @name. Letters only, upper-cased.
function avatarInitials(fullName, name) {
  const words = String(fullName || "").trim().split(/\s+/).filter(Boolean)
    .map(word => Array.from(word).find(ch => /\p{L}/u.test(ch)) || "").filter(Boolean);
  if (words.length) return (words.length > 1 ? words[0] + words[words.length - 1] : words[0]).toLocaleUpperCase();
  const letters = Array.from(String(name || "")).filter(ch => /\p{L}|\d/u.test(ch));
  return letters.slice(0, 2).join("").toLocaleUpperCase() || "?";
}

// Whether a card draws an avatar at all: an icon, or a colour (which then
// carries the initials).
function cardHasAvatar(card) {
  return !!(card && (card.icon || card.avatar_color));
}

// The avatar of a card. `size` is a class: "xs" sits in a line of text, "lg"
// heads the profile. A card with neither icon nor colour draws nothing at
// "xs" and a neutral initials avatar elsewhere.
function avatarHtml(card, name, size = "xs") {
  const icon = card?.icon || "";
  const color = Object.hasOwn(AVATAR_COLORS, card?.avatar_color) ? card.avatar_color : "";
  if (size === "xs" && !icon && !color) return "";
  const shape = AVATAR_SHAPES.includes(card?.avatar_shape) ? card.avatar_shape : "circle";
  const face = icon && icon !== "initials" && Object.hasOwn(PROFILE_ICONS, icon)
    ? profileIconSvg(icon)
    : `<span class="avinitials">${esc(avatarInitials(card?.full_name, name))}</span>`;
  const style = color ? ` style="--av-bg:${AVATAR_COLORS[color][0]};--av-ink:${AVATAR_COLORS[color][1]}"` : "";
  return `<span class="avatar av-${size} av-${shape}${color ? " av-filled" : ""}"${style} aria-hidden="true">${face}</span>`;
}
