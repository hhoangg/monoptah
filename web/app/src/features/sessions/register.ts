// Registrations of WS-T (sessions). Imported for its side effects by the WS-T route files, because
// main.tsx belongs to WS-S; a single `import "./features/sessions/register";` line in main.tsx
// would register it before the first render for every route, including `/` (see the report).
import { registerLastPlaceResolver } from "@/data";
import { readLastPlace } from "./lastPlace";

registerLastPlaceResolver(readLastPlace);
