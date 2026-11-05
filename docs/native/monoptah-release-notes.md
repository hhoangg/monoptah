# Monoptah Native 0.45.0

## Changes since 0.44.0

- Linux and Windows installs now update themselves. The AppImage, Debian package, Enterprise Linux rpm and Windows x64 installer are signed and listed in the update feed next to the macOS updater, and an rpm package is new in this release.
- Split terminals to the right, drag dock terminals out into splits or title tabs, and keep the Terminal panel open with a fresh shell after dragging one out. Title tabs show when a session has an unsent draft.
- Split sessions start with your default access mode.
- Model pickers show fast mode.
- Keep retrying frames until a window's redraw is answered, and stop the macOS window freezing after a screenshot is dropped onto it. The freeze log now also records window occlusion, resize and file drop state.
- Include upstream changes through 9ee7b3dd.

## Known limits

- macOS packages are ad-hoc signed and are not Apple-notarized. The Windows installer is unsigned, so SmartScreen warns on first install.
- Linux and Windows desktop interaction still need broader runtime testing.
