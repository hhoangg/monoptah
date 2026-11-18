// M-4 (client part): the card answers only while the link is ready. The link and data hooks are mocked.
import type { Approval } from "@monoptah/protocol";
import { act, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const link = { permission: "control" as string | null, status: "ready" as string };
vi.mock("@/link", () => ({ useLinkState: (select: (state: typeof link) => unknown) => select(link) }));
vi.mock("@/version", () => ({ useCapability: () => true }));
vi.mock("@/data", () => ({
  useApprovals: () => [{ requestId: 7, label: "Run example command", detail: "example-cli --check" } satisfies Approval],
  useLinkMutation: () => ({ mutateAsync: vi.fn() }),
}));

import { APPROVAL_LINK_NOT_READY_REASON, ARM_DELAY_MS, ApprovalCard } from "./ApprovalCard";

beforeEach(() => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  vi.stubEnv("VITE_WEB_APPROVALS", "on");
  link.permission = "control";
  link.status = "ready";
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllEnvs();
});

describe("ApprovalCard link readiness", () => {
  it("answers when the link is ready", () => {
    render(<ApprovalCard sessionId="s1" />);
    act(() => void vi.advanceTimersByTime(ARM_DELAY_MS));
    expect(screen.getByRole("button", { name: "Allow once" })).toBeEnabled();
  });

  it.each(["reconnecting", "connecting", "closed"])("disables both buttons with an explanation while the link is %s", (status) => {
    link.status = status;
    render(<ApprovalCard sessionId="s1" />);
    act(() => void vi.advanceTimersByTime(ARM_DELAY_MS));
    expect(screen.getByRole("button", { name: "Allow once" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Deny" })).toBeDisabled();
    expect(screen.getByText(APPROVAL_LINK_NOT_READY_REASON)).toBeInTheDocument();
  });
});
