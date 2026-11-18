// F-47: the pill/card morph of the composer. The reduced-motion preference is read once by `motion`,
// so the "reduce" case lives in Composer.morphReduced.test.tsx.
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CARD_RADIUS_PX, Composer, MORPH_TRANSITION, PILL_FIELD_MIN_PX, PILL_RADIUS_PX } from "./Composer";

beforeEach(() => {
  window.matchMedia = ((query: string) => ({
    matches: false,
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

function glass() {
  return screen.getByTestId("composer").querySelector<HTMLElement>("[data-shape]")!;
}

describe("composer morph (F-47)", () => {
  it("rests as a pill with a one-line field", () => {
    render(<Composer onSubmit={() => Promise.resolve()} />);
    expect(glass()).toHaveAttribute("data-shape", "pill");
    expect(glass().style.borderRadius).toBe(`${PILL_RADIUS_PX}px`);
    expect(screen.getByRole("textbox", { name: "Message" }).style.minHeight).toBe(`${PILL_FIELD_MIN_PX}px`);
  });

  it("morphs to a card on focus and back to a pill on blur while the draft is empty", async () => {
    const user = userEvent.setup();
    render(<Composer onSubmit={() => Promise.resolve()} />);
    await user.click(screen.getByRole("textbox", { name: "Message" }));
    expect(glass()).toHaveAttribute("data-shape", "card");
    expect(screen.getByRole("textbox", { name: "Message" }).style.minHeight).toBe("64px");
    await waitFor(() => expect(glass().style.borderRadius).toBe(`${CARD_RADIUS_PX}px`));
    await user.click(document.body);
    expect(glass()).toHaveAttribute("data-shape", "pill");
    await waitFor(() => expect(glass().style.borderRadius).toBe(`${PILL_RADIUS_PX}px`));
  });

  it("stays a card while it holds a draft, even without focus", async () => {
    const user = userEvent.setup();
    render(<Composer onSubmit={() => Promise.resolve()} />);
    await user.type(screen.getByRole("textbox", { name: "Message" }), "hello");
    await user.click(document.body);
    expect(glass()).toHaveAttribute("data-shape", "card");
  });

  it("is a card while a notice takes the field", () => {
    render(<Composer onSubmit={() => Promise.resolve()} notice={{ text: "Boom", dismiss: () => {} }} />);
    expect(glass()).toHaveAttribute("data-shape", "card");
  });

  it("is the design-system pill at rest: 50 px, 25 px corners, one row, extras clipped but still in the DOM", () => {
    render(<Composer onSubmit={() => Promise.resolve()} topRow={<span>Workspace row</span>} toolbar={<button type="button">Model</button>} />);
    expect(PILL_RADIUS_PX).toBe(25);
    expect(glass().className).toContain("min-h-[50px]");
    expect(glass().className).toContain("flex-row");
    expect(glass().className).not.toContain("flex-col");
    expect(glass().style.borderRadius).toBe("25px");
    expect(screen.getByText("Workspace row").parentElement?.className).toContain("overflow-hidden");
    expect(screen.getByRole("button", { name: "Model" }).parentElement?.className).toContain("overflow-hidden");
    // Not aria-hidden or inert: a Tab into them is what opens the card.
    expect(screen.getByRole("button", { name: "Model" })).toBeInTheDocument();
  });

  it("opens the second row when focus moves into the model controls", async () => {
    const user = userEvent.setup();
    render(<Composer onSubmit={() => Promise.resolve()} toolbar={<button type="button">Model</button>} />);
    await user.tab();
    await user.tab();
    await user.tab();
    expect(glass()).toHaveAttribute("data-shape", "card");
    expect(glass().className).toContain("flex-col");
    expect(glass().className).not.toContain("min-h-[50px]");
    expect(screen.getByRole("button", { name: "Model" }).parentElement?.className).not.toContain("overflow-hidden");
  });

  it("morphs over 280 ms with out-cubic, the duration of design-system.md section 6.2", () => {
    expect(MORPH_TRANSITION).toEqual({ duration: 0.28, ease: [0.215, 0.61, 0.355, 1] });
  });
});
