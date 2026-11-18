// A Mono's pixel mascot in its colour (F-69 header, F-13 rows, F-76 identity). It reuses the project mascot
// (WS-T, F-91): the sprites are the same set, and the talk frame animates while the Mono works.
import { safeHexColor } from "@/lib/color";
import { ProjectMascot } from "@/features/sessions/ProjectMascot";

export function MonoMascot({
  monoId,
  mascot,
  color,
  working = false,
  size = 28,
  className,
}: {
  monoId: string;
  mascot?: string | null;
  color?: string | null;
  working?: boolean;
  size?: number;
  className?: string;
}) {
  return <ProjectMascot project={monoId} name={mascot} color={safeHexColor(color ?? undefined, "var(--accent-color)")} size={size} active={working} className={className} />;
}
