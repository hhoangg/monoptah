// The app's Trusted Types `default` policy first, before any other module runs (security.md 8.2).
import "../../../../app/src/trusted-types-init.ts";
import { createRoot } from "react-dom/client";
import { App } from "./App";

createRoot(document.getElementById("root")!).render(<App />);
