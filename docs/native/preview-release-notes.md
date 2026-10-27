This is the first experimental Monoptah Native preview, built with Rust + GPUI. React/Tauri remains the production app.

Choose the download for your platform: macOS Apple Silicon or Intel, Windows x64, or Linux x64 (AppImage, Debian/Ubuntu `.deb`, or Fedora/Enterprise Linux `.rpm`). Install it alongside your existing Monoptah application.

**Testing status:** macOS is the primary tested platform. A Linux ARM64 source build was launched and exercised in an Ubuntu VM with a graphics workaround; the downloadable Linux x64 packages and Windows app still need runtime testing. Native system notifications currently support macOS only. HTML panels and media support remain limited on Linux and Windows.

**Data:** this preview uses your existing Monoptah database and saves changes there. Back up your Monoptah data with the app closed before testing. Native preferences are stored separately. Avoid editing the same conversations in both apps at once.

**Updates:** the sidebar is configured to show the next native preview and offer download, verification, installation, and restart. Settings → General also offers Check for updates. The first packaged upgrade will be tested with a subsequent preview; if updating fails, download the new installer manually. Linux `.deb`/`.rpm` installs use the system authentication prompt (`pkexec`) to install the downloaded package. Cancelling authentication leaves the current version installed. Native previews receive native preview updates; production Tauri releases use their own feed.

The initial installation requires a download. macOS completion/approval notifications, sounds, and Dock badges still need testing in the distributed app.

Feature and platform parity are still being tested. Read `docs/native/parity.md` at this release's tag for known limitations. A successfully built package does not establish that every feature works on that platform.

Please report the preview version, operating system/architecture, installation format, provider, and reproduction steps with any issue. Remove credentials and private conversation content from logs and screenshots.
