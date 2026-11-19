// A markdown file, rendered by the app's own renderer (an AST, no HTML sink). Relative images are fetched through
// `chat.mentionedFile` (`asset`) and shown from data: URIs; a remote image is never loaded (security.md section 8).
import { useQuery } from "@tanstack/react-query";
import { useCallback, useMemo, useState } from "react";
import { Markdown } from "@/features/markdown/Markdown";
import { Notice } from "@/ui/Notice";
import { type PreviewBudget, assetPath, budgetForText, concat, imageDataUri, readMention, type MentionRequester } from "./mentionedFile";

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
        const uri = content.meta.kind === "image" ? imageDataUri(content.meta.mime ?? "", concat(content.chunks, content.length)) : null;
        if (!uri) throw new Error("Not an image");
        return uri;
      } catch (error) {
        if (budget.limited) onLimited();
        throw error;
      }
    },
  });
  if (!query.data) return <span data-testid="preview-md-image-missing">{alt || "image"}</span>;
  return <img src={query.data} alt={alt} data-testid="preview-md-image" className="my-2 block max-w-full" />;
}

export function MarkdownPreview({ text, path, request, cacheKey }: { text: string; path: string; request: MentionRequester; cacheKey: readonly unknown[] }) {
  // The document itself counts against the budget, so its images get only what is left.
  const shared = useMemo(() => budgetForText(text), [text]);
  const [limited, setLimited] = useState(false);
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
