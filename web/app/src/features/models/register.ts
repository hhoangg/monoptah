// Slot registration of WS-T (U-07). Imported for its side effects by one line in src/main.tsx
// (the slot registration contract of workstreams.md 3.2): `import "@/features/models/register";`
import { registerSlot } from "@/ui/slots";
import { DefaultModelSection } from "./DefaultModelSection";

registerSlot("desktop-settings-sections", DefaultModelSection);
