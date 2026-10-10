# Build from source

Monoptah supports macOS, Linux, and Windows. You need Node.js 20+ and a current stable Rust toolchain. Install and sign in to at least one agent CLI using [provider setup](providers.md).

## Platform prerequisites

On Linux, install the standard Tauri prerequisites, including `libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `libsoup-3.0-dev`, and `libjavascriptcoregtk-4.1-dev` on Debian/Ubuntu. The setup helpers below install the native dependencies and packaging tools for their distributions.

On Windows, the packaging script uses `stable-x86_64-pc-windows-msvc`. The installer bootstraps the [WebView2](https://developer.microsoft.com/microsoft-edge/webview2/) runtime when it is missing.

## Run locally

From the repository root:

```bash
npm install
npm run tauri dev
```

## Ubuntu / Debian packages

On an Ubuntu/Debian workstation, the repository can install the native Tauri prerequisites and build distributable Linux packages directly:

```bash
npm run setup:linux:deb
npm ci
npm run build:linux
```

The Linux build emits `.deb` and AppImage bundles under `target/release/bundle/`, in the `deb/` and `appimage/` directories. `build:linux` repacks the AppImage so it uses the host WebKitGTK 4.1 stack instead of bundled Ubuntu libraries. See [Linux installation](install.md) for runtime dependencies and package instructions.

## Fedora / Enterprise Linux packages

On Fedora or Enterprise Linux 10 (registered RHEL, Rocky, Alma, CentOS Stream, Oracle):

```bash
npm run setup:linux:fedora
npm ci
npm run build:fedora
```

On EL 10, the setup helper enables EPEL 10 and CRB automatically, since the development packages need CRB. On Oracle Linux it enables `ol10_codeready_builder` and `ol10_developer_EPEL`. Fedora does not need these extra repositories.

The build emits a `.rpm` under `target/release/bundle/rpm/`, installable with:

```bash
sudo dnf install ./target/release/bundle/rpm/Monoptah-*.rpm
```

EL 9 and older are unsupported (`webkit2gtk4.1-devel` only exists in EPEL 10). The published release `.rpm` is built on Enterprise Linux 10 so it loads on Fedora and EL 10. See [Linux installation](install.md#fedora--enterprise-linux-packages) for release package installation and runtime repository setup.

## Windows packages

```bash
npm ci
npm run build:windows
```

The Windows build emits an NSIS installer under `target/release/bundle/nsis/`.

## Tauri configuration

- [src-tauri/tauri.conf.json](../src-tauri/tauri.conf.json) contains the shared application and bundle configuration.
- [src-tauri/tauri.linux.conf.json](../src-tauri/tauri.linux.conf.json) loads automatically for Linux development and builds.
- [src-tauri/tauri.windows.conf.json](../src-tauri/tauri.windows.conf.json) loads automatically for Windows development and builds.

## Releases and updates

The [release workflow](../.github/workflows/release.yml) packages the platform builds, signs the updater bundles, and publishes the updater feed.

Read [CONTRIBUTING.md](../CONTRIBUTING.md) for contribution guidelines and checks. The experimental remote host has its own [build and packaging instructions](remote-access.md#release-packaging).
