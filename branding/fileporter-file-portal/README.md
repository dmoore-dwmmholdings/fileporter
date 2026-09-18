# Fileporter — File Portal branding kit

The selected identity: a folded-corner file passing through an upright portal, with integrated motion strokes. This kit is separate from the installed application assets.

## Start here
- `index.html`: visual brand guide and asset gallery (open locally).
- `source/app-icon-master.png`: full native-resolution, text-free app icon.
- `icons/png/fileporter-1024.png`: opaque 1024px icon with square outer corners.
- `icons/ios/AppIcon.appiconset`: standalone iOS asset catalog entry.
- `icons/macos/Fileporter.icns`: macOS icon container and companion iconset.
- `icons/windows/Fileporter.ico`: multi-resolution Windows icon.
- `logos/`: horizontal light and dark raster lockups.
- `web/`: favicon, touch icon, web icons and manifest.
- `social/`: avatar and share card.
- `fonts/`: IBM Plex Sans and Mono Latin webfonts with licenses.
- `tokens.css` and `palette.json`: reusable brand tokens.

## Identity and usage
Use the standalone symbol for app icons and avatars. Use the horizontal lockup where the product name must be readable. Preserve aspect ratio and the supplied icon padding. Do not add words inside the app icon, mirror the portal, stretch the artwork, or place the mint mark on a pale background. Use the dark-on-light lockup on light surfaces.

Keep clear space around a logo at least equal to the folded corner's width. Recommended minimum displayed horizontal-lockup width is 240px; use the app icon below that. The 16–32px exports necessarily soften the smallest gaps. Do not crop icon masters or pre-apply an iOS corner mask.

## Color and typography
Mint #74e6a0, Forest #0a100d, Ivory #eaf6ef. Secondary Soft #d6e5db and Muted #8aa398. These are the intended brand colors; generated raster pixels have slight variation. IBM Plex Sans: 400 body, 500 labels, 600 headings. IBM Plex Mono 400 for file metadata. Keep headings brief and sentence case. The raster wordmarks are generated lettering, not editable font outlines.

## Voice
Direct, calm, useful. Describe the action and destination: “Send files”, “Choose a device”, “Transfer complete”. Product description: “Private, direct file transfer between your devices.” Avoid unsupported speed or security claims.

## Production notes
Artwork was produced with the built-in image generation tool using the original selected File Portal image. PNG size exports and platform containers were derived from the same opaque icon master; no independent icon redraws. These are raster assets, not SVG/vector source. Generated symbol source is retained for provenance; use the opaque icon master for app packaging. Mobile/desktop exports are supplied but are not installed or tested in an app build. Existing app code and icons were not modified.

See `PROMPTS.md` for generation direction and `verification.json` for dimensions and export checks.
