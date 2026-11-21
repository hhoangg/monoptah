// A markdown file, rendered by the app's own renderer (an AST, no HTML sink). Relative images are fetched through
// `chat.mentionedFile` (`asset`) and shown from data: URIs; a remote image is never loaded (security.md section 8).
import { useQuery } from "@tanstack/react-query";
import { useCallback, useEffect, useRef, useState } from "react";
import { safeDownloadName } from "./download";
import { ImageLightbox, type LightboxImage } from "./ImageLightbox";
import { useBlobUrl } from "./useMentionedFile";
import { Markdown } from "@/features/markdown/Markdown";
import { assetPath, imageMime, readAsset, type MentionSource } from "./mentionedFile";

interface ImageProps {
  src: string;
  alt: string;
  path: string;
  request: MentionSource;
  cacheKey: readonly unknown[];
  onOpen: (image: LightboxImage) => void;
}

/** True once `ref` is near the viewport (and for ever after); always true where there is no IntersectionObserver. */
function useNearViewport(ref: React.RefObject<HTMLElement | null>): boolean {
  const [near, setNear] = useState(() => typeof IntersectionObserver === "undefined");
  useEffect(() => {
    const element = ref.current;
    if (near || !element) return;
    const observer = new IntersectionObserver((entries) => entries.some((entry) => entry.isIntersecting) && setNear(true), { rootMargin: "400px" });
    observer.observe(element);
    return () => observer.disconnect();
  }, [near, ref]);
  return near;
}

function MarkdownImage({ src, alt, path, request, cacheKey, onOpen }: ImageProps) {
  const asset = assetPath(src);
  const holder = useRef<HTMLSpanElement>(null);
  // Read only when the image is near the screen, so a file with hundreds of images holds what is shown, not everything.
  const near = useNearViewport(holder);
  const query = useQuery({
    queryKey: [...cacheKey, "asset", asset],
    enabled: asset !== null && near,
    networkMode: "always",
    retry: false,
    gcTime: 30_000,
    structuralSharing: false,
    queryFn: async ({ signal }) => {
      const content = await readAsset(request, { path, asset: asset as string }, { signal });
      const mime = content.meta.kind === "image" ? imageMime(content.meta.mime) : null;
      if (!mime) throw new Error("Not an image");
      // A Blob, not a data: URI of its bytes: it does not sit in the JS heap as one long string per image.
      return new Blob(content.chunks as BlobPart[], { type: mime });
    },
  });
  // Loading or failed, the image shows as its text (a failed one says so), never as a broken picture.
  if (!query.data) return <span ref={holder} data-testid="preview-md-image-missing">{query.isError ? `${alt || "image"} (image not loaded)` : alt || "image"}</span>;
  return <BlobImage blob={query.data} alt={alt} name={asset ?? alt} onOpen={onOpen} />;
}

/** An image from a Blob: its object URL is made once and revoked when the image goes away. A click opens it in the zoom viewer. */
function BlobImage({ blob, alt, name, onOpen }: { blob: Blob; alt: string; name: string; onOpen: (image: LightboxImage) => void }) {
  const url = useBlobUrl([blob], blob.type);
  return (
    <button
      type="button"
      aria-label={`Open image ${alt || name}`}
      data-testid="preview-md-image-open"
      className="my-2 block max-w-full cursor-zoom-in"
      // The bytes already loaded for this image: nothing is asked of the desktop again.
      onClick={() => void blob.arrayBuffer().then((buffer) => onOpen({ name: safeDownloadName(name, "image"), mime: blob.type, chunks: [new Uint8Array(buffer)] }))}
    >
      <img src={url} alt={alt} data-testid="preview-md-image" className="block max-w-full" />
    </button>
  );
}

export function MarkdownPreview({ text, path, request, cacheKey }: { text: string; path: string; request: MentionSource; cacheKey: readonly unknown[] }) {
  const [opened, setOpened] = useState<LightboxImage | null>(null);
  const renderImage = useCallback(
    (src: string, alt: string) => <MarkdownImage src={src} alt={alt} path={path} request={request} cacheKey={cacheKey} onOpen={setOpened} />,
    [path, request, cacheKey],
  );
  return (
    <div data-testid="preview-markdown" className="min-h-0 flex-1 overflow-auto px-1 py-1">
      <Markdown text={text} renderImage={renderImage} />
      <ImageLightbox image={opened} onClose={() => setOpened(null)} />
    </div>
  );
}
