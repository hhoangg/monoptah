// External links (security.md section 8, rule 2): a click shows where the link really goes (the host and the
// full address) next to the text it was written with, and opens the address only after the user confirms.
// The anchor keeps its href, so a middle click or "open in new tab" works as before: the allowlist is unchanged.
import { useState, type ReactNode } from "react";
import { ConfirmDialog } from "@/ui/ConfirmDialog";
import type { Inline } from "./parser";

/** The text of inline nodes as the reader sees it, without markup. */
export function inlineText(nodes: readonly Inline[]): string {
  let out = "";
  for (const node of nodes) {
    if (node.type === "text" || node.type === "code") out += node.text;
    else out += inlineText(node.children);
  }
  return out;
}

export interface ExternalLinkProps {
  href: string;
  insecure: boolean;
  /** The link text as written, shown in the sheet. */
  label: string;
  children: ReactNode;
}

/** The confirm sheet of an external link: the real host, the full address and the text it was written with. */
export function ExternalLinkSheet({ href, insecure, label, onOpenChange }: { href: string; insecure: boolean; label: string; onOpenChange: (open: boolean) => void }) {
  let host = href;
  try {
    host = new URL(href).host;
  } catch {
    // classifyLink only passes parseable addresses; keep the whole address if that ever changes.
  }
  const shownLabel = label.length > 200 ? `${label.slice(0, 200)}...` : label;
  return (
    <ConfirmDialog
      open
      onOpenChange={onOpenChange}
      title="Open this link?"
      confirmLabel="Open link"
      description={
        <span className="block space-y-2 text-left">
          <span className="block">
            It goes to <strong data-testid="external-link-host">{host}</strong>
          </span>
          <span className="block break-all font-mono text-caption" data-testid="external-link-url">
            {href}
          </span>
          {shownLabel && shownLabel !== href ? (
            <span className="block text-muted-text">
              The link says: <bdi data-testid="external-link-label">{shownLabel}</bdi>
            </span>
          ) : null}
          {insecure ? <span className="block text-deletions">This address is not encrypted (http).</span> : null}
        </span>
      }
      onConfirm={() => {
        onOpenChange(false);
        window.open(href, "_blank", "noopener,noreferrer");
      }}
    />
  );
}

export function ExternalLink({ href, insecure, label, children }: ExternalLinkProps) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <a
        className="md-link"
        href={href}
        target="_blank"
        rel="noopener noreferrer nofollow"
        data-insecure={insecure || undefined}
        title={insecure ? "Not encrypted (http)" : undefined}
        onClick={(event) => {
          // A plain left click confirms first; modified clicks keep the browser's own behaviour.
          if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
          event.preventDefault();
          setOpen(true);
        }}
      >
        {children}
      </a>
      {open ? <ExternalLinkSheet href={href} insecure={insecure} label={label} onOpenChange={setOpen} /> : null}
    </>
  );
}
