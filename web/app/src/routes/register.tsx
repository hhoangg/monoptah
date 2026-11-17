// Route of /register. Owner: WS-S (workstreams.md 3.1). The invite code arrives in the URL fragment.
import { createFileRoute } from "@tanstack/react-router";
import { RegisterScreen } from "@/features/accounts/RegisterScreen";

export const Route = createFileRoute("/register")({
  component: RegisterScreen,
});
