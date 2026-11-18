// F-47 with `prefers-reduced-motion: reduce`: the morph is instant (no spring), the state is the same.
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CARD_RADIUS_PX, Composer, PILL_RADIUS_PX } from "./Composer";

beforeEach(() => {
  window.matchMedia = ((query: string) => ({
    matches: query.includes("prefers-reduced-motion"),
    media: query,
    onchange: null,
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
    dispatchEvent: () => false,
  })) as never;
  window.scrollTo = vi.fn() as never;
});

describe("composer morph with reduced motion (F-47)", () => {
  it("snaps between pill and card without waiting for a spring", async () => {
    const user = userEvent.setup();
    render(<Composer onSubmit={() => Promise.resolve()} />);
    const glass = screen.getByTestId("composer").querySelector<HTMLElement>("[data-shape]")!;
    expect(glass.style.borderRadius).toBe(`${PILL_RADIUS_PX}px`);
    await user.click(screen.getByRole("textbox", { name: "Message" }));
    expect(glass).toHaveAttribute("data-shape", "card");
    // Zero duration: the radius lands within a frame, not after a spring settles.
    await vi.waitFor(() => expect(glass.style.borderRadius).toBe(`${CARD_RADIUS_PX}px`), { timeout: 200 });
  });
});
