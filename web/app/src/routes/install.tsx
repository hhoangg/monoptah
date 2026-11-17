// Route of /install (public). Owner: WS-S (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { InstallGuide } from "@/features/settings/InstallGuide";

export const Route = createFileRoute("/install")({
  component: InstallGuide,
});
