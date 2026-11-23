// Route of /pair (no code): the "Pair a desktop" page outside the settings shell, where leaving settings goes for an
// account with no desktop. It shows the steps and the paste field that continues to /pair/:code.
import { createFileRoute } from "@tanstack/react-router";
import { PairDesktopPage } from "@/features/desktops/PairDesktop";

export const Route = createFileRoute("/pair/")({
  component: PairDesktopPage,
});
