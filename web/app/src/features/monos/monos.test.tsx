// U-09 (P1): the Mono chat, its menu, the Mono home list and the "Monos only" preference, against the real link
// wired to the mock desktop (sessions/dev/mockLink.ts).
import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { FIXTURE_HANDLERS, type MockDesktop } from "@monoptah/mock-desktop";
import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { MemoryPersistAdapter, registerPersistAdapter, type PersistAdapter } from "@/data";
import { resetLinkConnections } from "@/link";
import { clearLinkStores } from "@/link/store.ts";
import { resetSessionActivity } from "@/features/sessions/activityStore";
import { DESKTOP_ID, connectMockDesktop, seedWorkspace, type MockLink, type MockLinkOptions } from "@/features/sessions/dev/mockLink";
import { resetMonoSeen } from "@/features/sessions/monoSeen";
import { stubTranscriptLayout } from "@/features/transcript/layoutStub";
import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { clearHooks } from "@/ui/hooks";
import { clearSlots } from "@/ui/slots";
import { WsPSettingsSections } from "@/features/themes/SettingsSections";
import { MONOS_ONLY_KEY, reloadMonoPrefs, useMonoPrefs } from "./monoPrefs";
import { MONO_ACCENTS_KEY, reloadMonoAccents, setMonoAccent } from "./monoAccent";

beforeAll(() => {
  window.matchMedia = ((query: string) => ({ matches: false, media: query, onchange: null, addEventListener() {}, removeEventListener() {}, addListener() {}, removeListener() {}, dispatchEvent: () => false })) as never;
  stubTranscriptLayout();
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.setPointerCapture ??= () => {};
  Element.prototype.releasePointerCapture ??= () => {};
  Element.prototype.scrollIntoView ??= () => {};
});

const original = { ...FIXTURE_HANDLERS };
let previous: PersistAdapter;
let link: MockLink;
beforeEach(() => {
  localStorage.clear();
  reloadMonoPrefs();
  reloadMonoAccents();
  resetMonoSeen();
  previous = registerPersistAdapter(new MemoryPersistAdapter());
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

export const MONO = "mono-example";

/** A Mono's conversation is the session `mono:<id>` of the mock desktop. */
function seedMonoChat(desktop: MockDesktop) {
  const base = desktop.world.sessions.get("sess-local-1")!;
  desktop.world.sessions.set(`mono:${MONO}`, { ...base, id: `mono:${MONO}`, turns: structuredClone(base.turns), approvals: [], script: [] } as never);
}

function mount(path: string, options: MockLinkOptions = {}) {
  link = connectMockDesktop({
    ...options,
    seed: (desktop) => {
      seedWorkspace(desktop);
      seedMonoChat(desktop);
      options.seed?.(desktop);
    },
  });
  const queryClient = createQueryClient();
  const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [`/d/${DESKTOP_ID}${path}`] }), queryClient });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { router, queryClient };
}

const field = () => screen.findByRole("textbox", { name: "Message" });
const noReset = () => {
  expect(screen.queryByText(/Reset conversation/i)).toBeNull();
  expect(screen.queryByText(/Delete Mono/i)).toBeNull();
};

describe("Mono chat (F-69)", () => {
  it("renders the transcript of mono:<id> under the Mono's name and sends from the composer", async () => {
    const user = userEvent.setup();
    mount(`/monos/${MONO}`);
    expect(await screen.findByRole("heading", { level: 1, name: "Example Mono" })).toBeInTheDocument();
    expect(await screen.findByText("Done. This is canned example output.")).toBeInTheDocument();
    await user.type(await field(), "Hello Mono");
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(JSON.stringify(link.desktop.world.sessions.get(`mono:${MONO}`)!.turns)).toContain("Hello Mono"));
  });

  it("shows the empty state of a Mono with no messages", async () => {
    mount(`/monos/${MONO}`, { seed: (desktop) => void (desktop.world.sessions.get(`mono:${MONO}`)!.turns = []) });
    expect(await screen.findByTestId("mono-empty")).toHaveTextContent("Example Mono");
  });

  it("a read-only device cannot send", async () => {
    mount(`/monos/${MONO}`, { permission: "read" });
    expect(await field()).toBeDisabled();
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
    expect(screen.getByTestId("read-only-chip")).toBeInTheDocument();
  });

  it("draws the chat in the accent picked for this Mono, kept on this browser only", async () => {
    setMonoAccent(DESKTOP_ID, MONO, "violet");
    expect(JSON.parse(localStorage.getItem(MONO_ACCENTS_KEY) ?? "{}")).toEqual({ [`${DESKTOP_ID}:${MONO}`]: "violet" });
    mount(`/monos/${MONO}`);
    const screenRoot = await screen.findByTestId("route-stub");
    expect(screenRoot.style.getPropertyValue("--accent-color")).toMatch(/^#/);
  });
});

describe("Mono menu (F-70)", () => {
  it("offers Details only: reset and delete are never shown, and Settings is not the action", async () => {
    const user = userEvent.setup();
    mount(`/monos/${MONO}`);
    await user.click(await screen.findByRole("button", { name: "Mono actions" }));
    const menu = await screen.findByRole("menu");
    expect(within(menu).getByRole("menuitem", { name: "View details" })).toHaveAttribute("href", `/d/${DESKTOP_ID}/monos/${MONO}/profile`);
    expect(within(menu).getAllByRole("menuitem")).toHaveLength(1);
    noReset();
    expect(screen.queryByText("Settings", { selector: "a" })).toBeNull();
  });

  it("shows a Settings action instead of the menu when monoTools is off", async () => {
    const handler = FIXTURE_HANDLERS["workspace.list"] as (p: never, c: never) => Record<string, unknown>;
    (FIXTURE_HANDLERS as Record<string, unknown>)["workspace.list"] = ((p: never, c: never) => ({ ...handler(p, c), monoTools: false })) as never;
    mount(`/monos/${MONO}`);
    const links = await screen.findAllByRole("link", { name: "Settings" });
    expect(links.some((link) => link.getAttribute("href") === `/d/${DESKTOP_ID}/settings` && link.textContent === "Settings")).toBe(true);
    expect(screen.queryByRole("button", { name: "Mono actions" })).toBeNull();
  });
});

describe("Mono home (F-13)", () => {
  it("lists the Monos newest first with a context menu that has no reset or delete", async () => {
    mount("/monos");
    const rows = await screen.findAllByTestId("mono-row");
    expect(rows.map((row) => row.textContent)).toEqual([expect.stringContaining("Example Mono"), expect.stringContaining("Research Mono")]);
    expect(rows[0]).toHaveAttribute("data-unread", "true");
    fireEvent.contextMenu(rows[0]!);
    const menu = await screen.findByRole("menu");
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual(["Mark as read", "View details", "Hide"]);
    noReset();
  });

  it("marks an idle Mono unread, then read again", async () => {
    const user = userEvent.setup();
    mount("/monos", { seed: (desktop) => void ((desktop.world.monos() as unknown as Array<Record<string, unknown>>)[1]!.status = "idle") });
    const research = (await screen.findAllByTestId("mono-row")).find((row) => row.textContent?.includes("Research Mono"))!;
    expect(research).not.toHaveAttribute("data-unread");
    fireEvent.contextMenu(research);
    await user.click(await screen.findByRole("menuitem", { name: "Mark as unread" }));
    await waitFor(() => expect(research).toHaveAttribute("data-unread", "true"));
    fireEvent.contextMenu(research);
    await user.click(await screen.findByRole("menuitem", { name: "Mark as read" }));
    await waitFor(() => expect(research).not.toHaveAttribute("data-unread"));
  });

  it("hides a Mono and restores it, device-locally", async () => {
    const user = userEvent.setup();
    mount("/monos");
    const research = (await screen.findAllByTestId("mono-row")).find((row) => row.textContent?.includes("Research Mono"))!;
    fireEvent.contextMenu(research);
    await user.click(await screen.findByRole("menuitem", { name: "Hide" }));
    await waitFor(() => expect(screen.queryByText("Research Mono")).toBeNull());
    expect(JSON.parse(localStorage.getItem("monoptah.hiddenMonos") ?? "{}")).toEqual({ [DESKTOP_ID]: ["mono-two"] });
    await user.click(screen.getByRole("button", { name: /Hidden \(1\)/ }));
    await user.click(screen.getByRole("button", { name: "Show Research Mono" }));
    expect(await screen.findByText("Research Mono")).toBeInTheDocument();
  });
});

describe("Monos only setting (F-13)", () => {
  it("is a switch in the WS-P settings group, stored as monoptah.monosOnly", async () => {
    const user = userEvent.setup();
    render(<WsPSettingsSections />);
    const toggle = screen.getByRole("switch", { name: "Monos only" });
    expect(toggle).toHaveAttribute("aria-checked", "false");
    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-checked", "true");
    expect(localStorage.getItem(MONOS_ONLY_KEY)).toBe("1");
    expect(useMonoPrefs.getState().monosOnly).toBe(true);
  });
});
