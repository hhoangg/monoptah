import type { Approval } from "@monoptah/protocol";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { APPROVAL_READ_ONLY_REASON, ApprovalCardView, MAX_DETAIL_CHARS } from "./ApprovalCard";
import { isApprovalCardEnabled } from "./flag";

const one: Approval = { requestId: 41, label: "Run example command", detail: "example-cli --check" };
const two: Approval = { requestId: 42, label: "Edit example file", detail: "example.txt" };

describe("ApprovalCardView", () => {
  it("renders nothing without a request", () => {
    const { container } = render(<ApprovalCardView approvals={[]} onAnswer={vi.fn()} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("shows one request at a time with a count", () => {
    render(<ApprovalCardView approvals={[one, two]} onAnswer={vi.fn()} />);
    expect(screen.getByText("Run example command")).toBeInTheDocument();
    expect(screen.queryByText("Edit example file")).toBeNull();
    expect(screen.getByText("1 of 2")).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Needs your approval" })).toBeInTheDocument();
  });

  it("offers exactly Allow once and Deny", () => {
    render(<ApprovalCardView approvals={[one]} onAnswer={vi.fn()} />);
    expect(screen.getAllByRole("button").map((button) => button.textContent)).toEqual(["Deny", "Allow once"]);
    expect(screen.queryByText(/always|for session|full.?access|mode/i)).toBeNull();
  });

  it("sends allow and deny with the request id", async () => {
    const user = userEvent.setup();
    const onAnswer = vi.fn().mockResolvedValue(undefined);
    const { unmount } = render(<ApprovalCardView approvals={[one]} onAnswer={onAnswer} />);
    await user.click(screen.getByRole("button", { name: "Allow once" }));
    expect(onAnswer).toHaveBeenCalledWith(41, "allow");
    unmount();
    const denied = vi.fn().mockResolvedValue(undefined);
    render(<ApprovalCardView approvals={[one]} onAnswer={denied} />);
    await user.click(screen.getByRole("button", { name: "Deny" }));
    expect(denied).toHaveBeenCalledWith(41, "deny");
  });

  it("sends once on a double click and stays disabled until the list changes", async () => {
    const user = userEvent.setup();
    const onAnswer = vi.fn().mockResolvedValue(undefined);
    const { rerender } = render(<ApprovalCardView approvals={[one, two]} onAnswer={onAnswer} />);
    const allow = screen.getByRole("button", { name: "Allow once" });
    await user.dblClick(allow);
    expect(onAnswer).toHaveBeenCalledTimes(1);
    expect(allow).toBeDisabled();
    expect(screen.getByRole("button", { name: "Deny" })).toBeDisabled();
    expect(screen.getByText("Run example command")).toBeInTheDocument();
    // The next frame brings the next request: it is answerable again.
    rerender(<ApprovalCardView approvals={[two]} onAnswer={onAnswer} />);
    expect(screen.getByRole("button", { name: "Allow once" })).toBeEnabled();
    await user.click(screen.getByRole("button", { name: "Deny" }));
    expect(onAnswer).toHaveBeenLastCalledWith(42, "deny");
    expect(onAnswer).toHaveBeenCalledTimes(2);
  });

  it("shows a retry-able error when the answer fails", async () => {
    const user = userEvent.setup();
    const onAnswer = vi.fn().mockRejectedValueOnce(new Error("Link lost")).mockResolvedValue(undefined);
    render(<ApprovalCardView approvals={[one]} onAnswer={onAnswer} />);
    await user.click(screen.getByRole("button", { name: "Allow once" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Link lost");
    await user.click(screen.getByRole("button", { name: "Allow once" }));
    expect(onAnswer).toHaveBeenCalledTimes(2);
  });

  it("disables both buttons with an explanation on a read-only device", async () => {
    const user = userEvent.setup();
    const onAnswer = vi.fn();
    render(<ApprovalCardView approvals={[one]} onAnswer={onAnswer} disabledReason={APPROVAL_READ_ONLY_REASON} />);
    expect(screen.getByText("Approvals need a device with control access.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Allow once" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Deny" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Allow once" }));
    expect(onAnswer).not.toHaveBeenCalled();
  });

  it("keeps untrusted label and detail inert", () => {
    const payloads = [
      '<img src=x onerror="window.__pwned=1">',
      "<script>window.__pwned=1</script>",
      "[click me](javascript:window.__pwned=1)",
      '<a href="javascript:window.__pwned=1">go</a>',
      "**bold** `code` # heading\n- item",
    ];
    const { container } = render(
      <ApprovalCardView approvals={[{ requestId: 1, label: payloads[0]!, detail: payloads.join("\n") }]} onAnswer={vi.fn()} />,
    );
    expect(container.querySelector("img, script, a, strong, code, h1, ul, [onerror]")).toBeNull();
    expect(container.querySelector("[href], [src]")).toBeNull();
    expect(container.querySelector("[data-approval-label]")?.textContent).toBe(payloads[0]);
    expect(container.querySelector("[data-approval-detail]")?.textContent).toBe(payloads.join("\n"));
    expect((window as unknown as { __pwned?: number }).__pwned).toBeUndefined();
  });

  it("caps a huge detail and makes it a scrollable focusable region", () => {
    render(<ApprovalCardView approvals={[{ requestId: 1, label: "x", detail: "a".repeat(MAX_DETAIL_CHARS * 2) }]} onAnswer={vi.fn()} />);
    const detail = screen.getByLabelText("Request details");
    expect(detail.textContent!.length).toBeLessThanOrEqual(MAX_DETAIL_CHARS + 3);
    expect(detail).toHaveAttribute("tabindex", "0");
    expect(detail.className).toContain("overflow-auto");
    expect(detail.className).toContain("max-h-");
  });
});

describe("isApprovalCardEnabled", () => {
  it("is off by default and on only for VITE_WEB_APPROVALS=on", () => {
    expect(isApprovalCardEnabled()).toBe(false);
    vi.stubEnv("VITE_WEB_APPROVALS", "on");
    expect(isApprovalCardEnabled()).toBe(true);
    vi.stubEnv("VITE_WEB_APPROVALS", "true");
    expect(isApprovalCardEnabled()).toBe(false);
    vi.unstubAllEnvs();
  });
});
