// Dictation (F-49) over the Web Speech API. Ported in spirit from mobile/src/features/sessions/DictationBar.tsx:
// text only, never sends anything: the caller decides what to do with `onText`.
import { useCallback, useEffect, useRef, useState } from "react";

/** The cap of one dictation, as on mobile. */
export const DICTATION_MAX_MS = 5 * 60_000;

interface RecognitionAlternative {
  transcript: string;
}
interface RecognitionResult {
  readonly isFinal: boolean;
  readonly length: number;
  [index: number]: RecognitionAlternative;
}
interface RecognitionEvent {
  readonly resultIndex: number;
  readonly results: { readonly length: number; [index: number]: RecognitionResult };
}
interface RecognitionErrorEvent {
  readonly error: string;
}
/** The slice of `SpeechRecognition` this hook uses (the DOM lib does not ship the type). */
export interface SpeechRecognitionLike {
  lang: string;
  continuous: boolean;
  interimResults: boolean;
  onresult: ((event: RecognitionEvent) => void) | null;
  onerror: ((event: RecognitionErrorEvent) => void) | null;
  onend: (() => void) | null;
  start(): void;
  stop(): void;
  abort(): void;
}
export type SpeechRecognitionCtor = new () => SpeechRecognitionLike;

/** The recognition constructor of this browser, or null (Firefox, many iOS PWAs). */
export function getSpeechRecognition(): SpeechRecognitionCtor | null {
  if (typeof window === "undefined") return null;
  const scope = window as unknown as {
    SpeechRecognition?: SpeechRecognitionCtor;
    webkitSpeechRecognition?: SpeechRecognitionCtor;
  };
  return scope.SpeechRecognition ?? scope.webkitSpeechRecognition ?? null;
}

export interface UseDictationOptions {
  /** Receives each finished phrase; the caller inserts it (the hook never sends a message). */
  onText: (text: string) => void;
  /** BCP 47 tag; defaults to the page language. */
  lang?: string;
}

export interface Dictation {
  supported: boolean;
  listening: boolean;
  /** The phrase being recognised, not final yet. */
  interim: string;
  /** A short message for the last failure, or null. */
  error: string | null;
  start: () => void;
  stop: () => void;
  toggle: () => void;
}

const ERROR_COPY: Record<string, string> = {
  "not-allowed": "Microphone access was denied.",
  "service-not-allowed": "Speech recognition is not allowed here.",
  "audio-capture": "No microphone was found.",
  network: "Speech recognition needs a network connection.",
};

export function useDictation({ onText, lang }: UseDictationOptions): Dictation {
  const [supported] = useState(() => getSpeechRecognition() !== null);
  const [listening, setListening] = useState(false);
  const [interim, setInterim] = useState("");
  const [error, setError] = useState<string | null>(null);
  const recognition = useRef<SpeechRecognitionLike | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const onTextRef = useRef(onText);
  useEffect(() => {
    onTextRef.current = onText;
  });

  const clearTimer = useCallback(() => {
    if (timer.current !== null) clearTimeout(timer.current);
    timer.current = null;
  }, []);

  const stop = useCallback(() => {
    recognition.current?.stop();
  }, []);

  const start = useCallback(() => {
    if (recognition.current) return;
    const Ctor = getSpeechRecognition();
    if (!Ctor) return;
    const instance = new Ctor();
    const pageLang = typeof document !== "undefined" ? document.documentElement.lang : "";
    instance.lang = lang ?? (pageLang || "en-US");
    instance.continuous = true;
    instance.interimResults = true;
    instance.onresult = (event) => {
      let pending = "";
      for (let i = event.resultIndex; i < event.results.length; i += 1) {
        const result = event.results[i];
        const text = result?.[0]?.transcript ?? "";
        if (result?.isFinal) {
          const finished = text.trim();
          if (finished) onTextRef.current(finished);
        } else {
          pending += text;
        }
      }
      setInterim(pending);
    };
    instance.onerror = (event) => {
      if (event.error === "no-speech" || event.error === "aborted") return;
      setError(ERROR_COPY[event.error] ?? "Dictation stopped unexpectedly.");
    };
    instance.onend = () => {
      clearTimer();
      recognition.current = null;
      setListening(false);
      setInterim("");
    };
    recognition.current = instance;
    setError(null);
    try {
      instance.start();
    } catch {
      recognition.current = null;
      setError("Dictation could not start.");
      return;
    }
    setListening(true);
    timer.current = setTimeout(() => instance.stop(), DICTATION_MAX_MS);
  }, [lang, clearTimer]);

  const toggle = useCallback(() => {
    if (recognition.current) stop();
    else start();
  }, [start, stop]);

  useEffect(
    () => () => {
      clearTimer();
      const instance = recognition.current;
      if (instance) {
        instance.onresult = null;
        instance.onerror = null;
        instance.onend = null;
        instance.abort();
        recognition.current = null;
      }
    },
    [clearTimer],
  );

  return { supported, listening, interim, error, start, stop, toggle };
}
