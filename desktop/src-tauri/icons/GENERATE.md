# App icons — loca-dev generates these

loca-care has no image toolchain, so the actual icon binaries are not committed.
Generate the full set from the existing Loca brand glyph:

```bash
# from repo root, on the desktop branch, with the tauri CLI installed:
# (rasterize the SVG to a 1024x1024 PNG first, then let tauri fan it out)
rsvg-convert -w 1024 -h 1024 web/assets/favicon.svg -o /tmp/loca-src.png   # or inkscape/convert
cargo tauri icon /tmp/loca-src.png                                          # writes into src-tauri/icons/
```

`cargo tauri icon` produces `32x32.png`, `128x128.png`, `128x128@2x.png`,
`icon.icns`, `icon.ico`, and the Windows Store logos referenced by
`tauri.conf.json`. Commit the generated `icons/*` on this branch.

Source glyph: `web/assets/favicon.svg` (teal square-in-square on `#0b0e12`).
