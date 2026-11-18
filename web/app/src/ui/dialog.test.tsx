import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it } from "vitest";
import { ConfirmDialog } from "./ConfirmDialog";

function Harness() {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button type="button" onClick={() => setOpen(true)}>
        Open it
      </button>
      <ConfirmDialog open={open} onOpenChange={setOpen} title="Sure?" description="Really" confirmLabel="Yes" onConfirm={() => setOpen(false)} />
    </>
  );
}

describe("DialogContent focus", () => {
  it("returns focus to the control that opened a state-controlled dialog", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    const opener = screen.getByRole("button", { name: "Open it" });
    opener.focus();
    await user.keyboard("{Enter}");
    expect(await screen.findByRole("dialog", { name: "Sure?" })).toBeTruthy();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(opener));
  });
});
