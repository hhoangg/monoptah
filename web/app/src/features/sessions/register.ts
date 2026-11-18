// Registrations of WS-T (sessions). Imported for its side effects by the WS-T route files, because
// main.tsx belongs to WS-S; a single `import "./features/sessions/register";` line in main.tsx
// would register it before the first render for every route, including `/` (see the report).
import { registerLastPlaceResolver } from "@/data";
import "@/features/models/register";
import { onForgetDesktop } from "@/features/settings/desktopLink";
import { forgetDesktop, readLastPlace } from "./lastPlace";
import { forgetMonoSeen } from "./monoSeen";

registerLastPlaceResolver(readLastPlace);
// "Forget this device" wipes what the sessions screens keep per desktop: the last place and the Mono seen map.
onForgetDesktop((desktopId) => {
  forgetDesktop(desktopId);
  return forgetMonoSeen(desktopId);
});
