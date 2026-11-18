import { useCallback, useEffect, useState } from "react";
import { Button } from "@/ui/button";
import { Icon } from "@/ui/Icon";
import { useDictation } from "./useDictation";

export interface DictationButtonProps {
  /** Receives each finished phrase. The button never sends a message itself. */
  onText: (text: string) => void;
  lang?: string;
  disabled?: boolean;
  className?: string;
}

/**
 * Microphone toggle for the composer (F-49). Renders nothing where the Web Speech API is missing.
 * Speech recognition needs a user gesture and often a network, so a failure shows as a short notice.
 * The button keeps one name ("Dictation") and says whether it is on with `aria-pressed`; the live region speaks only
 * state changes and finished phrases, never the interim words.
 */
export function DictationButton({ onText, lang, disabled, className }: DictationButtonProps) {
  const [announcement, setAnnouncement] = useState("");
  const insert = useCallback(
    (text: string) => {
      onText(text);
      setAnnouncement(`Added: ${text}`);
    },
    [onText],
  );
  const dictation = useDictation({ onText: insert, lang });
  const { listening, stop } = dictation;

  // Said on a change only, not on the first render (state kept from the last render, adjusted while rendering).
  const [wasListening, setWasListening] = useState(false);
  if (listening !== wasListening) {
    setWasListening(listening);
    setAnnouncement(listening ? "Listening" : "Dictation stopped");
  }

  // A composer that locks (a send in flight, the link going read-only) must not keep listening behind a dead button.
  useEffect(() => {
    if (disabled && listening) stop();
  }, [disabled, listening, stop]);

  if (!dictation.supported) return null;
  return (
    <>
      <Button
        type="button"
        variant="circle"
        size="circle"
        className={className}
        aria-label="Dictation"
        aria-pressed={listening}
        data-on={listening}
        disabled={disabled}
        onClick={dictation.toggle}
      >
        <Icon name="mic" size={18} />
      </Button>
      {dictation.error ? (
        <span role="alert" className="max-w-48 text-meta text-deletions">
          {dictation.error}
        </span>
      ) : null}
      <span role="status" aria-live="polite" className="sr-only">
        {announcement}
      </span>
    </>
  );
}
