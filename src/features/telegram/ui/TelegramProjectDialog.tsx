import { useEffect, useRef, useState, type ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { copyText } from "../../../platform/tauri/clipboard";
import { Check, Copy, Loader } from "../../../shared/ui/icons";
import { Modal } from "../../../shared/ui/Modal";
import { SecondaryButton } from "../../../shared/ui/SecondaryButton";
import { Toggle } from "../../settings/ui/primitives";
import {
  telegramConnectLink,
  telegramDisconnect,
  telegramRemoveWebhook,
  telegramSetEnabled,
  telegramSetToken,
  telegramStatus,
  type ConnectLink,
  type TelegramStatus,
} from "../model/telegram";

type Props = {
  project: string;
  name: string;
  onClose: () => void;
};

const POLL_MS = 3000;
const WEBHOOK_PROBLEM = "This bot has a webhook";
const ADMIN_NOTE =
  "Make the bot an admin with Manage topics so it can create a topic per session.";

function errorText(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

export function TelegramProjectDialog({ project, name, onClose }: Props) {
  const [status, setStatus] = useState<TelegramStatus | null>(null);
  const [token, setToken] = useState("");
  const [link, setLink] = useState<ConnectLink | null>(null);
  const [copied, setCopied] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // A poll that started before a change must not overwrite the change.
  const revision = useRef(0);
  const alive = useRef(true);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  useEffect(() => {
    const refresh = async () => {
      const started = revision.current;
      try {
        const next = await telegramStatus(project);
        if (alive.current && revision.current === started) setStatus(next);
      } catch (cause) {
        if (alive.current && revision.current === started)
          setError(errorText(cause));
      }
    };
    void refresh();
    const timer = window.setInterval(() => void refresh(), POLL_MS);
    return () => window.clearInterval(timer);
  }, [project]);

  const run = async (action: () => Promise<void>) => {
    if (busy) return;
    revision.current += 1;
    setBusy(true);
    setError(null);
    try {
      await action();
    } catch (cause) {
      if (alive.current) setError(errorText(cause));
    } finally {
      if (alive.current) setBusy(false);
    }
  };

  const saveToken = () =>
    run(async () => {
      const next = await telegramSetToken(project, token.trim());
      if (!alive.current) return;
      setToken("");
      setStatus(next);
    });

  const connect = () =>
    run(async () => {
      const next = await telegramConnectLink(project);
      if (alive.current) setLink(next);
    });

  const setEnabled = (enabled: boolean) =>
    run(async () => {
      const next = await telegramSetEnabled(project, enabled);
      if (alive.current) setStatus(next);
    });

  const removeWebhook = () =>
    run(async () => {
      await telegramRemoveWebhook(project);
      const next = await telegramStatus(project);
      if (alive.current) setStatus(next);
    });

  const disconnect = () =>
    run(async () => {
      await telegramDisconnect(project);
      if (!alive.current) return;
      setLink(null);
      setToken("");
      setStatus(await telegramStatus(project));
    });

  const copyFallback = async (text: string) => {
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
      title="Telegram"
      description={`Answer questions from ${name} in a Telegram group`}
      size="sm"
      fitViewport
      onClose={onClose}
    >
      <div className="flex flex-col gap-5 p-4">
        <p className="text-[12px] leading-relaxed text-content/45">
          Questions that wait more than 5 minutes are sent to this group. Any
          member can answer.
        </p>

        {status === null ? (
          <p className="text-[12px] text-content/45">Checking Telegram…</p>
        ) : !status.configured ? (
          <form
            onSubmit={(event) => {
              event.preventDefault();
              if (token.trim()) void saveToken();
            }}
            className="flex flex-col gap-3"
          >
            <label className="flex flex-col gap-1 text-[12px] text-content/65">
              Bot token
              <input
                aria-label="Bot token"
                type="password"
                value={token}
                onChange={(event) => setToken(event.target.value)}
                placeholder="123456:ABC..."
                disabled={busy}
                autoComplete="off"
                spellCheck={false}
                className="h-8 w-full rounded-md border border-content/10 bg-transparent px-2 text-content outline-none focus:border-content/20"
              />
            </label>
            <div className="flex items-center gap-3">
              <SecondaryButton type="submit" disabled={busy || !token.trim()}>
                {busy ? (
                  <Loader className="size-3.5 animate-spin" aria-hidden />
                ) : null}
                Save
              </SecondaryButton>
              <button
                type="button"
                onClick={() =>
                  void openUrl("https://t.me/BotFather").catch(() => undefined)
                }
                className="text-[12px] text-content/65 hover:text-content"
              >
                Create a bot with BotFather
              </button>
            </div>
          </form>
        ) : (
          <>
            <Row label="Bot">
              <span className="text-[12px] text-content/65">
                @{status.botUsername}
              </span>
            </Row>

            {status.chatId == null ? (
              <div className="flex flex-col gap-3 border-t border-stroke pt-4">
                <SecondaryButton onClick={() => void connect()} disabled={busy}>
                  Connect a group
                </SecondaryButton>
                {link ? (
                  <div className="flex flex-col gap-2">
                    <SecondaryButton
                      onClick={() =>
                        void openUrl(link.url).catch(() => undefined)
                      }
                    >
                      Open Telegram
                    </SecondaryButton>
                    <p className="text-[11px] leading-relaxed text-content/45">
                      Or send this command in the group:
                    </p>
                    <div className="flex items-center gap-2">
                      <code className="min-w-0 flex-1 break-all rounded-md border border-content/10 px-2 py-1 text-[12px] text-content">
                        {link.fallback}
                      </code>
                      <SecondaryButton
                        aria-label="Copy command"
                        onClick={() => void copyFallback(link.fallback)}
                      >
                        {copied ? (
                          <Check className="size-3.5" aria-hidden />
                        ) : (
                          <Copy className="size-3.5" aria-hidden />
                        )}
                        {copied ? "Copied" : "Copy"}
                      </SecondaryButton>
                    </div>
                    <p className="text-[11px] leading-relaxed text-content/45">
                      Waiting for the bot to join a group…
                    </p>
                  </div>
                ) : null}
              </div>
            ) : (
              <>
                <Row label={`Connected to ${status.chatTitle ?? "a group"}`}>
                  <Toggle
                    label="Enabled"
                    on={status.enabled}
                    disabled={busy}
                    onChange={(next) => void setEnabled(next)}
                  />
                </Row>
                <dl className="flex flex-col gap-1 text-[12px]">
                  <StatusLine
                    label="Topics"
                    value={status.isForum}
                    on="On"
                    off="Off"
                  />
                  <StatusLine
                    label="Bot can manage topics"
                    value={status.canManageTopics}
                    on="Yes"
                    off="No"
                  />
                </dl>
                {status.canManageTopics === false ? (
                  <p className="rounded-lg bg-amber-400/10 px-2.5 py-2 text-[11px] leading-4 text-amber-700 dark:text-amber-300">
                    {ADMIN_NOTE}
                  </p>
                ) : null}
              </>
            )}

            {status.problem ? (
              <div className="flex flex-col gap-2 border-t border-stroke pt-4">
                <p className="text-[12px] text-amber-600 dark:text-amber-300">
                  {status.problem}
                </p>
                {status.problem === WEBHOOK_PROBLEM ? (
                  <div>
                    <SecondaryButton
                      onClick={() => void removeWebhook()}
                      disabled={busy}
                    >
                      Remove webhook
                    </SecondaryButton>
                  </div>
                ) : null}
              </div>
            ) : null}

            <div className="border-t border-stroke pt-4">
              <SecondaryButton
                danger
                onClick={() => void disconnect()}
                disabled={busy}
              >
                Disconnect
              </SecondaryButton>
            </div>
          </>
        )}

        {error ? (
          <p role="alert" className="text-[12px] text-red-400/90">
            {error}
          </p>
        ) : null}
      </div>
    </Modal>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-4 border-t border-stroke pt-4 first:border-t-0 first:pt-0">
      <span className="min-w-0 break-words text-[13px] font-medium text-content">
        {label}
      </span>
      {children}
    </div>
  );
}

function StatusLine({
  label,
  value,
  on,
  off,
}: {
  label: string;
  value: boolean | undefined;
  on: string;
  off: string;
}) {
  return (
    <div className="flex items-center justify-between gap-4">
      <dt className="text-content/45">{label}</dt>
      <dd className="text-content/85">
        {value == null ? "Unknown" : value ? on : off}
      </dd>
    </div>
  );
}
