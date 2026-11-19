// Route of /recover. Owner: WS-S (workstreams.md 3.1). Also the forced "add a passkey" step of a recoveryOnly session.
import { createFileRoute } from "@tanstack/react-router";
import { RecoverScreen } from "@/features/accounts/RecoverScreen";

export const Route = createFileRoute("/recover")({
  component: RecoverScreen,
});
