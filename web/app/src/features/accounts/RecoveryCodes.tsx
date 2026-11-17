import { useEffect, useRef, useState } from "react";
import { Button } from "@/ui/button";
import { Checkbox } from "@/ui/Checkbox";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { downloadTextFile, recoveryCodesFileName, recoveryCodesText } from "./recoveryCodesFile";

export interface RecoveryCodesProps {
  /** Account handle, used in the downloaded file's name and content. */
  handle: string;
  /** The 10 codes. Keep them in component state only; never in web storage or the query cache. */
  codes: readonly string[];
  /** Called when the user ticks "I saved these codes" and presses Continue. */
  onContinue: () => void;
  continueLabel?: string;
  /** Disables Continue while the parent is busy. */
  busy?: boolean;
}

/**
 * Shared recovery codes view (register, regenerate in settings, forced regenerate): shown once,
 * Copy, Download, Print, and a Continue that stays disabled until the "I saved these codes" box is ticked.
 * The server records the acknowledgement itself when it issues the codes (register or regenerate),
 * so the gate here is client-side only: the contract has no route to say "I saved these".
 */
export function RecoveryCodes({ handle, codes, onContinue, continueLabel = "Continue", busy = false }: RecoveryCodesProps) {
  const [saved, setSaved] = useState(false);
  const [copyState, setCopyState] = useState<"idle" | "copied" | "failed">("idle");
  const copyTimer = useRef<number | undefined>(undefined);

  useEffect(() => () => window.clearTimeout(copyTimer.current), []);

  async function copy() {
    window.clearTimeout(copyTimer.current);
    try {
      await navigator.clipboard.writeText(recoveryCodesText(handle, codes));
      setCopyState("copied");
    } catch {
      setCopyState("failed");
    }
    copyTimer.current = window.setTimeout(() => setCopyState("idle"), 2500);
  }

  return (
    <div className="flex flex-col gap-4" data-testid="recovery-codes">
      <Notice variant="info" title="Shown only once">
        Each code signs you in once if you lose your passkeys. Save them now; Monoptah cannot show them again.
      </Notice>
      <div data-print-area className="flex flex-col gap-2">
        <p className="hidden text-body font-semibold print:block">{`Monoptah recovery codes for ${handle}`}</p>
        {/* One column: a code is 19 characters and must never wrap in the middle. */}
        <ol aria-label="Recovery codes" className="grid list-none grid-cols-1 gap-2 overflow-x-auto rounded-row bg-fill p-3">
          {codes.map((code, index) => (
            <li key={code} className="flex items-baseline gap-2 font-mono text-label tabular-nums whitespace-nowrap text-content">
              <span aria-hidden="true" className="w-5 shrink-0 text-right text-caption text-muted-text">
                {index + 1}
              </span>
              <span>{code}</span>
            </li>
          ))}
        </ol>
      </div>
      <div className="flex flex-wrap gap-2">
        <Button type="button" variant="outline" className="flex-1" onClick={() => void copy()}>
          <Icon name={copyState === "copied" ? "check" : "copy"} size={18} />
          {copyState === "copied" ? "Copied" : "Copy"}
        </Button>
        <Button type="button" variant="outline" className="flex-1" onClick={() => downloadTextFile(recoveryCodesFileName(handle), recoveryCodesText(handle, codes))}>
          <Icon name="download" size={18} />
          Download
        </Button>
        <Button type="button" variant="outline" className="flex-1" onClick={() => window.print()}>
          <Icon name="print" size={18} />
          Print
        </Button>
      </div>
      <p role="status" className="min-h-5 text-meta text-muted-text">
        {copyState === "copied" ? "Codes copied to the clipboard." : copyState === "failed" ? "Copy is not available here. Use Download instead." : ""}
      </p>
      <Checkbox checked={saved} onChange={(event) => setSaved(event.target.checked)} label="I saved these codes" />
      <Button type="button" size="lg" className="w-full" disabled={!saved || busy} onClick={onContinue}>
        {continueLabel}
      </Button>
    </div>
  );
}
