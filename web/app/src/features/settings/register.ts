// Slot registrations of WS-S (W-11). Imported for its side effects by one line in src/main.tsx
// (the slot registration contract of workstreams.md 3.2).
import { registerSlot } from "@/ui/slots";
import { FirstRunPicker } from "./FirstRunPicker";

registerSlot("first-run-picker", FirstRunPicker);
