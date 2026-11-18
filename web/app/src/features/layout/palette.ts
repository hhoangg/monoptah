// Open state of the command palette (F-15). `openCommandPalette()` is for any button (the sidebar
// header, a mobile header); Mod+K toggles it (shortcuts.ts).
import { create } from "zustand";

export const useCommandPalette = create<{ open: boolean }>(() => ({ open: false }));

export function openCommandPalette(): void {
  useCommandPalette.setState({ open: true });
}
export function closeCommandPalette(): void {
  useCommandPalette.setState({ open: false });
}
export function toggleCommandPalette(): void {
  useCommandPalette.setState((state) => ({ open: !state.open }));
}
export function isCommandPaletteOpen(): boolean {
  return useCommandPalette.getState().open;
}
