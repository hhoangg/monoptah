// F-72: the document dialog reads `body` (Markdown) and `tree` (a visual) of `mono.document`, as the Mono page does.
import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const query = vi.hoisted(() => ({ current: { data: undefined, isError: false } as { data: unknown; isError: boolean } }));
vi.mock("@/data", () => ({ useLinkQuery: () => query.current }));

import { DocumentDialog } from "./DocumentDialog";
import type { DocumentItem } from "./turnItems";

const item = (kind: string): DocumentItem => ({ type: "document", id: "t1:3", documentId: "doc-1", title: "Weekly notes", kind });

beforeEach(() => {
  query.current = { data: undefined, isError: false };
});

describe("DocumentDialog", () => {
  it("renders the Markdown `body`", () => {
    query.current = { data: { title: "Weekly notes", kind: "note", body: "# Heading\n\nSome **bold** text" }, isError: false };
    render(<DocumentDialog document={item("note")} onClose={() => {}} />);
    expect(screen.getByRole("heading", { name: "Heading" })).toBeInTheDocument();
    expect(screen.getByText("bold")).toBeInTheDocument();
  });

  it("draws the `tree` of a visual", () => {
    query.current = { data: { title: "CPU", kind: "visual", body: "", tree: { kind: "gauge", label: "CPU", value: 40, min: 0, max: 100, color: "accent" } }, isError: false };
    render(<DocumentDialog document={item("visual")} onClose={() => {}} />);
    expect(screen.getByTestId("visual-tree")).toBeInTheDocument();
  });

  it("says so when a visual has no tree it can draw", () => {
    query.current = { data: { title: "Page", kind: "html", body: "<b>x</b>", treeError: "Too large to draw." }, isError: false };
    render(<DocumentDialog document={item("html")} onClose={() => {}} />);
    expect(screen.getByText("Too large to draw.")).toBeInTheDocument();
    expect(screen.queryByText("x")).toBeNull();
  });

  it("falls back to the legacy `text` field", () => {
    query.current = { data: { title: "Old", kind: "note", text: "plain words" }, isError: false };
    render(<DocumentDialog document={item("note")} onClose={() => {}} />);
    expect(screen.getByText("plain words")).toBeInTheDocument();
  });

  it("points to the desktop when there is no text at all", () => {
    query.current = { data: { title: "Empty", kind: "note", body: "" }, isError: false };
    render(<DocumentDialog document={item("note")} onClose={() => {}} />);
    expect(screen.getByText("Open this document on your desktop to read it.")).toBeInTheDocument();
  });

  it("shows an error when the document cannot be loaded", () => {
    query.current = { data: undefined, isError: true };
    render(<DocumentDialog document={item("note")} onClose={() => {}} />);
    expect(screen.getByText("Could not load this document.")).toBeInTheDocument();
  });
});
