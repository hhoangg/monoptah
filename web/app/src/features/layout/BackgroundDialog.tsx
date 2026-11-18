// The background picker of the current project in a dialog (a sheet below 640px): opened from the project bar or the
// command palette, so the first photo of a project can be added without the project's new session screen.
import { useEffect } from "react";
import { useCatalog } from "@/data";
import { BackgroundControls } from "@/features/backgrounds";
import { ResponsiveOverlay } from "@/ui/ResponsiveOverlay";
import { usePanels } from "./panels";
import { closeBackgroundDialog, useBackgroundDialog } from "./projectTools";

export function BackgroundDialog() {
  const requested = useBackgroundDialog((state) => state.target);
  const scope = usePanels((state) => state.scope);
  // The dialog belongs to the project it was opened for: leaving that project's screens closes it.
  const target = requested && scope && scope.desktopId === requested.desktopId && scope.projectId === requested.projectId ? requested : null;
  useEffect(() => {
    if (requested && !target) closeBackgroundDialog();
  }, [requested, target]);
  const catalog = useCatalog();
  const name = target ? (catalog.data?.projects.find((project) => project.id === target.projectId)?.name ?? "this project") : "";
  return (
    <ResponsiveOverlay
      open={!!target}
      onOpenChange={(next) => (next ? undefined : closeBackgroundDialog())}
      title="Background"
      description="A photo shown behind an empty session of this project. It is saved on this device only."
    >
      {target ? <BackgroundControls projectId={target.projectId} projectName={name} /> : null}
    </ResponsiveOverlay>
  );
}
