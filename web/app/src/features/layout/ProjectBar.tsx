// The project controls from 640px, never above the screen (WS-T screens have a fixed height, so a bar in the page
// flow would push their composer off the screen). Where the shell draws the sidebar they sit in its foot (icons
// only on the collapsed rail); on the screens that draw their own sidebar (home, project, cloud: they scroll with
// the page) they sit above the page. They: toggle the Files, Changes and Git panels and open
// the background picker for the project of the current screen. Each panel button is hidden when the
// session-capabilities predicate refuses it. Below 640px the command palette carries the same actions.
import { Icon, type IconName } from "@/ui/Icon";
import { PANEL_LABELS, useProjectTools } from "./projectTools";
import type { PanelKind } from "./panels";

const ICONS: Record<PanelKind, IconName> = { files: "folder", changes: "edit", git: "branch" };

const button =
  "inline-flex min-h-11 items-center gap-1.5 rounded-pill px-3 text-label text-muted-text hover:bg-selection aria-pressed:bg-selection aria-pressed:text-content pointer-fine:min-h-9";

const railButton =
  "inline-flex size-11 items-center justify-center rounded-pill text-muted-text hover:bg-selection aria-pressed:bg-selection aria-pressed:text-content pointer-fine:size-9";

export function ProjectBar({ variant = "bar" }: { variant?: "bar" | "foot" | "rail" }) {
  const tools = useProjectTools();
  if (!tools.scope) return null;
  return (
    <div
      role="toolbar"
      aria-label="Project tools"
      data-testid="project-bar"
      data-variant={variant}
      className={
        variant === "bar" ? "flex items-center justify-end gap-1 px-4 pt-2" : variant === "foot" ? "flex flex-wrap items-center gap-1 border-t border-stroke px-3 py-2" : "flex flex-col items-center gap-1"
      }
    >
      {tools.panels.map((kind) => (
        <button key={kind} type="button" aria-label={PANEL_LABELS[kind]} data-panel-trigger="" aria-pressed={tools.isOpen(kind)} onClick={() => tools.toggle(kind)} className={variant === "rail" ? railButton : button}>
          <Icon name={ICONS[kind]} size={18} />
          {variant === "rail" ? null : PANEL_LABELS[kind]}
        </button>
      ))}
      <button type="button" aria-label="Background" onClick={tools.openBackground} className={variant === "rail" ? railButton : button}>
        <Icon name="image" size={18} />
        {variant === "rail" ? null : "Background"}
      </button>
    </div>
  );
}
