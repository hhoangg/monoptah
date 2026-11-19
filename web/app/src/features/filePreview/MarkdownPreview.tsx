// A markdown file, rendered by the app's own renderer (an AST, no HTML sink). Relative images are fetched through
// `chat.mentionedFile` (`asset`) and shown from blob: URLs; a remote image is never loaded (security.md section 8).
import { useQuery } from "@tanstack/react-query";
import { useCallback, useRef, useState } from "react";
import { Markdown } from "@/features/markdown/Markdown";
import { Notice } from "@/ui/Notice";
import { PreviewBudget, assetPath, imageMime, readMention, type MentionRequester } from "./mentionedFile";
import { useBlobUrl } from "./useMentionedFile";

interface ImageProps {
  src: string;
  alt: string;
  path: string;
  request: MentionRequester;
  budget: PreviewBudget;
  cacheKey: readonly unknown[];
  onLimited: () => void;
}

function MarkdownImage({ src, alt, path, request, budget, cacheKey, onLimited }: ImageProps) {
  const asset = assetPath(src);
  const query = useQuery({
    queryKey: [...cacheKey, "asset", asset],
    enabled: asset !== null,
    networkMode: "always",
    retry: false,
    gcTime: 30_000,
    structuralSharing: false,
    queryFn: async ({ signal }) => {
      try {
        const content = await readMention(request, { path, asset: asset as string }, { signal, budget });
        const mime = imageMime(content.meta.mime);
        if (content.meta.kind !== "image" || !mime) throw new Error("Not an image");
        return { mime, chunks: content.chunks as BlobPart[] };
      } catch (error) {
        if (budget.limited) onLimited();
        throw error;
      }
    },
  });
  if (!query.data) return <span data-testid="preview-md-image-missing">{alt || "image"}</span>;
  return <LoadedImage chunks={query.data.chunks} mime={query.data.mime} alt={alt} />;
}

function LoadedImage({ chunks, mime, alt }: { chunks: BlobPart[]; mime: string; alt: string }) {
  const url = useBlobUrl(chunks, mime);
  return <img src={url} alt={alt} data-testid="preview-md-image" className="my-2 block max-w-full" />;
}

export function MarkdownPreview({ text, path, request, cacheKey }: { text: string; path: string; request: MentionRequester; cacheKey: readonly unknown[] }) {
  const budget = useRef<PreviewBudget | null>(null);
  budget.current ??= new PreviewBudget();
  const [limited, setLimited] = useState(false);
  const shared = budget.current;
  const onLimited = useCallback(() => setLimited(true), []);
  const renderImage = useCallback(
    (src: string, alt: string) => <MarkdownImage src={src} alt={alt} path={path} request={request} budget={shared} cacheKey={cacheKey} onLimited={onLimited} />,
    [path, request, shared, cacheKey, onLimited],
  );
  return (
    <div data-testid="preview-markdown" className="min-h-0 flex-1 overflow-auto px-1 py-1">
      {limited ? (
        <div data-testid="preview-limit-notice" className="mb-3">
          <Notice>Some images are not shown because this file has more than the preview can load.</Notice>
        </div>
      ) : null}
      <Markdown text={text} renderImage={renderImage} />
    </div>
  );
}
