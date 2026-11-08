# Rex program icons

Original program and agent artwork exported from Rex Beta 0.1.0 (1875), installed at `Rex Beta.app/Contents/Resources/SuperBlockIcons_SuperBlockIcons.bundle` on 2026-11-08. `shell.png` is the app's `SettingsIconTerminal` asset, used by the native picker for Zsh and unknown programs; it was also verified unchanged against Rex Beta 0.1.0 (1880). Asset names are `block-glyph-NAME`; the PNGs retain Rex’s artwork and are rasterized at 48×48 for terminal display. A small neutral backplate preserves the contrast of white and black artwork on selected and unselected rows. Artwork and product marks belong to their respective owners; the repository’s code license does not relicense these assets.

The binary embeds these files and does not need the app bundle, Swift or network access at runtime. Refresh the files on macOS with:

```sh
swift scripts/extract_rex_icons.swift '/Applications/Rex Beta.app/Contents/Resources/SuperBlockIcons_SuperBlockIcons.bundle' assets/programs
```

The exporter reads the enclosing Rex app bundle for the default terminal icon as well as the program icon bundle. Keep the sorted inventory in `src/icons.rs` in sync when Rex adds or removes assets. Unit tests check PNG dimensions, names and terminal placeholder width.
