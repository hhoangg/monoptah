import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LONG_PRESS_DELAY_MS, useLongPress, type UseLongPressOptions } from "./useLongPress";

function Target(props: Partial<UseLongPressOptions> & { onLongPress: UseLongPressOptions["onLongPress"]; onClick?: () => void }) {
  const { onClick, ...options } = props;
  const { bind } = useLongPress(options);
  return (
    <button type="button" data-testid="target" onClick={onClick} {...bind}>
      press
    </button>
  );
}

const touch = (extra: Record<string, unknown> = {}) => ({ pointerType: "touch", pointerId: 1, isPrimary: true, button: 0, clientX: 100, clientY: 100, ...extra });

let vibrate: ReturnType<typeof vi.fn>;
beforeEach(() => {
  vi.useFakeTimers();
  vibrate = vi.fn();
  Object.defineProperty(navigator, "vibrate", { value: vibrate, configurable: true, writable: true });
});
afterEach(() => {
  vi.useRealTimers();
  Reflect.deleteProperty(navigator, "vibrate");
  localStorage.clear();
});

describe("useLongPress", () => {
  it("fires after 500 ms with the finger position, and gives a haptic tick", () => {
    const onLongPress = vi.fn();
    render(<Target onLongPress={onLongPress} />);
    const target = screen.getByTestId("target");
    fireEvent.pointerDown(target, touch({ clientX: 40, clientY: 60 }));
    act(() => void vi.advanceTimersByTime(LONG_PRESS_DELAY_MS - 1));
    expect(onLongPress).not.toHaveBeenCalled();
    act(() => void vi.advanceTimersByTime(1));
    expect(onLongPress).toHaveBeenCalledTimes(1);
    expect(onLongPress).toHaveBeenCalledWith({ x: 40, y: 60, target });
    expect(vibrate).toHaveBeenCalledTimes(1);
  });

  it("does not need navigator.vibrate (iOS)", () => {
    Reflect.deleteProperty(navigator, "vibrate");
    const onLongPress = vi.fn();
    render(<Target onLongPress={onLongPress} />);
    fireEvent.pointerDown(screen.getByTestId("target"), touch());
    act(() => void vi.advanceTimersByTime(LONG_PRESS_DELAY_MS));
    expect(onLongPress).toHaveBeenCalledTimes(1);
  });

  it("is cancelled by a move past the tolerance, but not by a small tremor", () => {
    const onLongPress = vi.fn();
    render(<Target onLongPress={onLongPress} />);
    const target = screen.getByTestId("target");
    fireEvent.pointerDown(target, touch());
    fireEvent.pointerMove(target, touch({ clientX: 104, clientY: 103 }));
    act(() => void vi.advanceTimersByTime(LONG_PRESS_DELAY_MS));
    expect(onLongPress).toHaveBeenCalledTimes(1);

    fireEvent.pointerUp(target, touch());
    fireEvent.pointerDown(target, touch());
    fireEvent.pointerMove(target, touch({ clientX: 100, clientY: 130 }));
    act(() => void vi.advanceTimersByTime(LONG_PRESS_DELAY_MS * 2));
    expect(onLongPress).toHaveBeenCalledTimes(1);
  });

  it("is cancelled by a release, a pointercancel (the browser started to scroll) and a scroll", () => {
    const onLongPress = vi.fn();
    render(<Target onLongPress={onLongPress} />);
    const target = screen.getByTestId("target");
    for (const end of [() => fireEvent.pointerUp(target, touch()), () => fireEvent.pointerCancel(target, touch()), () => fireEvent.scroll(window)]) {
      fireEvent.pointerDown(target, touch());
      act(() => void vi.advanceTimersByTime(200));
      end();
      act(() => void vi.advanceTimersByTime(LONG_PRESS_DELAY_MS));
    }
    expect(onLongPress).not.toHaveBeenCalled();
  });

  it("ignores the mouse and a second finger, and does nothing when disabled", () => {
    const onLongPress = vi.fn();
    const view = render(<Target onLongPress={onLongPress} />);
    const target = screen.getByTestId("target");
    fireEvent.pointerDown(target, touch({ pointerType: "mouse" }));
    fireEvent.pointerDown(target, touch({ isPrimary: false }));
    act(() => void vi.advanceTimersByTime(LONG_PRESS_DELAY_MS));
    expect(onLongPress).not.toHaveBeenCalled();
    view.rerender(<Target onLongPress={onLongPress} enabled={false} />);
    fireEvent.pointerDown(target, touch());
    act(() => void vi.advanceTimersByTime(LONG_PRESS_DELAY_MS));
    expect(onLongPress).not.toHaveBeenCalled();
  });

  it("swallows the click of the release after a long press, once", () => {
    const onClick = vi.fn();
    render(<Target onLongPress={() => undefined} onClick={onClick} />);
    const target = screen.getByTestId("target");
    fireEvent.pointerDown(target, touch());
    act(() => void vi.advanceTimersByTime(LONG_PRESS_DELAY_MS));
    fireEvent.pointerUp(target, touch());
    fireEvent.click(target);
    expect(onClick).not.toHaveBeenCalled();
    // The next, ordinary tap goes through.
    fireEvent.pointerDown(target, touch());
    fireEvent.pointerUp(target, touch());
    fireEvent.click(target);
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it("when the callback declines (returns false): no haptic tick and the click is not swallowed", () => {
    const onClick = vi.fn();
    render(<Target onLongPress={() => false} onClick={onClick} />);
    const target = screen.getByTestId("target");
    fireEvent.pointerDown(target, touch());
    act(() => void vi.advanceTimersByTime(LONG_PRESS_DELAY_MS));
    fireEvent.pointerUp(target, touch());
    fireEvent.click(target);
    expect(vibrate).not.toHaveBeenCalled();
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it("forgets a long press whose release produced no click, so a later click is not swallowed", () => {
    const onClick = vi.fn();
    render(<Target onLongPress={() => undefined} onClick={onClick} />);
    const target = screen.getByTestId("target");
    fireEvent.pointerDown(target, touch());
    act(() => void vi.advanceTimersByTime(LONG_PRESS_DELAY_MS));
    fireEvent.pointerUp(target, touch());
    act(() => void vi.advanceTimersByTime(200));
    fireEvent.click(target);
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it("does not swallow the click of a short tap", () => {
    const onClick = vi.fn();
    render(<Target onLongPress={() => undefined} onClick={onClick} />);
    const target = screen.getByTestId("target");
    fireEvent.pointerDown(target, touch());
    act(() => void vi.advanceTimersByTime(120));
    fireEvent.pointerUp(target, touch());
    fireEvent.click(target);
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it("gives no haptic when the user turned haptics off", () => {
    localStorage.setItem("monoptah.haptics", "off");
    render(<Target onLongPress={() => undefined} />);
    fireEvent.pointerDown(screen.getByTestId("target"), touch());
    act(() => void vi.advanceTimersByTime(LONG_PRESS_DELAY_MS));
    expect(vibrate).not.toHaveBeenCalled();
  });
});
