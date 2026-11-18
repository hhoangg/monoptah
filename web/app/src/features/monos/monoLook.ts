// What a Mono can look like (F-75, F-78): the desktop's nine swatches (mobile/src/features/workspace/monoLook.ts)
// and the ten mascots of the project mascot set (the same names the desktop uses for Monos).
import { PROJECT_MASCOTS } from "@/features/sessions/projectMascots";

export const MONO_SWATCHES = ["#459cf7", "#ea603e", "#f4bf25", "#39c363", "#e45a9f", "#b168d4", "#37beb4", "#ef8c39", "#8b63e9"] as const;

export const MONO_MASCOT_NAMES: readonly string[] = PROJECT_MASCOTS.map((mascot) => mascot.name);

export const MONO_NAME_MAX = 40;

/** The look a new Mono starts with: a pick the user keeps or changes. */
export function randomLook(random: () => number = Math.random): { mascot: string; color: string } {
  const mascot = MONO_MASCOT_NAMES[Math.floor(random() * MONO_MASCOT_NAMES.length)] ?? "ghost";
  const color = MONO_SWATCHES[Math.floor(random() * MONO_SWATCHES.length)] ?? MONO_SWATCHES[0];
  return { mascot, color };
}
