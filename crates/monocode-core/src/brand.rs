//! Fork identity. Names here are product copy and bundle identifiers; file names,
//! env vars and protocol strings keep their `monocode` spelling.

/// The product the user installed and reads data from.
pub const PRODUCT: &str = "Monoptah";
/// The native app's own name.
pub const NATIVE_PRODUCT: &str = "Monoptah Native";
/// Tauri bundle identifier of the installed desktop app, which names its data directories.
pub const DESKTOP_APP_ID: &str = "com.monoptah.desktop";
/// Bundle identifier of the native app; the updater matches packages on it.
pub const NATIVE_APP_ID: &str = "com.monoptah.native";
