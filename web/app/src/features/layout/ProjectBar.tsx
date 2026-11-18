// The project controls at the top of the main pane from 640px: toggle the Files, Changes and Git panels and open
// the background picker for the project of the current screen. Each panel button is hidden when the
// session-capabilities predicate refuses it. Below 640px the command palette carries the same actions.
import { Icon, type IconName } from "@/ui/Icon";
import { PANEL_LABELS, useProjectTools } from "./projectTools";
import type { PanelKind } from "./panels";

const ICONS: Record<PanelKind, IconName> = { files: "folder", changes: "edit", git: "branch" };

const button =
  "inline-flex min-h-11 items-center gap-1.5 rounded-pill px-3 text-label text-muted-text hover:bg-selection aria-pressed:bg-selection aria-pressed:text-content pointer-fine:min-h-9";

export function ProjectBar() {
  const tools = useProjectTools();
  if (!tools.scope) return null;
  return (
    <div role="toolbar" aria-label="Project tools" data-testid="project-bar" className="flex items-center justify-end gap-1 px-4 pt-2">
      {tools.panels.map((kind) => (
        <button key={kind} type="button" data-panel-trigger="" aria-pressed={tools.isOpen(kind)} onClick={() => tools.toggle(kind)} className={button}>
          <Icon name={ICONS[kind]} size={18} />
          {PANEL_LABELS[kind]}
        </button>
      ))}
      <button type="button" onClick={tools.openBackground} className={button}>
        <Icon name="image" size={18} />
        Background
      </button>
    </div>
  );
}
