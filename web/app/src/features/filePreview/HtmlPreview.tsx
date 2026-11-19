// An html report in a sandbox. The document is the desktop's sanitized html with its images inlined as data: URIs
// (mentionedFile.ts), loaded from a blob: URL into an iframe with `sandbox=""`: no script, no same-origin, no popups,
// no top navigation, no forms. The app CSP allows `frame-src blob:` for exactly this. The html is never parsed into
// the app's DOM (no innerHTML, no srcdoc).
import { Notice } from "@/ui/Notice";
import { useBlobUrl } from "./useMentionedFile";

export function HtmlPreview({ document: html, name, limited }: { document: string; name: string; limited: boolean }) {
  const url = useBlobUrl([html], "text/html;charset=utf-8");
  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col gap-2" data-testid="preview-html-frame">
      {limited ? (
        <div data-testid="preview-limit-notice" className="shrink-0">
          <Notice>Some images are not shown because this page has more than the preview can load.</Notice>
        </div>
      ) : null}
      <iframe
        data-testid="preview-html"
        title={`Preview of ${name}`}
        src={url}
        sandbox=""
        referrerPolicy="no-referrer"
        className="block min-h-0 w-full min-w-0 max-w-full flex-1 rounded-row border border-stroke bg-[#fff] [color-scheme:light]"
      />
    </div>
  );
}
