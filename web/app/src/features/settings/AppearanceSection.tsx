import { useLayoutPrefs, type LayoutMode } from "@/features/layout/layoutPrefs";
import { useAppearance } from "@/lib/appearance";
import { SegmentedControl } from "@/ui/SegmentedControl";
import { cn } from "@/lib/utils";
import { SETTINGS_ANCHOR, SettingsCard } from "@/ui/SettingsCard";
import { useMediaQuery } from "@/ui/useMediaQuery";
import type { Appearance } from "@/styles/themes";

const OPTIONS = [
  { value: "system", label: "System", icon: "appearance" },
  { value: "light", label: "Light", icon: "sun" },
  { value: "dark", label: "Dark", icon: "moon" },
] as const satisfies ReadonlyArray<{ value: Appearance; label: string; icon: "appearance" | "sun" | "moon" }>;

const LAYOUT_OPTIONS = [
  { value: "classic", label: "Classic" },
  { value: "focused", label: "Focused" },
] as const satisfies ReadonlyArray<{ value: LayoutMode; label: string }>;

const TABLET_QUERY = "(min-width: 640px)";
const DESKTOP_QUERY = "(min-width: 1024px)";

/**
 * Layout (like the native Appearance > Layout picker): Classic keeps a project's session list as a column beside the
 * sidebar, Focused leaves it out. A phone has one layout, so the picker is not shown there; between 640 and 1024 px
 * there is no session-list column to switch, so the picker is disabled and says why.
 */
function LayoutPicker() {
  const mode = useLayoutPrefs((state) => state.layoutMode);
  const setMode = useLayoutPrefs((state) => state.setLayoutMode);
  const tablet = useMediaQuery(TABLET_QUERY);
  const desktop = useMediaQuery(DESKTOP_QUERY);
  if (!tablet) return null;
  return (
    <div id="layout" className={cn("flex flex-col gap-2 px-1.5 py-2", SETTINGS_ANCHOR)} data-testid="layout-picker">
      <SegmentedControl label="Layout" options={LAYOUT_OPTIONS} value={mode} onChange={setMode} disabled={!desktop} />
      <p className="text-small text-muted-text">
        {desktop
          ? "Classic shows a project's sessions in a column beside the sidebar. Focused gives the chat the room instead."
          : "The layout applies from 1024 px wide windows. Your choice is kept."}
      </p>
    </div>
  );
}

/** Colour scheme (F-87): System, Light or Dark, and the layout. Stored on this device only (`monoptah.appearance`, `monoptah.layout.mode`). */
export function AppearanceSection() {
  const appearance = useAppearance((state) => state.appearance);
  const setAppearance = useAppearance((state) => state.setAppearance);
  return (
    <SettingsCard title="Appearance" footnote="System follows your device setting. These choices are saved on this device only.">
      <div id="colour-scheme" className={cn(SETTINGS_ANCHOR, "px-1.5 py-2")}>
        <SegmentedControl label="Colour scheme" options={OPTIONS} value={appearance} onChange={setAppearance} />
      </div>
      <LayoutPicker />
    </SettingsCard>
  );
}
