// Opens a pull request link only through the confirm sheet of external links (W13-7): the address passes the
// link allowlist, the user sees the real host, and only "Open link" calls window.open.
import { useState, type ReactNode } from "react";
import { ExternalLinkSheet } from "@/features/markdown/ExternalLink";
import { safePrUrl } from "./gitModel";

/** `open(value)` shows the sheet and returns true when the address is allowed; `sheet` goes anywhere in the tree. */
export function usePullRequestOpener(): { open: (value: unknown) => boolean; sheet: ReactNode } {
  const [url, setUrl] = useState<string | null>(null);
  return {
    open: (value) => {
      const safe = safePrUrl(value);
      if (safe) setUrl(safe);
      return safe !== null;
    },
    sheet: url ? <ExternalLinkSheet href={url} insecure={false} label="Pull request" onOpenChange={(open) => (open ? undefined : setUrl(null))} /> : null,
  };
}
