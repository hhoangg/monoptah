use gpui::App;
use std::sync::OnceLock;

/// GPUI resolves this family to the platform system UI font.
pub(crate) const UI: &str = ".SystemUIFont";

/// GPUI matches family names literally outside macOS, so the CSS generic
/// `monospace` resolves to nothing there and text falls back to a
/// proportional sans. Mirror the web build's terminal stack instead.
const MONO_CANDIDATES: &[&str] = &[
    "JetBrainsMono NFM",
    "JetBrainsMono Nerd Font Mono",
    "JetBrainsMono NF",
    "MesloLGS NF",
    "MesloLGS Nerd Font Mono",
    "FiraCode Nerd Font Mono",
    "Hack Nerd Font Mono",
    "CaskaydiaCove Nerd Font Mono",
    "JetBrains Mono",
    "Cascadia Mono",
    "Consolas",
    "DejaVu Sans Mono",
    "Ubuntu Mono",
    "Noto Sans Mono",
    "Liberation Mono",
    "Courier New",
];

static MONO: OnceLock<String> = OnceLock::new();

/// The monospace family for code and the terminal.
pub(crate) fn mono() -> &'static str {
    MONO.get().map_or(
        if cfg!(target_os = "macos") {
            "Menlo"
        } else {
            "monospace"
        },
        String::as_str,
    )
}

pub(crate) fn init(cx: &App) {
    if cfg!(target_os = "macos") {
        return;
    }
    if let Some(family) = pick_mono(&cx.text_system().all_font_names(), system_mono) {
        let _ = MONO.set(family);
    }
}

fn pick_mono(installed: &[String], system: impl FnOnce() -> Option<String>) -> Option<String> {
    let has = |name: &str| installed.iter().any(|font| font == name);
    MONO_CANDIDATES
        .iter()
        .find(|name| has(name))
        .map(|name| name.to_string())
        .or_else(|| system().filter(|name| has(name)))
}

/// What fontconfig maps the `monospace` alias to, as WebKitGTK does.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn system_mono() -> Option<String> {
    let output = std::process::Command::new("fc-match")
        .args(["-f", "%{family[0]}", "monospace"])
        .output()
        .ok()?;
    let name = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (output.status.success() && !name.is_empty()).then_some(name)
}

#[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
fn system_mono() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::pick_mono;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn prefers_the_web_stack_order() {
        let installed = names(&["Ubuntu", "DejaVu Sans Mono", "JetBrainsMono Nerd Font Mono"]);
        assert_eq!(
            pick_mono(&installed, || None).as_deref(),
            Some("JetBrainsMono Nerd Font Mono")
        );
    }

    #[test]
    fn falls_back_to_the_installed_system_alias() {
        let installed = names(&["Ubuntu", "Iosevka"]);
        assert_eq!(
            pick_mono(&installed, || Some("Iosevka".into())).as_deref(),
            Some("Iosevka")
        );
        assert_eq!(pick_mono(&installed, || Some("Missing".into())), None);
    }
}
