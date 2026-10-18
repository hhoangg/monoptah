import { useEffect, useState } from "react";
import { SecondaryButton } from "../../../shared/ui/SecondaryButton";
import {
  getAiConfig,
  setAiConfig,
  type AiConfigStatus,
} from "../../ticket-contributors/model/ticketsApi";

type Runner = AiConfigStatus["runner"];

const RUNNERS: { id: Runner; label: string }[] = [
  { id: "claude-code", label: "Claude Code (your subscription)" },
  { id: "api", label: "Anthropic API key" },
];

export function TicketScoringSettings() {
  const [runner, setRunner] = useState<Runner>("claude-code");
  const [model, setModel] = useState("");
  // The saved key is never read back: the field only ever holds a new key.
  const [apiKey, setApiKey] = useState("");
  const [hasApiKey, setHasApiKey] = useState(false);
  const [checking, setChecking] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void getAiConfig()
      .then((config) => {
        if (cancelled) return;
        setRunner(config.runner);
        setModel(config.model);
        setHasApiKey(config.hasApiKey);
      })
      .catch((err: unknown) => {
        if (!cancelled)
          setError(String(err instanceof Error ? err.message : err));
      })
      .finally(() => {
        if (!cancelled) setChecking(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const save = async (nextKey: string | null) => {
    if (busy || checking) return;
    setBusy(true);
    setError(null);
    try {
      const saved = await setAiConfig(runner, model.trim(), nextKey);
      setRunner(saved.runner);
      setModel(saved.model);
      setHasApiKey(saved.hasApiKey);
      setApiKey("");
    } catch (err) {
      setError(String(err instanceof Error ? err.message : err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="px-4 py-3.5">
      <form
        onSubmit={(event) => {
          event.preventDefault();
          void save(apiKey.trim() ? apiKey.trim() : null);
        }}
        className="flex flex-col gap-3"
      >
        <p className="text-[12px] leading-relaxed text-content/45">
          Choose what scores closed ClickUp tickets on the leaderboard.
        </p>
        <div
          role="radiogroup"
          aria-label="Runner"
          className="flex flex-col gap-2"
        >
          {RUNNERS.map((option) => (
            <label
              key={option.id}
              className="flex items-center gap-2 text-[13px] text-content"
            >
              <input
                type="radio"
                name="ticket-scoring-runner"
                aria-label={option.label}
                checked={runner === option.id}
                onChange={() => setRunner(option.id)}
                disabled={busy || checking}
              />
              {option.label}
            </label>
          ))}
        </div>
        <label className="flex flex-col gap-1 text-[12px] text-content/65">
          Model
          <input
            aria-label="Model"
            type="text"
            value={model}
            onChange={(event) => setModel(event.target.value)}
            placeholder="claude-opus-5-5"
            disabled={busy || checking}
            autoComplete="off"
            spellCheck={false}
            className="h-8 w-full rounded-md border border-content/10 bg-transparent px-2 text-content outline-none focus:border-content/20"
          />
          <span className="text-content/45">
            claude-haiku-5-5 costs far less per ticket
          </span>
        </label>
        {runner === "api" ? (
          <label className="flex flex-col gap-1 text-[12px] text-content/65">
            Anthropic API key
            <input
              aria-label="API key"
              type="password"
              value={apiKey}
              onChange={(event) => setApiKey(event.target.value)}
              placeholder="sk-ant-..."
              disabled={busy || checking}
              autoComplete="off"
              spellCheck={false}
              className="h-8 w-full rounded-md border border-content/10 bg-transparent px-2 text-content outline-none focus:border-content/20"
            />
            {hasApiKey ? (
              <span className="flex items-center gap-3 text-content/45">
                Key saved
                <button
                  type="button"
                  onClick={() => void save("")}
                  disabled={busy}
                  className="text-content/65 hover:text-content"
                >
                  Remove key
                </button>
              </span>
            ) : null}
          </label>
        ) : null}
        <div>
          <SecondaryButton type="submit" disabled={busy || checking}>
            {busy ? "Saving" : "Save"}
          </SecondaryButton>
        </div>
      </form>
      {error ? (
        <p role="alert" className="mt-3 text-[12px] text-red-400/90">
          {error}
        </p>
      ) : null}
    </div>
  );
}
