// U-05: fills the `files` right panel and the Mod+P quick-open shortcut. Imported for its side effects by
// features/layout/register.ts. Entry points consult the session-capabilities predicate through
// `openPanel(kind, target, context)` (panels.ts) and the route itself.
import { registerPanel } from "@/features/layout/panels";
import { FilesPanel } from "./FilesPanel";
import { registerQuickOpen } from "./quickOpen";

registerPanel("files", { title: "Files", component: FilesPanel });
registerQuickOpen();
