import { act, fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import {
  JUMP_TO_BOTTOM_THRESHOLD_PX,
  TranscriptJumpToBottom,
  isAwayFromBottom,
  useTranscriptJumpVisibility,
} from "./TranscriptJumpToBottom";

describe("isAwayFromBottom", () => {
  it("uses a 160 px threshold", () => {
    expect(JUMP_TO_BOTTOM_THRESHOLD_PX).toBe(160);
    const at = (gap: number) => ({ scrollHeight: 1000, clientHeight: 400, scrollTop: 600 - gap });
    expect(isAwayFromBottom(at(160))).toBe(false);
    expect(isAwayFromBottom(at(161))).toBe(true);
    expect(isAwayFromBottom(at(0))).toBe(false);
  });
});

function Harness({ onJump }: { onJump: () => void }) {
  const visibility = useTranscriptJumpVisibility();
  return (
    <>
      <button type="button" onClick={() => visibility.setVisible(true)}>
        show
      </button>
      <TranscriptJumpToBottom visibility={visibility} onJump={onJump} />
    </>
  );
}

describe("TranscriptJumpToBottom", () => {
  it("is hidden until visibility is set, then jumps on click", () => {
    const onJump = vi.fn();
    render(<Harness onJump={onJump} />);
    expect(screen.queryByRole("button", { name: "Jump to latest" })).toBeNull();
    act(() => screen.getByText("show").click());
    fireEvent.click(screen.getByRole("button", { name: "Jump to latest" }));
    expect(onJump).toHaveBeenCalledTimes(1);
  });

  it("jumps when the press lands on the glass hit slop around the button", () => {
    const onJump = vi.fn();
    render(<Harness onJump={onJump} />);
    act(() => screen.getByText("show").click());
    const glass = screen.getByRole("button", { name: "Jump to latest" }).parentElement!;
    // A hit on the ::after hit slop targets the glass element itself.
    fireEvent.click(glass);
    expect(onJump).toHaveBeenCalledTimes(1);
  });
});
