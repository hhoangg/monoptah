// A small `models.list` catalog for the tests of this feature (no network).
import type { ModelCatalog } from "@monoptah/protocol";

export const CLAUDE_EFFORT = {
  id: "effort",
  label: "Effort",
  kind: "select" as const,
  value: "medium",
  options: [
    { value: "low", label: "Low" },
    { value: "medium", label: "Medium" },
    { value: "high", label: "High" },
    { value: "xhigh", label: "Extra high" },
    { value: "max", label: "Max" },
  ],
};

export function makeCatalog(runtimeMode: string = "supervised"): ModelCatalog {
  return {
    harnesses: [
      { id: "claude", label: "Claude" },
      { id: "codex", label: "Codex" },
    ],
    models: [
      {
        id: "opus",
        harness: "claude",
        name: "Opus",
        provider: "Anthropic",
        settings: [CLAUDE_EFFORT, { id: "fast", label: "Fast mode", kind: "toggle" as const, value: "false", options: [{ value: "false", label: "Off" }, { value: "true", label: "On" }] }],
      },
      { id: "sonnet", harness: "claude", name: "Sonnet", provider: "Anthropic", settings: [CLAUDE_EFFORT] },
      {
        id: "gpt-5",
        harness: "codex",
        name: "GPT-5",
        provider: "OpenAI",
        settings: [{ id: "reasoningEffort", label: "Reasoning", kind: "select" as const, value: "medium", options: [{ value: "low", label: "Low" }, { value: "medium", label: "Medium" }, { value: "high", label: "High" }] }],
      },
    ],
    runtimeModes: [
      { id: "supervised", label: "Supervised", hint: "Ask first." },
      { id: "auto", label: "Auto", hint: "Reviewer." },
    ],
    accounts: [
      { id: "default", provider: "claude", label: "Personal" },
      { id: "work", provider: "claude", label: "Work" },
    ],
    selectedAccounts: { claude: "default" },
    current: { harness: "claude", model: "opus", modelSettings: { effort: "medium", fast: "false" }, runtimeMode: runtimeMode as ModelCatalog["current"]["runtimeMode"] },
  };
}
