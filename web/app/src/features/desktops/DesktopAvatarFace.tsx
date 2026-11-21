// What a desktop avatar shows inside its colored shape: the chosen emoji, the uploaded image (an <img>, so no SVG or
// script can run), or the initials. The caller draws the shape, the color and the text color.
import { cn } from "@/lib/utils";
import { desktopAvatarColor, desktopAvatarUrl, desktopInitials, onAvatarColor } from "./avatar";
import type { DesktopSummary } from "./api";

export interface AvatarDesktop {
  id: string;
  name: string;
  color: string | null;
  avatar?: DesktopSummary["avatar"];
}

/** The content of the avatar. An image fills the shape (`rounded-[inherit]`, so it follows the current-desktop squaring). */
export function DesktopAvatarFace({ desktop, emojiClass }: { desktop: AvatarDesktop; emojiClass?: string }) {
  const avatar = desktop.avatar;
  if (avatar?.kind === "image") {
    return <img src={desktopAvatarUrl(desktop.id, avatar.hash)} alt="" draggable={false} decoding="async" data-testid="avatar-image" className="absolute inset-0 size-full rounded-[inherit] object-cover" />;
  }
  if (avatar?.kind === "emoji") {
    return (
      <span aria-hidden="true" data-testid="avatar-emoji" className={cn("text-xl leading-none", emojiClass)}>
        {avatar.emoji}
      </span>
    );
  }
  return <span aria-hidden="true">{desktopInitials(desktop.name)}</span>;
}

/** A small round avatar for a list row or a top bar (the full-size one is the rail's). */
export function DesktopAvatarBubble({ desktop, className }: { desktop: AvatarDesktop; className?: string }) {
  const background = desktopAvatarColor(desktop.id, desktop.color);
  return (
    <span
      aria-hidden="true"
      data-testid="avatar-bubble"
      className={cn("relative inline-flex size-7 shrink-0 items-center justify-center rounded-pill text-meta font-semibold", className)}
      style={{ backgroundColor: background, color: onAvatarColor(background) }}
    >
      <DesktopAvatarFace desktop={desktop} emojiClass="text-base" />
    </span>
  );
}
