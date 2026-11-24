// A Mono's emoji picture inside its reply (display only: the web never picks, uploads or edits emoji). The desktop
// sends the picks with the reply (`emoji` of a text item, see turnItems.ts) and answers `mono.emoji` with the
// image of one hash. The hash is the sha256 of the file, so an image is cached by desktop and hash for good.
import { useQuery } from "@tanstack/react-query";
import type { MonoEmojiPick } from "@monoptah/protocol";
import { LINK_QUERY_DEFAULTS } from "@/data";
import { getLinkConnection, useLinkDesktopId } from "@/link";

export const EMOJI_INLINE_PX = 20;
export const EMOJI_LARGE_PX = 56;
const MIME_TYPES = new Set(["image/png", "image/jpeg", "image/gif", "image/webp"]);
const BASE64 = /^[A-Za-z0-9+/]*={0,2}$/;
/** An image never changes under its hash: keep it a day in memory. */
const EMOJI_GC_TIME_MS = 24 * 60 * 60_000;
/** Codes of a desktop that will not answer differently next time: the picture is simply not there. */
const FINAL_CODES = new Set(["not_found", "too_large", "not_selected", "invalid_params"]);

/** `null`: the desktop has no such picture for this Mono (nothing is drawn, as native draws nothing). */
export type EmojiImage = { src: string } | null;

function useEmojiImage(monoId: string, hash: string) {
  const desktopId = useLinkDesktopId();
  return useQuery<EmojiImage>({
    ...LINK_QUERY_DEFAULTS,
    gcTime: EMOJI_GC_TIME_MS,
    queryKey: ["link", desktopId, "mono.emoji", monoId, hash],
    queryFn: async ({ signal }) => {
      try {
        const result = await getLinkConnection(desktopId).request("mono.emoji", { monoId, hash }, { signal });
        if (!MIME_TYPES.has(result.mime) || !BASE64.test(result.data)) return null;
        return { src: `data:${result.mime};base64,${result.data}` };
      } catch (error) {
        const code = (error as { code?: unknown } | null)?.code;
        if (typeof code === "string" && FINAL_CODES.has(code)) return null;
        throw error;
      }
    },
  });
}

/** The picture of one pick: an empty box of its size while loading, nothing when the desktop has none. */
export function MonoEmojiImage({ monoId, pick }: { monoId: string; pick: MonoEmojiPick }) {
  const image = useEmojiImage(monoId, pick.hash);
  const size = pick.large ? EMOJI_LARGE_PX : EMOJI_INLINE_PX;
  // A large picture takes its own line of the paragraph (native draws a token alone on its line large); a small one flows with the text.
  const placement = pick.large ? "block" : "inline-block align-text-bottom";
  if (image.isPending) return <span aria-hidden="true" data-testid="mono-emoji-pending" className={placement} style={{ width: size, height: size }} />;
  if (!image.data) return null;
  return (
    <img
      src={image.data.src}
      alt={pick.emotion}
      title={`${pick.name} · ${pick.emotion}`}
      width={size}
      height={size}
      draggable={false}
      data-testid="mono-emoji"
      data-large={pick.large ? "true" : undefined}
      className={`${placement} object-contain`}
    />
  );
}
