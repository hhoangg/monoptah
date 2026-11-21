// The avatar of a desktop, as the activity rail and the desktop switcher show it: initials (the default), one emoji, or a
// small uploaded picture. It is saved on the account (the Worker), so every web device sees it. The color of the
// avatar is the Color section's. Only the owner of the account reaches this screen.
import { useQueryClient } from "@tanstack/react-query";
import { useRef, useState } from "react";
import { useAction } from "@/features/settings/useAction";
import { cn } from "@/lib/utils";
import { Button } from "@/ui/button";
import { Notice } from "@/ui/Notice";
import { SettingsCard } from "@/ui/SettingsCard";
import { setDesktopAvatar, type DesktopDirectory } from "./api";
import { desktopAvatarColor, onAvatarColor } from "./avatar";
import { AVATAR_IMAGE_MESSAGE, AvatarImageError, prepareAvatarImage } from "./avatarImage";
import { DesktopAvatarFace } from "./DesktopAvatarFace";
import { desktopErrorMessage } from "./parts";

/** The emoji on offer: a fixed set of pictographs, so what is stored is always a single emoji. */
export const AVATAR_EMOJIS: readonly string[] = [
  "\u{1F4BB}", "\u{1F5A5}️", "\u{1F9D1}‍\u{1F4BB}", "\u{1F680}", "\u{1F6E0}️", "⚡", "\u{1F525}", "\u{1F332}",
  "\u{1F30A}", "\u{1F319}", "☀️", "\u{1F419}", "\u{1F98A}", "\u{1F431}", "\u{1F436}", "\u{1F984}",
  "\u{1F916}", "\u{1F47E}", "\u{1F3AE}", "\u{1F3A7}", "☕", "\u{1F34E}", "\u{1F3E0}", "\u{1F3E2}",
];

export function DesktopAvatarSection({ desktop }: { desktop: DesktopDirectory }) {
  const queryClient = useQueryClient();
  const action = useAction(desktopErrorMessage);
  const [pickError, setPickError] = useState<string | null>(null);
  const input = useRef<HTMLInputElement>(null);
  const background = desktopAvatarColor(desktop.id, desktop.color);
  const current = desktop.avatar ?? null;

  async function upload(file: File | undefined) {
    setPickError(null);
    if (!file) return;
    let prepared;
    try {
      prepared = await prepareAvatarImage(file);
    } catch (error) {
      setPickError(AVATAR_IMAGE_MESSAGE[error instanceof AvatarImageError ? error.reason : "unreadable"]);
      return;
    }
    await action.run(() => setDesktopAvatar(queryClient, desktop.id, { kind: "image", mime: prepared.mime, data: prepared.data }));
  }

  return (
    <SettingsCard title="Avatar" footnote="Shown in the desktop list on the left and the desktop switcher, on every device of your account. Without one, the initials are used.">
      <div className="flex flex-col gap-3 px-1.5 py-2">
        <div className="flex items-center gap-3">
          <span
            aria-hidden="true"
            data-testid="avatar-preview"
            className="relative inline-flex size-14 shrink-0 items-center justify-center rounded-pill text-title font-semibold"
            style={{ backgroundColor: background, color: onAvatarColor(background) }}
          >
            <DesktopAvatarFace desktop={desktop} emojiClass="text-3xl" />
          </span>
          <div className="flex flex-wrap gap-2">
            <Button type="button" variant="outline" size="sm" disabled={action.busy} onClick={() => input.current?.click()}>
              Upload image
            </Button>
            <Button type="button" variant="ghost" size="sm" disabled={action.busy || current === null} onClick={() => void action.run(() => setDesktopAvatar(queryClient, desktop.id, null))}>
              Use initials
            </Button>
          </div>
          <input
            ref={input}
            type="file"
            accept="image/png,image/jpeg,image/webp"
            aria-label="Avatar image file"
            className="sr-only"
            tabIndex={-1}
            onChange={(event) => {
              const file = event.target.files?.[0];
              event.target.value = "";
              void upload(file);
            }}
          />
        </div>
        <div role="radiogroup" aria-label="Avatar emoji" className="flex flex-wrap gap-1">
          {AVATAR_EMOJIS.map((emoji) => (
            <label key={emoji} className="relative inline-flex size-11 cursor-pointer items-center justify-center pointer-fine:size-9">
              <input
                type="radio"
                name={`avatar-${desktop.id}`}
                value={emoji}
                checked={current?.kind === "emoji" && current.emoji === emoji}
                disabled={action.busy}
                aria-label={`Emoji ${emoji}`}
                className="peer sr-only"
                onChange={() => void action.run(() => setDesktopAvatar(queryClient, desktop.id, { kind: "emoji", emoji }))}
              />
              <span
                aria-hidden="true"
                className={cn(
                  "inline-flex size-full items-center justify-center rounded-row border-2 border-transparent text-xl hover:bg-selection peer-checked:border-content peer-focus-visible:outline-2 peer-focus-visible:outline-offset-2 peer-focus-visible:outline-brand peer-disabled:opacity-40",
                )}
              >
                {emoji}
              </span>
            </label>
          ))}
        </div>
        {pickError ? <Notice variant="error">{pickError}</Notice> : null}
        {action.error ? <Notice variant="error">{action.error}</Notice> : null}
      </div>
    </SettingsCard>
  );
}
