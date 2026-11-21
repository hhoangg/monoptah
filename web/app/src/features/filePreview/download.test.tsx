// What a download may be called and how it is made: a basename, never a path, and always an octet-stream saved, not opened.
import { act, cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SaveButton, safeDownloadName, saveBytes, type SaveContext } from "./download";

afterEach(() => vi.restoreAllMocks());

describe("safeDownloadName", () => {
  it("keeps the basename only, whichever separator", () => {
    expect(safeDownloadName("/a/b/chart.png")).toBe("chart.png");
    expect(safeDownloadName("..\\..\\evil.exe")).toBe("evil.exe");
    expect(safeDownloadName("img/../../etc/passwd")).toBe("passwd");
  });

  it("replaces control, bidi and reserved characters, and strips leading and trailing dots", () => {
    expect(safeDownloadName("a\u0000b\u001fc.txt")).toBe("a_b_c.txt");
    expect(safeDownloadName("fa‮txt.exe")).toBe("fa_txt.exe");
    expect(safeDownloadName('we"ird:na*me?.png')).toBe("we_ird_na_me_.png");
    expect(safeDownloadName("...hidden.")).toBe("hidden");
  });

  it("falls back when nothing is left, and cuts a very long name but keeps its extension", () => {
    expect(safeDownloadName("")).toBe("file");
    expect(safeDownloadName("/")).toBe("file");
    expect(safeDownloadName("..", "image")).toBe("image");
    const long = safeDownloadName(`${"x".repeat(400)}.png`);
    expect(long.length).toBe(150);
    expect(long.endsWith(".png")).toBe(true);
  });
});

describe("saveBytes", () => {
  it("clicks a download link of an octet-stream object URL and never navigates", () => {
    const made: Blob[] = [];
    URL.createObjectURL = vi.fn((blob: Blob) => (made.push(blob), "blob:https://app.test/1"));
    URL.revokeObjectURL = vi.fn();
    const clicked: Array<{ download: string; href: string; rel: string }> = [];
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) {
      clicked.push({ download: this.download, href: this.href, rel: this.rel });
    });
    saveBytes("/x/y/page.html", [new Uint8Array([1, 2, 3])]);
    expect(made[0]!.type).toBe("application/octet-stream");
    expect(made[0]!.size).toBe(3);
    expect(clicked).toEqual([{ download: "page.html", href: "blob:https://app.test/1", rel: "noopener" }]);
  });
});

describe("SaveButton", () => {
  it("gets lazy bytes when pressed, and says when that failed", async () => {
    URL.createObjectURL = vi.fn(() => "blob:https://app.test/2");
    URL.revokeObjectURL = vi.fn();
    const clicked: string[] = [];
    vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) {
      clicked.push(this.download);
    });
    let fail = true;
    render(<SaveButton name="a.txt" parts={async () => (fail ? Promise.reject(new Error("x")) : [new Uint8Array([1])])} />);
    const user = userEvent.setup();
    await user.click(screen.getByTestId("file-preview-download"));
    expect(await screen.findByText("Download failed, try again")).toBeInTheDocument();
    fail = false;
    await user.click(screen.getByTestId("file-preview-download"));
    await vi.waitFor(() => expect(clicked).toEqual(["a.txt"]));
  });

  it("shows that it is preparing, and waiting when the desktop says so, and stops the work when it goes away", async () => {
    let context: SaveContext | undefined;
    render(<SaveButton name="a.txt" parts={(given) => new Promise(() => (context = given))} />);
    const user = userEvent.setup();
    await user.click(screen.getByTestId("file-preview-download"));
    expect(screen.getByTestId("file-preview-download")).toHaveTextContent("Preparing...");
    expect(screen.getByTestId("file-preview-download")).toBeDisabled();
    act(() => context!.setWaiting(true));
    expect(screen.getByTestId("file-preview-download")).toHaveTextContent("Waiting for your desktop...");
    cleanup();
    expect(context!.signal.aborted).toBe(true);
  });
});
