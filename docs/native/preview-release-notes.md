Monoptah Native is an experimental Rust + GPUI preview. React/Tauri remains the production app.

Choose the download for your platform: macOS Apple Silicon or Intel, Windows x64, or Linux x64 (AppImage, Debian/Ubuntu `.deb`, or Fedora/Enterprise Linux `.rpm`). Install it alongside your existing Monoptah application.

**Data:** this preview uses your existing Monoptah database and saves changes there. Back up your Monoptah data with the app closed before testing. Native preferences are stored separately. Avoid editing the same conversations in both apps at once.

**Updates:** the sidebar shows the next native preview when one is available. Click it to download, verify, install, and restart. Settings → General also offers Check for updates. Linux `.deb`/`.rpm` installs use the system authentication prompt (`pkexec`) to install the downloaded package. Cancelling authentication leaves the current version installed. Native previews receive native preview updates; production Tauri releases use their own feed.

The initial installation requires a download. Subsequent previews can be installed from within the native app.

Feature and platform parity are still being tested. Read `docs/native/parity.md` at this release's tag for known limitations. A successfully built package does not establish that every feature works on that platform.

Please report the preview version, operating system/architecture, installation format, provider, and reproduction steps with any issue. Remove credentials and private conversation content from logs and screenshots.
