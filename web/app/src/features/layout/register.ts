// Slot registrations of WS-P (responsive layouts). Imported for its side effects by one line in
// src/main.tsx (the slot registration contract of workstreams.md 3.2).
//
// Panel contents (files, changes, git) and the quick-open shortcut are registered by the tickets that
// build them (U-05, U-06) through `registerPanel` (./panels) and `registerShortcut` (./shortcuts).
import { registerSlot } from "@/ui/slots";
import { AppShell } from "./AppShell";
// The WS-P settings groups (themes and accents, U-03) fill `settings-sections` from their own register.
import "@/features/themes/register";
// The Files panel and the Mod+P quick-open shortcut (U-05).
import "@/features/files/register";

registerSlot("app-shell", AppShell);
// The Changes and Git panels (U-06).
import "@/features/changes/register";
