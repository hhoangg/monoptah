// Saving a file the preview already holds. Always a download of `application/octet-stream` through a temporary object URL
// that is only ever the target of an `<a download>` click: the bytes never become a page of this origin, whatever their type
// (an html or svg file is saved, not opened). The name is the file's basename, cleaned.
import { useEffect, useRef, useState } from "react";
import { leafName } from "@/lib/fileKind";
import { Button } from "@/ui/button";
import { Icon } from "@/ui/Icon";

/** A file name that is safe to hand to a download: its last path segment only, no separators, control or reserved characters. */
export function safeDownloadName(raw: string, fallback = "file"): string {
  const reserved = new Set(['"', "*", ":", "<", ">", "?", "|", "/", "\\"]);
  // Control characters, bidi overrides and the characters a file system reserves become "_".
  const cleaned = [...leafName(raw)]
    .map((char) => {
      const code = char.codePointAt(0) as number;
      const bad = code < 0x20 || (code >= 0x7f && code <= 0x9f) || (code >= 0x202a && code <= 0x202e) || (code >= 0x2066 && code <= 0x2069) || reserved.has(char);
      return bad ? "_" : char;
    })
    .join("");
  let name = cleaned.replace(/^[.\s]+/, "").replace(/[.\s]+$/, "");
  if (name.length > 150) {
    const dot = name.lastIndexOf(".");
    const extension = dot > 0 && name.length - dot <= 16 ? name.slice(dot) : "";
    name = name.slice(0, 150 - extension.length) + extension;
  }
  return name || fallback;
}

/** Saves `parts` as `name` (cleaned) through a download, never a navigation. */
export function saveBytes(name: string, parts: BlobPart[]): void {
  const url = URL.createObjectURL(new Blob(parts, { type: "application/octet-stream" }));
  const link = document.createElement("a");
  link.href = url;
  link.download = safeDownloadName(name);
  link.rel = "noopener";
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

/** The bytes of a Blob, to hand to [`saveBytes`]. */
export function blobBytes(blob: Blob): Promise<BlobPart[]> {
  return blob.arrayBuffer().then((buffer) => [new Uint8Array(buffer)]);
}

/** What a lazy Download gets: a signal that ends with the button, and a way to say the desktop is making it wait. */
export interface SaveContext {
  signal: AbortSignal;
  setWaiting: (waiting: boolean) => void;
}

/**
 * A Download button. `parts` is the bytes the preview holds, or a function that gets them when the button is pressed: that one
 * is stopped when the button goes away, and the button says it is preparing the file (or waiting for the desktop) meanwhile.
 */
export function SaveButton({ name, parts, testId = "file-preview-download" }: { name: string; parts: BlobPart[] | ((context: SaveContext) => Promise<BlobPart[]>); testId?: string }) {
  const [state, setState] = useState<"idle" | "busy" | "waiting" | "failed">("idle");
  const controller = useRef<AbortController | null>(null);
  useEffect(() => () => controller.current?.abort(), []);
  return (
    <Button
      type="button"
      variant="outline"
      size="sm"
      data-testid={testId}
      data-state={state}
      disabled={state === "busy" || state === "waiting"}
      onClick={() => {
        if (typeof parts !== "function") return saveBytes(name, parts);
        setState("busy");
        const mine = new AbortController();
        controller.current = mine;
        parts({ signal: mine.signal, setWaiting: (waiting) => !mine.signal.aborted && setState(waiting ? "waiting" : "busy") }).then(
          (got) => {
            if (mine.signal.aborted) return;
            setState("idle");
            saveBytes(name, got);
          },
          () => !mine.signal.aborted && setState("failed"),
        );
      }}
    >
      <Icon name="download" size={16} />
      {state === "busy" ? "Preparing..." : state === "waiting" ? "Waiting for your desktop..." : state === "failed" ? "Download failed, try again" : "Download"}
    </Button>
  );
}
