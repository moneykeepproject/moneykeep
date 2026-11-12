# macOS disk image window

`package.sh` makes `moneykeep-<version>-macos-<arch>.dmg` with `hdiutil create -srcfolder`
from a folder holding `MoneyKeep.app` and a link to `/Applications`
(docs/specs/macos-release.md P3). The volume is called `MoneyKeep` (no version).

For now Finder shows the image in its plain window: the two icons, no background picture.
Nothing in this folder is copied into the image yet.

## Adding a laid-out window later

MoneyKeep Launcher's `packaging/macos/dmg/` (the project's own code) shows how: a background
picture as a HiDPI TIFF in `.background/background.tiff`, and a Finder layout written as
`.DS_Store` by a script, both committed and copied in by `package.sh`, so the image still builds
with a plain `hdiutil create` and no Finder scripting. To do the same here:

- write the background (SVG source and the TIFF made from it) and the layout script, with
  the volume name `MoneyKeep` and the icon names `MoneyKeep.app` and `Applications`;
- copy both into the staging folder in `package.sh` before `hdiutil create`;
- keep the volume name without a version: the layout finds the background through an alias
  that names the volume.

`verify.sh` checks the app inside the finished image with `codesign --verify --strict`, so
nothing added to the image may give the app extended attributes.
