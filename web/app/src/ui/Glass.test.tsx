import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { Glass } from "./Glass";

describe("Glass", () => {
  it("forwards a click on a pressable surface to the control inside", () => {
    const onPress = vi.fn();
    render(
      <Glass pressable data-testid="glass">
        <button type="button" onClick={onPress}>
          press
        </button>
      </Glass>,
    );
    fireEvent.click(screen.getByTestId("glass"));
    expect(onPress).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "press" }));
    expect(onPress).toHaveBeenCalledTimes(2);
  });

  it("does not forward clicks when it is not pressable", () => {
    const onPress = vi.fn();
    render(
      <Glass data-testid="glass">
        <button type="button" onClick={onPress}>
          press
        </button>
      </Glass>,
    );
    fireEvent.click(screen.getByTestId("glass"));
    expect(onPress).not.toHaveBeenCalled();
  });
});
