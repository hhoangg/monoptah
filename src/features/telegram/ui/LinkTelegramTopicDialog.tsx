import { useCallback, useEffect, useRef, useState } from "react";
import { copyText } from "../../../platform/tauri/clipboard";
import { Check, Copy, Loader } from "../../../shared/ui/icons";
import { Modal } from "../../../shared/ui/Modal";
import { SecondaryButton } from "../../../shared/ui/SecondaryButton";
import {
  telegramLinkCode,
  telegramSessionTopic,
  type LinkCode,
} from "../model/telegram";

type Props = {
  project: string;
  sessionId: string;
  sessionTitle: string;
  onClose: () => void;
};

const POLL_MS = 3000;
/** How long the app accepts a link code. */
const CODE_TTL_MS = 15 * 60_000;

function errorText(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

export function LinkTelegramTopicDialog({
  project,
  sessionId,
  sessionTitle,
  onClose,
}: Props) {
  const [link, setLink] = useState<LinkCode | null>(null);
  const [expired, setExpired] = useState(false);
  const [linked, setLinked] = useState(false);
  const [copied, setCopied] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const requestCode = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      const next = await telegramLinkCode(project, sessionId);
      if (!alive.current) return;
      setLink(next);
      setExpired(false);
    } catch (cause) {
      if (alive.current) setError(errorText(cause));
    } finally {
      if (alive.current) setBusy(false);
    }
  }, [project, sessionId]);

  useEffect(() => {
    void requestCode();
  }, [requestCode]);

  useEffect(() => {
    if (!link) return;
    const timer = window.setTimeout(() => setExpired(true), CODE_TTL_MS);
    return () => window.clearTimeout(timer);
  }, [link]);

  // A successful /link changes the stored binding, so compare with the topic
  // the session had when the dialog opened.
  useEffect(() => {
    let disposed = false;
    let timer: number | undefined;
    const start = async () => {
      let baseline: number | null;
      try {
        baseline = await telegramSessionTopic(project, sessionId);
      } catch (cause) {
        if (!disposed) setError(errorText(cause));
        return;
      }
      if (disposed) return;
      timer = window.setInterval(() => {
        void telegramSessionTopic(project, sessionId).then(
          (topic) => {
            if (disposed || topic == null || topic === baseline) return;
            window.clearInterval(timer);
            setLinked(true);
          },
          (cause) => {
            if (!disposed) setError(errorText(cause));
          },
        );
      }, POLL_MS);
    };
    void start();
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [project, sessionId]);

  const copyCommand = async (text: string) => {
    try {
      await copyText(text);
    } catch (cause) {
      if (alive.current) setError(errorText(cause));
      return;
    }
    if (!alive.current) return;
    setCopied(true);
    window.setTimeout(() => {
      if (alive.current) setCopied(false);
    }, 1500);
  };

  return (
    <Modal
      title="Link Telegram topic"
      description={`Use an existing topic for ${sessionTitle}`}
      size="sm"
      fitViewport
      onClose={onClose}
    >
      <div className="flex flex-col gap-4 p-4">
        {linked ? (
          <>
            <p className="text-[13px] font-medium text-content">Linked</p>
            <p className="text-[12px] leading-relaxed text-content/45">
              Questions from this session will be posted in that topic.
            </p>
            <div>
              <SecondaryButton onClick={onClose}>Done</SecondaryButton>
            </div>
          </>
        ) : link ? (
          <div className="flex flex-col gap-2">
            <p className="text-[12px] leading-relaxed text-content/65">
              Send this in the topic you want to use:
            </p>
            <div className="flex items-center gap-2">
              <code className="min-w-0 flex-1 select-text break-all rounded-md border border-content/10 px-2 py-1 text-[12px] text-content">
                {link.command}
              </code>
              <SecondaryButton
                aria-label="Copy command"
                onClick={() => void copyCommand(link.command)}
              >
                {copied ? (
                  <Check className="size-3.5" aria-hidden />
                ) : (
                  <Copy className="size-3.5" aria-hidden />
                )}
                {copied ? "Copied" : "Copy"}
              </SecondaryButton>
            </div>
            {expired ? (
              <div className="flex items-center gap-3">
                <p className="text-[12px] text-amber-600 dark:text-amber-300">
                  This code has expired.
                </p>
                <SecondaryButton
                  onClick={() => void requestCode()}
                  disabled={busy}
                >
                  New code
                </SecondaryButton>
              </div>
            ) : (
              <p className="text-[11px] leading-relaxed text-content/45">
                Waiting for the command… The code works for 15 minutes.
              </p>
            )}
          </div>
        ) : error ? null : (
          <p className="flex items-center gap-2 text-[12px] text-content/45">
            <Loader className="size-3.5 animate-spin" aria-hidden />
            Creating a code…
          </p>
        )}

        {error ? (
          <div className="flex items-center gap-3">
            <p role="alert" className="text-[12px] text-red-400/90">
              {error}
            </p>
            {link || linked ? null : (
              <SecondaryButton
                onClick={() => void requestCode()}
                disabled={busy}
              >
                Try again
              </SecondaryButton>
            )}
          </div>
        ) : null}
      </div>
    </Modal>
  );
}
