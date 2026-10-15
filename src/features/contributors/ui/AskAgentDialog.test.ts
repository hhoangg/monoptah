// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AskAgentRequest, AskScope } from "../model/askAgent";
import { AskAgentDialog } from "./AskAgentDialog";
import type { AskAgentDialogProps } from "./AskAgentDialog";
import { leaderboardModel } from "./leaderboardFixture";

const model = leaderboardModel();
const ana = model.leaderboard.rows.find((r) => r.name === "Ana")!;
const bob = model.leaderboard.rows.find((r) => r.name === "Bob")!;

let container: HTMLDivElement;
let root: Root;

const deferred = <T = void>() => {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
};

async function mount(props: Partial<AskAgentDialogProps> = {}) {
  const onStart = vi.fn<(r: AskAgentRequest) => Promise<void>>(async () => {});
  const onClose = vi.fn();
  await act(async () => {
    root.render(
      createElement(AskAgentDialog, {
        model,
        project: "/work/monoptah",
        scope: { kind: "team" } as AskScope,
        onStart,
        onClose,
        ...props,
      }),
    );
  });
  return { onStart, onClose };
}

const q = (selector: string) => document.querySelector<HTMLElement>(selector);
const all = (selector: string) =>
  Array.from(document.querySelectorAll<HTMLElement>(selector));
const byText = (selector: string, text: string) =>
  all(selector).find((el) => (el.textContent ?? "").includes(text));
const button = (name: string) => {
  const el = byText("button", name);
  if (!el) throw new Error(`no button ${name}`);
  return el as HTMLButtonElement;
};
const textarea = () => q("textarea") as HTMLTextAreaElement;
const checkbox = (label: string) =>
  byText("label", label)!.querySelector("input") as HTMLInputElement;
const chip = (id: string) => q(`[data-chip="${id}"]`);
const removeButton = (id: string) =>
  chip(id)!.querySelector("button") as HTMLButtonElement;

async function type(value: string) {
  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(
      HTMLTextAreaElement.prototype,
      "value",
    )!.set!;
    setter.call(textarea(), value);
    textarea().dispatchEvent(new Event("input", { bubbles: true }));
  });
}

const click = (el: HTMLElement) => act(async () => el.click());

const start = async (
  onStart: ReturnType<typeof mount> extends Promise<infer R>
    ? R["onStart"]
    : never,
) => {
  await click(button("Start session"));
  const request = onStart.mock.calls[0][0];
  return { request, snapshot: JSON.parse(request.snapshotJson) };
};

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.clearAllMocks();
  vi.unstubAllGlobals();
});

describe("AskAgentDialog header and context", () => {
  it("is a titled dialog with the scope as its description", async () => {
    await mount();
    expect(q('[role="dialog"]')?.textContent).toContain("Ask Agent");
    expect(q('[role="dialog"]')?.textContent).toContain("about the whole team");
    expect(q("textarea")?.getAttribute("placeholder")).toBe(
      "Ask anything about this data...",
    );
  });

  it("says 'about your score' for me and the name for a person", async () => {
    await mount({ scope: { kind: "me", personId: ana.personId } });
    expect(q('[role="dialog"]')?.textContent).toContain("about your score");
    expect(chip("focus")?.textContent).toContain("Ana (you)");
    await mount({ scope: { kind: "person", personId: bob.personId } });
    expect(q('[role="dialog"]')?.textContent).toContain("about Bob");
    expect(chip("focus")?.textContent).toContain("Bob (focus)");
    expect(chip("people")?.textContent).toContain("Whole team");
    expect(removeButton("people")).not.toBeNull();
  });

  it("shows the context chips; only the data chips can be removed", async () => {
    await mount();
    expect(
      q('[aria-label="Context sent to the agent"]')?.textContent,
    ).toContain("monoptah");
    for (const id of ["project", "branch", "period"]) {
      expect(chip(id)).not.toBeNull();
      expect(chip(id)!.querySelector("button")).toBeNull();
    }
    for (const id of ["people", "scores", "formula", "ai"]) {
      expect(removeButton(id)).not.toBeNull();
    }
  });

  it("names the snapshot file in the hint", async () => {
    await mount();
    expect(q('[role="dialog"]')?.textContent).toContain(
      `Opens a new session with contributors-${model.period.id}.json attached`,
    );
  });
});

describe("AskAgentDialog snapshot options", () => {
  it("sends everything by default: per-PR on, raw commits off, tools allowed", async () => {
    const { onStart } = await mount();
    expect(checkbox("Per-PR breakdown").checked).toBe(true);
    expect(checkbox("Raw commit list").checked).toBe(false);
    expect(checkbox("Allow the agent to run").checked).toBe(true);
    await type("Who drove this?");
    const { request, snapshot } = await start(onStart);
    expect(snapshot.formula).toBeDefined();
    expect(snapshot.prs.length).toBeGreaterThan(0);
    expect(snapshot.commits).toBeUndefined();
    expect(snapshot.people[0].aiAssistedPct).toBeDefined();
    expect(snapshot.people[0].parts).toBeDefined();
    expect(request.prompt).toContain("read-only git and gh");
  });

  it("removing the formula chip removes the formula from the snapshot", async () => {
    const { onStart } = await mount();
    await click(removeButton("formula"));
    expect(chip("formula")?.getAttribute("data-removed")).toBe("true");
    await type("q");
    const { snapshot } = await start(onStart);
    expect(snapshot.formula).toBeUndefined();
    expect(snapshot.people[0].parts).toBeDefined();
  });

  it("removing the AI-assisted chip removes those fields", async () => {
    const { onStart } = await mount();
    await click(removeButton("ai"));
    await type("q");
    const { snapshot } = await start(onStart);
    expect(snapshot.people.some((p: object) => "aiAssistedPct" in p)).toBe(
      false,
    );
    expect(snapshot.formula).toBeDefined();
  });

  it("removing the scores chip removes the breakdown but keeps score and rank", async () => {
    const { onStart } = await mount();
    await click(removeButton("scores"));
    await type("q");
    const { snapshot } = await start(onStart);
    expect(snapshot.people[0].parts).toBeUndefined();
    expect(snapshot.people[0].breakdown).toBeUndefined();
    expect(snapshot.people[0].score).toBeGreaterThan(0);
  });

  it("removing the people chip drops identities", async () => {
    const { onStart } = await mount();
    await click(removeButton("people"));
    await type("q");
    const { snapshot } = await start(onStart);
    expect(snapshot.people.some((p: object) => "identities" in p)).toBe(false);
  });

  it("removing the people chip leaves no e-mail address in the snapshot or prompt", async () => {
    const scope = { kind: "person", personId: bob.personId } as const;
    const { onStart } = await mount({ scope });
    await click(removeButton("people"));
    await click(checkbox("Raw commit list"));
    await type("q");
    const { request } = await start(onStart);
    expect(request.snapshotJson).not.toContain("@");
    expect(request.prompt).not.toContain("@");
    expect(JSON.parse(request.snapshotJson).scope.personId).toMatch(
      /^person-\d+$/,
    );
    expect(request.scope).toEqual(scope);
  });

  it("a removed chip can be added back", async () => {
    const { onStart } = await mount();
    await click(removeButton("formula"));
    await click(removeButton("formula"));
    expect(chip("formula")?.getAttribute("data-removed")).toBeNull();
    await type("q");
    const { snapshot } = await start(onStart);
    expect(snapshot.formula).toBeDefined();
  });

  it("per-PR off drops prs; raw commits on adds commits", async () => {
    const { onStart } = await mount();
    await click(checkbox("Per-PR breakdown"));
    await click(checkbox("Raw commit list"));
    await type("q");
    const { snapshot } = await start(onStart);
    expect(snapshot.prs).toBeUndefined();
    expect(snapshot.commits.length).toBeGreaterThan(0);
  });

  it("turning tools off changes the prompt wording", async () => {
    const { onStart } = await mount();
    await click(checkbox("Allow the agent to run"));
    await type("q");
    const { request } = await start(onStart);
    expect(request.prompt).toContain("Do not run commands");
    expect(request.prompt).not.toContain("read-only git and gh");
  });

  it("passes the scope, file name and trimmed question through", async () => {
    const scope = { kind: "person", personId: bob.personId } as const;
    const { onStart } = await mount({ scope });
    await type("  Why is Bob low?  ");
    const { request } = await start(onStart);
    expect(request.scope).toEqual(scope);
    expect(request.question).toBe("Why is Bob low?");
    expect(request.fileName).toBe(`contributors-${model.period.id}.json`);
    expect(JSON.parse(request.snapshotJson).focus.name).toBe("Bob");
  });
});

describe("AskAgentDialog suggestions and question", () => {
  it("fills the textarea from a suggestion", async () => {
    await mount();
    const first = q("[data-suggestion]")!;
    await click(first);
    expect(textarea().value).toBe(first.textContent);
    expect(textarea().value.length).toBeGreaterThan(0);
  });

  it("offers different suggestions per scope", async () => {
    await mount();
    const team = all("[data-suggestion]").map((b) => b.textContent);
    expect(team).toContain(
      "Which modules depend on a single person (bus factor)?",
    );
    await mount({ scope: { kind: "me", personId: ana.personId } });
    const me = all("[data-suggestion]").map((b) => b.textContent);
    expect(me).toContain("What would raise my Quality factor?");
    expect(me).not.toContain(team[0]);
    await mount({ scope: { kind: "person", personId: bob.personId } });
    const person = all("[data-suggestion]").map((b) => b.textContent);
    expect(person).toContain("Summarize Bob's contributions this period.");
  });

  it("disables Start session while the question is empty or blank", async () => {
    await mount();
    expect(button("Start session").disabled).toBe(true);
    await type("   ");
    expect(button("Start session").disabled).toBe(true);
    await type("hello");
    expect(button("Start session").disabled).toBe(false);
  });

  it("submits with Cmd+Enter or Ctrl+Enter, not with a plain Enter or when empty", async () => {
    const { onStart } = await mount();
    const press = (init: KeyboardEventInit) =>
      act(async () => {
        textarea().dispatchEvent(
          new KeyboardEvent("keydown", {
            key: "Enter",
            bubbles: true,
            ...init,
          }),
        );
      });
    await press({ metaKey: true });
    expect(onStart).not.toHaveBeenCalled();
    await type("question");
    await press({});
    expect(onStart).not.toHaveBeenCalled();
    await press({ metaKey: true });
    expect(onStart).toHaveBeenCalledTimes(1);
  });

  it("accepts Ctrl+Enter", async () => {
    const { onStart } = await mount();
    await type("question");
    await act(async () => {
      textarea().dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "Enter",
          ctrlKey: true,
          bubbles: true,
        }),
      );
    });
    expect(onStart).toHaveBeenCalledTimes(1);
  });
});

describe("AskAgentDialog starting", () => {
  it("shows Starting... and blocks a second start while pending, then closes", async () => {
    const pending = deferred();
    const onStart = vi.fn(() => pending.promise);
    const { onClose } = await mount({ onStart });
    await type("q");
    await click(button("Start session"));
    expect(button("Starting...").disabled).toBe(true);
    await act(async () => {
      textarea().dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "Enter",
          metaKey: true,
          bubbles: true,
        }),
      );
    });
    expect(onStart).toHaveBeenCalledTimes(1);
    expect(onClose).not.toHaveBeenCalled();
    await act(async () => pending.resolve());
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("shows the rejection inline, keeps the dialog open and allows a retry", async () => {
    const onStart = vi
      .fn<(r: AskAgentRequest) => Promise<void>>()
      .mockRejectedValueOnce(new Error("No provider is available"))
      .mockResolvedValueOnce(undefined);
    const { onClose } = await mount({ onStart });
    await type("q");
    await click(button("Start session"));
    expect(q('[role="alert"]')?.textContent).toBe("No provider is available");
    expect(onClose).not.toHaveBeenCalled();
    expect(button("Start session").disabled).toBe(false);
    expect(textarea().value).toBe("q");
    await click(button("Start session"));
    expect(q('[role="alert"]')).toBeNull();
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("shows a generic message for a rejection without text", async () => {
    const onStart = vi.fn().mockRejectedValue(undefined);
    await mount({ onStart });
    await type("q");
    await click(button("Start session"));
    expect(q('[role="alert"]')?.textContent).toBe(
      "Could not start the session.",
    );
  });

  it("does not touch state after it was unmounted mid-start", async () => {
    const pending = deferred();
    const onClose = vi.fn();
    await mount({ onStart: () => pending.promise, onClose });
    await type("q");
    await click(button("Start session"));
    await act(async () => root.render(createElement("div")));
    await act(async () => pending.resolve());
    expect(onClose).not.toHaveBeenCalled();
  });

  it("cannot be closed while starting: Cancel, Escape, X and the backdrop are inert", async () => {
    const pending = deferred();
    const { onClose } = await mount({ onStart: () => pending.promise });
    await type("q");
    await click(button("Start session"));
    expect(button("Cancel").disabled).toBe(true);
    await click(button("Cancel"));
    await click(q('button[aria-label="Close"]')!);
    await act(async () => {
      q(".modal-backdrop")!.dispatchEvent(
        new MouseEvent("mousedown", { bubbles: true }),
      );
    });
    await act(async () => {
      window.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      );
    });
    expect(onClose).not.toHaveBeenCalled();
    await act(async () => pending.resolve());
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("can be closed again after a failed start", async () => {
    const { onClose } = await mount({
      onStart: async () => {
        throw new Error("nope");
      },
    });
    await type("q");
    await click(button("Start session"));
    expect(button("Cancel").disabled).toBe(false);
    await click(button("Cancel"));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("closes with Escape when idle", async () => {
    const { onClose } = await mount();
    await act(async () => {
      window.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
      );
    });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("Cancel closes without starting", async () => {
    const { onStart, onClose } = await mount();
    await click(button("Cancel"));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(onStart).not.toHaveBeenCalled();
  });
});

describe("AskAgentDialog provider", () => {
  const providers = [
    { id: "claude", label: "Claude Code" },
    { id: "codex", label: "Codex" },
  ];

  it("sends the default provider", async () => {
    const { onStart } = await mount({ providers, defaultProvider: "codex" });
    expect(q('button[aria-label="Agent: Codex"]')).not.toBeNull();
    await type("q");
    const { request } = await start(onStart);
    expect(request.harness).toBe("codex");
  });

  it("sends the provider picked from the list", async () => {
    const { onStart } = await mount({ providers, defaultProvider: "codex" });
    await click(q('button[aria-label="Agent: Codex"]')!);
    await click(byText('[role="option"]', "Claude Code")!);
    await type("q");
    const { request } = await start(onStart);
    expect(request.harness).toBe("claude");
  });

  it("shows a single provider as a label and none as nothing", async () => {
    const { onStart } = await mount({
      providers: [providers[0]],
    });
    expect(q('[role="dialog"]')?.textContent).toContain("Agent: Claude Code");
    expect(q('[aria-haspopup="listbox"]')).toBeNull();
    await type("q");
    expect((await start(onStart)).request.harness).toBe("claude");
    await mount({ providers: [] });
    expect(q('[role="dialog"]')?.textContent).not.toContain("Agent:");
    expect(q('[aria-haspopup="listbox"]')).toBeNull();
  });

  it("omits harness when no provider is known", async () => {
    const { onStart } = await mount();
    await type("q");
    expect((await start(onStart)).request.harness).toBeUndefined();
  });
});
