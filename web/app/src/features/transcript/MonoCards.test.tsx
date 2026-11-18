// F-43 and F-72.
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { TranscriptActionsContext, type TranscriptActions } from "./context";
import { DocumentCard, LaunchedSessions, MonoChoices, MonoHeader } from "./MonoCards";
import type { LaunchedSession } from "./turnItems";

const wrap = (actions: TranscriptActions, node: React.ReactNode) => render(<TranscriptActionsContext value={actions}>{node}</TranscriptActionsContext>);

describe("MonoHeader (F-43)", () => {
  const agent = { name: "Research Mono", color: "#7c5cff" };
  it("signs a finished turn with the Mono's name or the turn's label", () => {
    const { rerender } = render(<MonoHeader item={{ live: false }} agent={agent} />);
    expect(screen.getByTestId("mono-signature")).toHaveTextContent("Research Mono");
    rerender(<MonoHeader item={{ live: false, label: "Notes" }} agent={agent} />);
    expect(screen.getByTestId("mono-signature")).toHaveTextContent("Notes");
  });

  it('says "Thinking..." beside the signature while it works', () => {
    render(<MonoHeader item={{ live: true }} agent={agent} />);
    expect(screen.getByTestId("mono-header-live")).toHaveTextContent("Research Mono");
    expect(screen.getByText("Thinking...")).toBeInTheDocument();
  });

  it("shows the status the desktop sends instead", () => {
    render(<MonoHeader item={{ live: true, label: "Reading the docs" }} agent={agent} />);
    expect(screen.getByText("Reading the docs")).toBeInTheDocument();
    expect(screen.queryByText("Thinking...")).toBeNull();
  });

  it("ignores a colour that is not #rrggbb", () => {
    render(<MonoHeader item={{ live: false }} agent={{ name: "Mono", color: "red; background:url(x)" }} />);
    expect(screen.getByTestId("mono-signature").querySelector("span[aria-hidden]")?.getAttribute("style") ?? "").not.toContain("url");
  });
});

describe("MonoChoices (F-72)", () => {
  it("sends the pressed reply once and checks it", async () => {
    const onChoose = vi.fn().mockResolvedValue(undefined);
    const user = userEvent.setup();
    wrap({ onChoose }, <MonoChoices options={["Yes", "No"]} />);
    await user.click(screen.getByRole("button", { name: "Yes" }));
    await user.click(screen.getByRole("button", { name: "No" }));
    expect(onChoose).toHaveBeenCalledTimes(1);
    expect(onChoose).toHaveBeenCalledWith("Yes");
    expect(screen.getByRole("button", { name: "Yes" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "No" })).toBeDisabled();
  });

  it("checks the reply that followed and steps the rest back", () => {
    wrap({ onChoose: vi.fn() }, <MonoChoices options={["Yes", "No"]} chosen=" No " />);
    expect(screen.getByRole("button", { name: "No" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "Yes" })).toBeDisabled();
  });

  it("tells a send that failed and lets the reader try again", async () => {
    const onChoose = vi.fn().mockRejectedValueOnce(new Error("link down")).mockResolvedValue(undefined);
    const user = userEvent.setup();
    wrap({ onChoose }, <MonoChoices options={["Yes"]} />);
    await user.click(screen.getByRole("button", { name: "Yes" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("link down");
    await user.click(screen.getByRole("button", { name: "Yes" }));
    expect(onChoose).toHaveBeenCalledTimes(2);
  });

  it("is disabled on a read-only host", () => {
    wrap({}, <MonoChoices options={["Yes"]} />);
    expect(screen.getByRole("button", { name: "Yes" })).toBeDisabled();
  });
});

describe("LaunchedSessions (F-72)", () => {
  const session: LaunchedSession = { id: "s1", projectId: "p1", project: "Alpha", title: "Fix the parser", harness: "claude", state: "working" };

  it("opens the session it names", async () => {
    const onOpenSession = vi.fn();
    const user = userEvent.setup();
    wrap({ onOpenSession }, <LaunchedSessions sessions={[session]} />);
    await user.click(screen.getByRole("button", { name: "Open session Fix the parser, working" }));
    expect(onOpenSession).toHaveBeenCalledWith("p1", "s1");
    expect(screen.getByText("Alpha")).toBeInTheDocument();
  });

  it("says a session of a project outside this device's selection is not available and offers no open action", () => {
    wrap({ onOpenSession: vi.fn(), isProjectAvailable: () => false }, <LaunchedSessions sessions={[{ ...session, state: "done" }]} />);
    expect(screen.getByTestId("launched-session-unavailable")).toHaveTextContent("Not on this device's projects");
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("labels each state", () => {
    const states = ["input", "draft", "failed", "stopped", "done"] as const;
    wrap(
      { onOpenSession: vi.fn() },
      <LaunchedSessions sessions={states.map((state) => ({ ...session, id: state, title: `T ${state}`, state }))} />,
    );
    expect(screen.getByRole("button", { name: "Open session T input, needs input" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open session T failed, failed" })).toBeInTheDocument();
  });
});

describe("DocumentCard (F-72)", () => {
  it("opens the document, and calls a visual one a visual", async () => {
    const onOpenDocument = vi.fn();
    const user = userEvent.setup();
    wrap({ onOpenDocument }, (
      <>
        <DocumentCard document={{ type: "document", id: "d1", documentId: "doc1", title: "Plan", kind: "md" }} />
        <DocumentCard document={{ type: "document", id: "d2", documentId: "doc2", title: "Chart", kind: "visual" }} />
      </>
    ));
    await user.click(screen.getByRole("button", { name: "Open document Plan" }));
    expect(onOpenDocument).toHaveBeenCalledWith(expect.objectContaining({ documentId: "doc1" }));
    expect(screen.getByRole("button", { name: "Open visual Chart" })).toBeInTheDocument();
  });
});
