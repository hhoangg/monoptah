import "fake-indexeddb/auto";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { IDBFactory } from "fake-indexeddb";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { BackgroundControls } from "./BackgroundControls";
import { BackgroundsSection } from "./BackgroundsSection";
import { BACKGROUND_CONTAINER_CLASS, EmptySessionBackground } from "./EmptySessionBackground";
import { getBackground, listBackgrounds, putBackground, removeBackground, setBackgroundOpacity } from "./store";

const renderBackground = vi.hoisted(() => vi.fn());
vi.mock("./render", () => ({ renderBackground }));

const png = () => new Blob(["x"], { type: "image/png" });

beforeEach(() => {
  // Radix Slider measures its thumb with ResizeObserver, which jsdom lacks.
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
  globalThis.indexedDB = new IDBFactory();
  renderBackground.mockReset();
  renderBackground.mockResolvedValue(png());
  URL.createObjectURL = vi.fn(() => "blob:test");
  URL.revokeObjectURL = vi.fn();
});

describe("store", () => {
  it("keeps one record per project, clamps opacity and removes it", async () => {
    await putBackground({ projectId: "b", projectName: "Beta", image: png(), opacity: 0.4, updatedAt: 1 });
    await putBackground({ projectId: "a", projectName: "Alpha", image: png(), opacity: 5, updatedAt: 1 });
    expect((await listBackgrounds()).map((r) => r.projectName)).toEqual(["Alpha", "Beta"]);
    expect((await getBackground("a"))?.opacity).toBe(0.65);
    await setBackgroundOpacity("b", 0.1);
    expect((await getBackground("b"))?.opacity).toBe(0.1);
    await removeBackground("a");
    expect(await getBackground("a")).toBeNull();
  });

  it("uses a database under the monoptah. prefix", async () => {
    await putBackground({ projectId: "a", projectName: "A", image: png(), opacity: 0.4, updatedAt: 1 });
    const names = (await indexedDB.databases()).map((db) => db.name);
    expect(names).toContain("monoptah.backgrounds");
  });
});

describe("EmptySessionBackground", () => {
  it("renders nothing without a stored photo, then the photo at its opacity", async () => {
    render(<EmptySessionBackground projectId="p1" />);
    expect(screen.queryByTestId("empty-session-background")).toBeNull();
    await act(() => putBackground({ projectId: "p1", projectName: "P", image: png(), opacity: 0.3, updatedAt: 1 }));
    const img = await screen.findByTestId("empty-session-background");
    expect(img).toHaveAttribute("src", "blob:test");
    expect(img).toHaveAttribute("aria-hidden", "true");
    expect(img).toHaveStyle({ opacity: "0.3" });
  });

  it("revokes its object URL when it unmounts", async () => {
    await putBackground({ projectId: "p1", projectName: "P", image: png(), opacity: 0.3, updatedAt: 1 });
    const view = render(<EmptySessionBackground projectId="p1" />);
    await screen.findByTestId("empty-session-background");
    expect(URL.revokeObjectURL).not.toHaveBeenCalled();
    view.unmount();
    expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:test");
  });

  it("names the container class that keeps its negative z-index inside the container", () => {
    expect(BACKGROUND_CONTAINER_CLASS.split(" ")).toEqual(expect.arrayContaining(["relative", "isolate"]));
  });
});

describe("BackgroundControls", () => {
  it("picks a photo through a file input, then removes it", async () => {
    const user = userEvent.setup();
    render(<BackgroundControls projectId="p1" projectName="Proj" />);
    const input = screen.getByLabelText("Background photo for Proj") as HTMLInputElement;
    expect(input).toHaveAttribute("accept", "image/*");
    await user.upload(input, new File(["raw"], "photo.jpg", { type: "image/jpeg" }));
    await waitFor(async () => expect(await getBackground("p1")).not.toBeNull());
    expect(renderBackground).toHaveBeenCalledOnce();
    expect((await getBackground("p1"))?.projectName).toBe("Proj");
    expect(await screen.findByText("Opacity")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Remove background for Proj" }));
    await waitFor(async () => expect(await getBackground("p1")).toBeNull());
  });

  it("rejects a non-image file", async () => {
    const user = userEvent.setup({ applyAccept: false });
    render(<BackgroundControls projectId="p1" projectName="Proj" />);
    await user.upload(screen.getByLabelText("Background photo for Proj"), new File(["x"], "a.txt", { type: "text/plain" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Choose an image file.");
    expect(renderBackground).not.toHaveBeenCalled();
  });

  it("shows an error when the dither fails", async () => {
    renderBackground.mockRejectedValue(new Error("boom"));
    const user = userEvent.setup();
    render(<BackgroundControls projectId="p1" projectName="Proj" />);
    await user.upload(screen.getByLabelText("Background photo for Proj"), new File(["x"], "a.png", { type: "image/png" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not use that image.");
  });
});

describe("BackgroundControls failures", () => {
  it("says so when the opacity cannot be saved or the background cannot be removed", async () => {
    const user = userEvent.setup();
    await putBackground({ projectId: "p1", projectName: "Proj", image: png(), opacity: 0.4, updatedAt: 1 });
    render(<BackgroundControls projectId="p1" projectName="Proj" />);
    const remove = await screen.findByRole("button", { name: "Remove background for Proj" });
    const open = vi.spyOn(globalThis.indexedDB, "open").mockImplementation(() => {
      throw new Error("storage blocked");
    });
    await user.click(remove);
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not remove the background.");
    open.mockRestore();
  });

  it("gives the opacity slider a 44px touch target", async () => {
    await putBackground({ projectId: "p1", projectName: "Proj", image: png(), opacity: 0.4, updatedAt: 1 });
    render(<BackgroundControls projectId="p1" projectName="Proj" />);
    const thumb = await screen.findByRole("slider", { name: "Opacity for Proj" });
    expect(thumb.closest(".touch-none")?.className).toContain("h-11");
  });
});

describe("BackgroundsSection", () => {
  it("is empty by default and lists stored backgrounds", async () => {
    render(<BackgroundsSection />);
    expect(await screen.findByText("No backgrounds yet.")).toBeInTheDocument();
    await act(() => putBackground({ projectId: "p1", projectName: "Alpha", image: png(), opacity: 0.4, updatedAt: 1 }));
    expect(await screen.findByLabelText("Background photo for Alpha")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Backgrounds" })).toBeInTheDocument();
    // The footnote names the real entry point: the Background action of a project.
    expect(screen.getByText(/use Background in the project bar/)).toBeInTheDocument();
  });
});
