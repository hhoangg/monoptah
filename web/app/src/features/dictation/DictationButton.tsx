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
 */
export function DictationButton({ onText, lang, disabled, className }: DictationButtonProps) {
  const dictation = useDictation({ onText, lang });
  if (!dictation.supported) return null;
  return (
    <>
      <Button
        type="button"
        variant="circle"
        size="circle"
        className={className}
        aria-label={dictation.listening ? "Stop dictation" : "Start dictation"}
        aria-pressed={dictation.listening}
        data-on={dictation.listening}
        disabled={disabled}
        onClick={dictation.toggle}
      >
        <Icon name="mic" size={18} />
      </Button>
      <span role="status" aria-live="polite" className="sr-only">
        {dictation.error ?? (dictation.listening ? dictation.interim || "Listening" : "")}
      </span>
    </>
  );
}
