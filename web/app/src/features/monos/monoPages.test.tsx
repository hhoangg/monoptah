// U-09 (P2): profile, documents, habit, document and Soul/Memory viewer, edit and create, against the real link
// wired to the mock desktop. `mono.*` handlers are replaced by fixtures that record every call.
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { FIXTURE_HANDLERS, FixtureError } from "@monoptah/mock-desktop";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { MemoryPersistAdapter, registerPersistAdapter, type PersistAdapter } from "@/data";
import { resetLinkConnections } from "@/link";
import { clearLinkStores } from "@/link/store.ts";
import { resetSessionActivity } from "@/features/sessions/activityStore";
import { DESKTOP_ID, connectMockDesktop, seedWorkspace, type MockLinkOptions } from "@/features/sessions/dev/mockLink";
import { resetMonoSeen } from "@/features/sessions/monoSeen";
import { stubTranscriptLayout } from "@/features/transcript/layoutStub";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { clearHooks } from "@/ui/hooks";
import { clearSlots } from "@/ui/slots";
import { MONO_ACCENTS_KEY, reloadMonoAccents } from "./monoAccent";
import { MONO_MASCOT_NAMES } from "./monoLook";
import { reloadMonoPrefs } from "./monoPrefs";
import { NAME_AUTOSAVE_MS } from "./MonoEditScreen";

beforeAll(() => {
  window.matchMedia = ((query: string) => ({ matches: false, media: query, onchange: null, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {}, dispatchEvent: () => false })) as never;
  stubTranscriptLayout();
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.setPointerCapture ??= () => {};
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
});

const original = { ...FIXTURE_HANDLERS };
const handlers = FIXTURE_HANDLERS as unknown as Record<string, (p: never, c: never) => unknown>;
let calls: Array<{ method: string; params: Record<string, unknown> }>;
let previous: PersistAdapter;

const NOW = Date.now();
const PROFILE = {
  status: "Idle · last active 2h ago",
  state: "idle",
  contributions: { total: 1234, streak: 5, levels: Array.from({ length: 26 * 7 }, (_, i) => i % 5) },
  today: { merged: 3, additions: 1200, deletions: 80, sessions: 4, habitsRan: 1, habitsTotal: 2 },
  upNext: [{ id: "h1", name: "Morning digest", schedule: "Every day 8:00", when: "in 3h", running: false }],
  habits: [
    {
      id: "h1",
      name: "Morning digest",
      instructions: "Write a **digest** of yesterday.",
      schedule: "Every day 8:00",
      enabled: true,
      when: "in 3h",
      running: false,
      lastOutcome: "ok",
      lastError: null,
      runs: [{ at: NOW - 3_600_000, durationMs: 75_000, outcome: "ok", report: "Sent 3 notes", error: null }],
    },
  ],
  documents: Array.from({ length: 6 }, (_, i) => ({ id: `doc-${i}`, title: `Doc ${i}`, kind: i === 1 ? "visual" : "note", summary: "", updatedAt: NOW - i * 3_600_000 })),
};

beforeEach(() => {
  localStorage.clear();
  reloadMonoPrefs();
  reloadMonoAccents();
  resetMonoSeen();
  previous = registerPersistAdapter(new MemoryPersistAdapter());
  calls = [];
  const record = (method: string) => (p: never, c: never) => {
    calls.push({ method, params: p as Record<string, unknown> });
    return (original as unknown as Record<string, (p: never, c: never) => unknown>)[method]!(p, c);
  };
  for (const method of ["mono.update", "mono.create", "mono.saveFile", "mono.reset", "mono.delete"]) handlers[method] = record(method);
  handlers["mono.profile"] = (p: never) => {
    calls.push({ method: "mono.profile", params: p as Record<string, unknown> });
    return PROFILE;
  };
  handlers["mono.file"] = (p: never) => ({ text: `Text of ${(p as { file: string }).file}`, hash: "h" });
  handlers["mono.document"] = (p: never) => {
    const id = (p as { documentId: string }).documentId;
    if (id === "doc-1") {
      return { title: "Visual one", kind: "visual", body: "", tree: { kind: "gauge", label: "CPU", value: 40, min: 0, max: 100, color: "accent" } };
    }
    return { title: "Notes", kind: "note", body: "# Heading\n\nSome **bold** text <img src=x onerror=alert(1)>" };
  };
});
afterEach(() => {
  Object.assign(FIXTURE_HANDLERS, original);
  resetLinkConnections();
  clearLinkStores();
  registerPersistAdapter(previous);
  resetSessionActivity();
  clearSlots();
  clearHooks();
  localStorage.clear();
});

function mount(path: string, options: MockLinkOptions = {}) {
  connectMockDesktop({ ...options, seed: (desktop) => { seedWorkspace(desktop); options.seed?.(desktop); } });
  const queryClient = createQueryClient();
  const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [`/d/${DESKTOP_ID}${path}`] }), queryClient });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { router };
}

const MONO = "mono-example";
const called = (method: string) => calls.filter((call) => call.method === method);

describe("profile (F-76)", () => {
  it("renders identity, today tiles, contributions, up next, habits, documents and projects from the fixture", async () => {
    mount(`/monos/${MONO}/profile`);
    expect(await screen.findByRole("heading", { level: 1, name: "Profile" })).toBeInTheDocument();
    expect(await screen.findByText("PRs merged")).toBeInTheDocument();
    expect(screen.getByText("+1,200")).toBeInTheDocument();
    expect(screen.getByText("1/2")).toBeInTheDocument();
    expect(screen.getByText("1,234")).toBeInTheDocument();
    expect(screen.getByText("5-day streak")).toBeInTheDocument();
    expect(screen.getByRole("img", { name: /Contributions/ })).toBeInTheDocument();
    const upNext = screen.getByRole("heading", { name: "Up next" }).closest("section")!;
    expect(within(upNext).getByRole("link", { name: /Morning digest/ })).toHaveAttribute("href", `/d/${DESKTOP_ID}/monos/${MONO}/profile/habit/h1`);
    // Five newest documents and a "See all" row for the sixth.
    expect(screen.getByRole("link", { name: "Open Doc 0" })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Open Doc 5" })).toBeNull();
    expect(screen.getByRole("link", { name: "See all 6 documents" })).toHaveAttribute("href", `/d/${DESKTOP_ID}/monos/${MONO}/profile/documents`);
    const projects = screen.getByRole("heading", { name: "Projects" }).closest("section")!;
    expect(within(projects).getAllByRole("listitem").length).toBeGreaterThan(0);
    expect(screen.getByRole("link", { name: "Edit" })).toBeInTheDocument();
  });

  it("is allowed for a read-only device, without Edit, and has a refresh button", async () => {
    const user = userEvent.setup();
    mount(`/monos/${MONO}/profile`, { permission: "read" });
    expect(await screen.findByText("PRs merged")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Edit" })).toBeNull();
    const before = called("mono.profile").length;
    await user.click(screen.getByRole("button", { name: "Refresh" }));
    await waitFor(() => expect(called("mono.profile").length).toBeGreaterThan(before));
  });

  it("lists all documents on the documents page, newest first", async () => {
    mount(`/monos/${MONO}/profile/documents`);
    expect(await screen.findByRole("heading", { level: 1, name: "Documents" })).toBeInTheDocument();
    const links = await screen.findAllByRole("link", { name: /^Open Doc/ });
    expect(links.map((link) => link.getAttribute("aria-label"))).toEqual(["Open Doc 0", "Open Doc 1", "Open Doc 2", "Open Doc 3", "Open Doc 4", "Open Doc 5"]);
  });
});

describe("habit (F-77)", () => {
  it("shows schedule, instructions as Markdown and the recent runs", async () => {
    mount(`/monos/${MONO}/profile/habit/h1`);
    expect(await screen.findByRole("heading", { level: 1, name: "Morning digest" })).toBeInTheDocument();
    expect(screen.getByText("Every day 8:00", { selector: "dd" })).toBeInTheDocument();
    expect(screen.getByText("digest", { selector: "strong" })).toBeInTheDocument();
    expect(screen.getByText(/Sent 3 notes/)).toBeInTheDocument();
    expect(screen.getByText(/1m 15s/)).toBeInTheDocument();
  });

  it("says when the habit is gone", async () => {
    mount(`/monos/${MONO}/profile/habit/nope`);
    expect(await screen.findByText("This habit is no longer there.")).toBeInTheDocument();
  });
});

describe("document page (F-76) and Soul and Memory viewer (F-79)", () => {
  it("renders a Markdown document through the safe renderer", async () => {
    mount(`/monos/${MONO}/profile/file/doc-0`);
    expect(await screen.findByRole("heading", { level: 1, name: "Notes" })).toBeInTheDocument();
    expect(await screen.findByText("Heading")).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
  });

  it("draws a visual document through the SVG renderer", async () => {
    mount(`/monos/${MONO}/profile/file/doc-1`);
    expect(await screen.findByTestId("visual-tree")).toBeInTheDocument();
    expect(screen.getByRole("img", { name: /CPU/ })).toBeInTheDocument();
  });

  it("shows Soul as read-only text with no Save and never calls mono.saveFile", async () => {
    mount(`/monos/${MONO}/profile/file/soul?mind=soul`);
    const field = await screen.findByRole("textbox", { name: "Soul" });
    expect(field).toHaveValue("Text of soul");
    expect(field).toHaveAttribute("readonly");
    expect(screen.queryByRole("button", { name: /save/i })).toBeNull();
    expect(called("mono.saveFile")).toHaveLength(0);
  });

  it.each(["soul", "memory", "Memory.md"])("treats a document called %s as a document, not the Mind viewer", async (name) => {
    mount(`/monos/${MONO}/profile/file/${name}`);
    expect(await screen.findByRole("heading", { level: 1, name: "Notes" })).toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: /Soul|Memory/ })).toBeNull();
  });

  it("opens the Mind viewer from the profile's Soul and Memory links", async () => {
    mount(`/monos/${MONO}/profile`);
    const soul = await screen.findByRole("link", { name: /Soul/ });
    expect(soul).toHaveAttribute("href", `/d/${DESKTOP_ID}/monos/${MONO}/profile/file/soul?mind=soul`);
  });

  it("shows Memory the same way, also for a read-only device", async () => {
    mount(`/monos/${MONO}/profile/file/memory?mind=memory`, { permission: "read" });
    expect(await screen.findByRole("textbox", { name: "Memory" })).toHaveValue("Text of memory");
    expect(screen.queryByRole("button", { name: /save/i })).toBeNull();
  });
});

describe("edit (F-78)", () => {
  it("autosaves the name 600 ms after typing stops with mono.update", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
      mount(`/monos/${MONO}/profile/edit`);
      const field = await screen.findByRole("textbox", { name: "Name" });
      await user.clear(field);
      await user.type(field, "Renamed");
      expect(NAME_AUTOSAVE_MS).toBe(600);
      // Not yet: 300 ms is shorter than the debounce.
      await act(async () => {
        vi.advanceTimersByTime(300);
      });
      expect(called("mono.update")).toHaveLength(0);
      await act(async () => {
        vi.advanceTimersByTime(NAME_AUTOSAVE_MS);
      });
      await waitFor(() => expect(called("mono.update")).toHaveLength(1));
      expect(called("mono.update")[0]?.params).toEqual({ monoId: MONO, name: "Renamed" });
    } finally {
      vi.useRealTimers();
    }
  });

  it("tries a name again after a failed save instead of treating it as saved", async () => {
    const user = userEvent.setup();
    let first = true;
    handlers["mono.update"] = (p: never, c: never) => {
      calls.push({ method: "mono.update", params: p as Record<string, unknown> });
      if (first) {
        first = false;
        throw new FixtureError("Could not reach the desktop");
      }
      return (original as unknown as Record<string, (p: never, c: never) => unknown>)["mono.update"]!(p, c);
    };
    mount(`/monos/${MONO}/profile/edit`);
    const field = await screen.findByRole("textbox", { name: "Name" });
    await user.clear(field);
    await user.type(field, "Renamed");
    await waitFor(() => expect(called("mono.update")).toHaveLength(1), { timeout: 3000 });
    expect(await screen.findByText(/Could not reach the desktop|Could not save/)).toBeInTheDocument();
    // The same name typed again is saved: the failed one was never counted as saved.
    await user.clear(field);
    await user.type(field, "Renamed");
    await waitFor(() => expect(called("mono.update")).toHaveLength(2), { timeout: 3000 });
    expect(called("mono.update")[1]?.params).toEqual({ monoId: MONO, name: "Renamed" });
  });

  it("undoes only the project whose save failed when two toggles overlap", async () => {
    const user = userEvent.setup();
    handlers["mono.update"] = (p: never, c: never) => {
      calls.push({ method: "mono.update", params: p as Record<string, unknown> });
      if ((p as { addProject?: string }).addProject === "proj-gamma") throw new FixtureError("nope");
      return (original as unknown as Record<string, (p: never, c: never) => unknown>)["mono.update"]!(p, c);
    };
    mount(`/monos/${MONO}/profile/edit`);
    const gamma = await screen.findByRole("checkbox", { name: "Gamma Service" });
    const beta = screen.getByRole("checkbox", { name: "Example Beta" });
    await user.click(gamma);
    await user.click(beta);
    await waitFor(() => expect(called("mono.update")).toHaveLength(2));
    await waitFor(() => expect(gamma).not.toBeChecked());
    expect(beta).toBeChecked();
  });

  it("moves between the mascots with the arrow keys", async () => {
    const user = userEvent.setup();
    mount(`/monos/${MONO}/profile/edit`);
    const group = await screen.findByRole("radiogroup", { name: "Mascot" });
    const radios = within(group).getAllByRole("radio");
    const selected = radios.findIndex((radio) => radio.getAttribute("aria-checked") === "true");
    expect(selected).toBeGreaterThanOrEqual(0);
    // One tab stop: the selected radio.
    expect(radios.filter((radio) => radio.tabIndex === 0)).toEqual([radios[selected]]);
    radios[selected]!.focus();
    await user.keyboard("{ArrowRight}");
    const expected = MONO_MASCOT_NAMES[(selected + 1) % MONO_MASCOT_NAMES.length]!;
    await waitFor(() => expect(called("mono.update").map((call) => call.params)).toEqual([{ monoId: MONO, mascot: expected }]));
    expect(within(group).getByRole("radio", { name: expected })).toHaveFocus();
  });

  it("names the colours and moves between them with the arrow keys", async () => {
    const user = userEvent.setup();
    mount(`/monos/${MONO}/profile/edit`);
    const group = await screen.findByRole("radiogroup", { name: "Color" });
    const radios = within(group).getAllByRole("radio");
    expect(radios.map((radio) => radio.getAttribute("aria-label"))).toEqual(["Blue", "Red", "Yellow", "Green", "Pink", "Purple", "Teal", "Orange", "Violet"]);
    // The desktop's own colour is not one of the swatches: the first radio is then the one tab stop.
    expect(radios.filter((radio) => radio.tabIndex === 0)).toEqual([radios[0]]);
    radios[0]!.focus();
    await user.keyboard("{End}");
    await waitFor(() => expect(radios[radios.length - 1]).toHaveAttribute("aria-checked", "true"));
  });

  it("offers Same as the Mono for the sessions it starts, saved as null", async () => {
    const user = userEvent.setup();
    mount(`/monos/${MONO}/profile/edit`, {
      seed: (desktop) => void ((desktop.world.monos() as unknown as Array<Record<string, unknown>>)[0]!.subsessions = { harness: "codex", model: "example-model-2" }),
    });
    const same = await screen.findByRole("button", { name: "Same as Example Mono" });
    expect(same).toHaveAttribute("aria-pressed", "false");
    expect(await screen.findByRole("button", { name: /Sessions it starts: Example Model Two/ })).toBeInTheDocument();
    await user.click(same);
    await waitFor(() => expect(called("mono.update").map((call) => call.params)).toEqual([{ monoId: MONO, subsessions: null }]));
  });

  it("shows Same as the Mono as the choice while no model is set, not the app's current model", async () => {
    mount(`/monos/${MONO}/profile/edit`);
    expect(await screen.findByRole("button", { name: "Same as Example Mono" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "Choose a model for sessions it starts" })).toBeInTheDocument();
  });

  it("saves the mascot, the colour and an assigned project one change at a time", async () => {
    const user = userEvent.setup();
    mount(`/monos/${MONO}/profile/edit`);
    await user.click(await screen.findByRole("radio", { name: "robot" }));
    await user.click(within(screen.getByRole("radiogroup", { name: "Color" })).getByRole("radio", { name: "Red" }));
    await user.click(screen.getByRole("checkbox", { name: "Gamma Service" }));
    await waitFor(() => expect(called("mono.update")).toHaveLength(3));
    expect(called("mono.update").map((call) => call.params)).toEqual([
      { monoId: MONO, mascot: "robot" },
      { monoId: MONO, color: "#ea603e" },
      { monoId: MONO, addProject: "proj-gamma" },
    ]);
    expect(screen.getByRole("radio", { name: "robot" })).toHaveAttribute("aria-checked", "true");
  });

  it("keeps the chat colour on this device only", async () => {
    const user = userEvent.setup();
    mount(`/monos/${MONO}/profile/edit`);
    await user.click(within(await screen.findByRole("radiogroup", { name: "Chat color" })).getByRole("radio", { name: "Violet" }));
    expect(JSON.parse(localStorage.getItem(MONO_ACCENTS_KEY) ?? "{}")).toEqual({ [`${DESKTOP_ID}:${MONO}`]: "violet" });
    expect(called("mono.update")).toHaveLength(0);
  });

  it("is control only: a read-only device sees why and no controls", async () => {
    mount(`/monos/${MONO}/profile/edit`, { permission: "read" });
    expect(await screen.findByText(/Read only: this device can look at/)).toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: "Name" })).toBeNull();
    expect(screen.queryByRole("radio")).toBeNull();
    expect(called("mono.update")).toHaveLength(0);
  });
});

describe("create Mono (F-75)", () => {
  it("creates with mono.create then mono.update and opens the chat", async () => {
    const user = userEvent.setup();
    const { router } = mount("/monos");
    await user.click(await screen.findByRole("button", { name: "New Mono" }));
    const dialog = await screen.findByTestId("create-mono");
    // Ten mascots and nine colours, the colours named and not labelled by their hex.
    expect(within(dialog).getAllByRole("radio").length).toBe(10 + 9);
    expect(within(dialog).getAllByRole("radio").every((radio) => !/^#/.test(radio.getAttribute("aria-label") ?? ""))).toBe(true);
    // A sheet below 640px (the test viewport matches no min-width query).
    expect(dialog.closest("[data-vaul-drawer]")).not.toBeNull();
    const create = within(dialog).getByRole("button", { name: "Create" });
    expect(create).toBeDisabled();
    await user.type(within(dialog).getByRole("textbox", { name: "Name" }), "Scout");
    await user.click(within(dialog).getByRole("radio", { name: "ghost" }));
    await user.click(within(dialog).getByRole("radio", { name: "Green" }));
    await user.click(create);
    await waitFor(() => expect(router.state.location.pathname).toBe(`/d/${DESKTOP_ID}/monos/mono-created`));
    expect(called("mono.create")).toHaveLength(1);
    expect(called("mono.update")[0]?.params).toEqual({ monoId: "mono-created", name: "Scout", mascot: "ghost", color: "#39c363" });
  });

  it("is not offered to a read-only device", async () => {
    mount("/monos", { permission: "read" });
    await screen.findAllByTestId("mono-row");
    expect(screen.queryByRole("button", { name: "New Mono" })).toBeNull();
  });

  it("is not offered when the desktop does not advertise monoTools", async () => {
    const list = original["workspace.list"] as unknown as (p: never, c: never) => Record<string, unknown>;
    handlers["workspace.list"] = (p: never, c: never) => ({ ...list(p, c), monoTools: false });
    mount("/monos");
    await screen.findAllByTestId("mono-row");
    expect(screen.queryByRole("button", { name: "New Mono" })).toBeNull();
    expect(called("mono.create")).toHaveLength(0);
  });
});

describe("denied methods (security.md section 7)", () => {
  const PAGES: Array<[string, string, string | RegExp]> = [
    ["chat", `/monos/${MONO}`, "Example Mono"],
    ["profile", `/monos/${MONO}/profile`, "Profile"],
    ["edit", `/monos/${MONO}/profile/edit`, "Edit Mono"],
    ["file", `/monos/${MONO}/profile/file/soul?mind=soul`, "Soul"],
    ["document", `/monos/${MONO}/profile/file/doc-0`, "Notes"],
    ["habit", `/monos/${MONO}/profile/habit/h1`, "Morning digest"],
    ["documents", `/monos/${MONO}/profile/documents`, "Documents"],
    ["list", "/monos", "Monos"],
  ];

  it.each(PAGES)("never calls mono.reset, mono.delete or mono.saveFile from the %s page", async (_name, path, heading) => {
    mount(path);
    await screen.findByRole("heading", { level: 1, name: heading });
    expect(screen.queryByText(/reset conversation|delete mono|\bsave\b/i)).toBeNull();
    expect(screen.queryByRole("button", { name: /reset|delete|save/i })).toBeNull();
    for (const method of ["mono.reset", "mono.delete", "mono.saveFile"]) expect(called(method)).toHaveLength(0);
  });
});
