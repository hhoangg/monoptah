// U-06: fills the `changes` and `git` right panels. Imported for its side effects by
// features/layout/register.ts. Entry points consult the session-capabilities predicate through
// `openPanel(kind, target, context)` (panels.ts) and the routes themselves.
import { registerPanel } from "@/features/layout/panels";
import { ChangesPanel, GitPanel } from "./panels";

registerPanel("changes", { title: "Changes", component: ChangesPanel });
registerPanel("git", { title: "Git", component: GitPanel });
