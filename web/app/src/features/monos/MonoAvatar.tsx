// A Mono's avatar wherever the web lists or signs it: the pixel mascot the desktop picked, in the desktop's color (the same
// avatar the native app draws). A desktop that does not send a known mascot (an older build, or a name this web does not
// have) gets the initial in a colored tile instead.
import { safeHexColor } from "@/lib/color";
import { cn } from "@/lib/utils";
import { projectColor } from "@/features/sessions/projectColors";
import { MONO_MASCOT_NAMES } from "./monoLook";
import { MonoMascot } from "./MonoMascot";

/** True when `mascot` is one of the sprites this web draws. */
export function isKnownMascot(mascot: string | null | undefined): mascot is string {
  return !!mascot && MONO_MASCOT_NAMES.includes(mascot);
}

/** The color of a Mono: the desktop's `#rrggbb`, else a stable one from its name. */
export function monoColor(name: string, color: string | null | undefined): string {
  return safeHexColor(color ?? undefined, projectColor(name));
}

export function MonoAvatar({
  monoId,
  name,
  mascot,
  color,
  size = 40,
  working = false,
  className,
}: {
  monoId: string;
  name: string;
  mascot?: string | null;
  color?: string | null;
  size?: number;
  working?: boolean;
  className?: string;
}) {
  const tint = monoColor(name, color);
  if (isKnownMascot(mascot)) {
    return (
      <span aria-hidden="true" data-testid="mono-avatar" data-kind="mascot" className={cn("inline-flex shrink-0 items-center justify-center", className)} style={{ width: size, height: size }}>
        <MonoMascot monoId={monoId} mascot={mascot} color={tint} working={working} size={size} />
      </span>
    );
  }
  return (
    <span
      aria-hidden="true"
      data-testid="mono-avatar"
      data-kind="initial"
      className={cn("flex shrink-0 items-center justify-center rounded-pill font-semibold text-white", className)}
      style={{ width: size, height: size, backgroundColor: tint, fontSize: Math.round(size * 0.4) }}
    >
      {(name.trim().charAt(0) || "M").toUpperCase()}
    </span>
  );
}
