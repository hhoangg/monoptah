import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CodeBlock } from "./CodeBlock";
import { Markdown } from "./Markdown";

function mockMatchMedia(reduce: boolean) {
  vi.stubGlobal(
    "matchMedia",
    (query: string) => ({
      matches: reduce && query.includes("prefers-reduced-motion"),
      media: query,
      addEventListener: () => {},
      removeEventListener: () => {},
    }),
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("<Markdown>", () => {
  it("renders the block types as elements", () => {
    const { container } = render(
      <Markdown
        text={["# Title", "", "Some **bold** and *it* and ~~gone~~.", "", "- a", "- b", "", "> q", "", "| x | y |", "| - | - |", "| 1 | 2 |"].join("\n")}
      />,
    );
    expect(container.querySelector("h1")?.textContent).toBe("Title");
    expect(container.querySelector("strong")?.textContent).toBe("bold");
    expect(container.querySelector("em")?.textContent).toBe("it");
    expect(container.querySelector("del")?.textContent).toBe("gone");
    expect(container.querySelectorAll("li")).toHaveLength(2);
    expect(container.querySelector("blockquote")?.textContent).toBe("q");
    expect(container.querySelectorAll("th")).toHaveLength(2);
    expect(container.querySelectorAll("td")).toHaveLength(2);
  });

  it("handles unterminated markdown while streaming", () => {
    const { container } = render(<Markdown text={"Fixed the **bo"} streaming />);
    expect(container.textContent).not.toContain("*");
    const second = render(<Markdown text={"```ts\nconst a ="} streaming />);
    expect(second.container.querySelector(".md-code")?.getAttribute("data-incomplete")).toBe("true");
  });

  it("opens files through onOpenFile with the line", async () => {
    const onOpenFile = vi.fn();
    render(<Markdown text={"See `src/a.ts:12` and [b](lib/b.rs#L3) and `npm test`."} onOpenFile={onOpenFile} />);
    await userEvent.click(screen.getByRole("button", { name: /src\/a\.ts:12/ }));
    expect(onOpenFile).toHaveBeenLastCalledWith("src/a.ts", 12);
    await userEvent.click(screen.getByRole("button", { name: "b" }));
    expect(onOpenFile).toHaveBeenLastCalledWith("lib/b.rs", 3);
    expect(screen.getAllByRole("button")).toHaveLength(2);
  });

  it("makes a file-naming code span a chip with the icon of its file type, and a file link too", async () => {
    const onOpenFile = vi.fn();
    render(<Markdown text={"Wrote `report.html`, `~/Documents/x/notes.md:12`, `/abs/dir/a.png` and [the log](out/run.log#L7)."} onOpenFile={onOpenFile} />);
    const chips = screen.getAllByTestId("file-mention");
    expect(chips.map((chip) => chip.getAttribute("data-path"))).toEqual(["report.html", "~/Documents/x/notes.md", "/abs/dir/a.png", "out/run.log"]);
    expect(chips[0]?.querySelector('[data-kind="markup"]')).not.toBeNull();
    expect(chips[1]?.querySelector('[data-kind="markdown"]')).not.toBeNull();
    expect(chips[2]?.querySelector('[data-kind="image"]')).not.toBeNull();
    await userEvent.click(chips[1] as HTMLElement);
    expect(onOpenFile).toHaveBeenLastCalledWith("~/Documents/x/notes.md", 12);
    await userEvent.click(chips[3] as HTMLElement);
    expect(onOpenFile).toHaveBeenLastCalledWith("out/run.log", 7);
  });

  it("keeps an image as its alt text in chat, and draws only a relative one when a renderImage is given", () => {
    const text = "![chart](img/a.png) and ![remote](https://x.invalid/t.png) and ![abs](/etc/a.png)";
    const chat = render(<Markdown text={text} />);
    expect(chat.container.querySelector("img")).toBeNull();
    expect(chat.container.textContent).toContain("chart and remote and abs");
    chat.unmount();
    const renderImage = vi.fn((src: string, alt: string) => <img data-testid="drawn" src={`blob:x/${src}`} alt={alt} />);
    const preview = render(<Markdown text={text} renderImage={renderImage} />);
    expect(renderImage).toHaveBeenCalledTimes(1);
    expect(renderImage).toHaveBeenCalledWith("img/a.png", "chart");
    expect(preview.container.querySelectorAll("img")).toHaveLength(1);
    expect(preview.container.textContent).toContain("remote and abs");
  });

  it("shows files unlinked without onOpenFile", () => {
    render(<Markdown text="See `src/a.ts`." />);
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("routes in-app links through onNavigate", async () => {
    const onNavigate = vi.fn();
    render(<Markdown text="[session](/d/1/s/2)" onNavigate={onNavigate} />);
    await userEvent.click(screen.getByRole("link", { name: "session" }));
    expect(onNavigate).toHaveBeenCalledWith("/d/1/s/2");
  });
});

describe("word reveal", () => {
  it("fades new words while streaming and settles them afterwards", () => {
    mockMatchMedia(false);
    vi.useFakeTimers();
    const { container, rerender } = render(<Markdown text="Hello there " streaming />);
    rerender(<Markdown text="Hello there my friend " streaming />);
    expect(container.querySelectorAll(".md-word").length).toBeGreaterThan(0);
    act(() => {
      vi.advanceTimersByTime(2000);
    });
    expect(container.querySelectorAll(".md-word")).toHaveLength(0);
    expect(container.textContent).toContain("Hello there my friend");
  });

  it("does not animate or hold words back under prefers-reduced-motion", () => {
    mockMatchMedia(true);
    const { container, rerender } = render(<Markdown text="Hello there " streaming />);
    rerender(<Markdown text="Hello there my fri" streaming />);
    expect(container.querySelectorAll(".md-word")).toHaveLength(0);
    // A half-written word is shown at once rather than held back.
    expect(container.textContent).toContain("fri");
  });

  it("does not fade text there on mount unless asked", () => {
    mockMatchMedia(false);
    const { container } = render(<Markdown text="Whole reply here" />);
    expect(container.querySelectorAll(".md-word")).toHaveLength(0);
    const animated = render(<Markdown text="Whole reply here" animate />);
    expect(animated.container.querySelectorAll(".md-word").length).toBeGreaterThan(0);
  });
});

describe("<CodeBlock>", () => {
  it("renders tokens as spans, never as HTML, and shows the language", async () => {
    const { container } = render(<CodeBlock code={"const a = 1;\n\nlet b;"} language="ts" lineNumbers />);
    await waitFor(() => expect(container.querySelectorAll(".md-token").length).toBeGreaterThan(2));
    expect(container.querySelector(".md-code-lang")?.textContent).toBe("ts");
    expect(container.querySelectorAll(".md-code-line")).toHaveLength(3);
    expect(container.querySelector("code")?.className).toContain("md-code-numbered");
    expect([...container.querySelectorAll(".md-code-line")].map((line) => line.textContent)).toEqual(["const a = 1;", "\n", "let b;"]);
  });

  it("falls back to plain lines for an unknown language", () => {
    const { container } = render(<CodeBlock code={"a\nb"} language="nope" />);
    expect(container.querySelectorAll(".md-token")).toHaveLength(0);
    expect(container.querySelectorAll(".md-code-line")).toHaveLength(2);
  });

  it("copies the code", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    render(<CodeBlock code={"echo hi\n"} language="sh" />);
    fireEvent.click(screen.getByRole("button", { name: "Copy code" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("echo hi"));
    await screen.findByRole("button", { name: "Copied" });
  });

  it("copies without control and bidi characters", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    render(<CodeBlock code={"ls\u001b[201~\n\u202erm -rf x"} language="sh" />);
    fireEvent.click(screen.getByRole("button", { name: "Copy code" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("ls[201~\nrm -rf x"));
  });

  it("keeps a highlighted prefix while a fence grows", async () => {
    const { container, rerender } = render(<CodeBlock code={"let a = 1;\nlet b"} language="js" isIncomplete />);
    await waitFor(() => expect(container.querySelectorAll(".md-token").length).toBeGreaterThan(2));
    rerender(<CodeBlock code={"let a = 1;\nlet b = 2;\nlet c"} language="js" isIncomplete />);
    expect(container.querySelectorAll(".md-code-line")).toHaveLength(3);
    expect([...container.querySelectorAll(".md-code-line")].map((line) => line.textContent)).toEqual(["let a = 1;", "let b = 2;", "let c"]);
  });
});

describe("external link confirm sheet (security.md 8, rule 2)", () => {
  it("shows the real host, the full address and the link text before opening, and opens only on confirm", async () => {
    const open = vi.spyOn(window, "open").mockReturnValue(null);
    const user = userEvent.setup();
    render(<Markdown text="[https://github.com/hhoangg/monoptah](https://evil.example/login?x=1)" />);
    await user.click(screen.getByRole("link"));
    expect(open).not.toHaveBeenCalled();
    expect(screen.getByTestId("external-link-host")).toHaveTextContent("evil.example");
    expect(screen.getByTestId("external-link-url")).toHaveTextContent("https://evil.example/login?x=1");
    expect(screen.getByTestId("external-link-label")).toHaveTextContent("https://github.com/hhoangg/monoptah");
    await user.click(screen.getByRole("button", { name: "Open link" }));
    expect(open).toHaveBeenCalledWith("https://evil.example/login?x=1", "_blank", "noopener,noreferrer");
    open.mockRestore();
  });

  it("does not open when cancelled, and warns about http", async () => {
    const open = vi.spyOn(window, "open").mockReturnValue(null);
    const user = userEvent.setup();
    render(<Markdown text="[docs](http://a.dev/x)" />);
    await user.click(screen.getByRole("link"));
    expect(screen.getByText("This address is not encrypted (http).")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(open).not.toHaveBeenCalled();
    open.mockRestore();
  });
});
