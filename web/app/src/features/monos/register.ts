// Registrations of the Monos feature, imported for its side effects by src/main.tsx.
import { onForgetDesktop } from "@/features/settings/desktopLink";
import { forgetReactions } from "./reactions";

// "Forget this device" and a revoked desktop wipe the phone-local emoji reactions kept for it (F-71).
onForgetDesktop((desktopId) => forgetReactions(desktopId));
