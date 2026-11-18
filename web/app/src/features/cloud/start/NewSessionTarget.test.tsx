// BILL M-4: the New session target resets when the capability goes off or the desktop or project changes.
// The data and link hooks are mocked, so only the slot's own state handling is under test.
import { render } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const caps = { cloud: true, cloudStart: true };
vi.mock("@/version", () => ({ useCapability: (name: "cloud" | "cloudStart") => caps[name] }));
vi.mock("@/link", () => ({ useLinkState: (select: (state: { permission: string }) => unknown) => select({ permission: "control" }) }));
vi.mock("@/data", () => ({ useLinkQuery: () => ({ data: undefined, isError: false, isPending: true }) }));

import { NewSessionTarget } from "./NewSessionTarget";

const sandbox = { kind: "sandbox", instance: "new", branch: "main" };

beforeEach(() => {
  caps.cloud = true;
  caps.cloudStart = true;
});

describe("NewSessionTarget reset", () => {
  it("keeps a picked target while nothing changes", () => {
    const onChange = vi.fn();
    const { rerender } = render(<NewSessionTarget desktopId="d1" projectId="p1" value={sandbox} onChange={onChange} />);
    rerender(<NewSessionTarget desktopId="d1" projectId="p1" value={sandbox} onChange={onChange} />);
    expect(onChange).not.toHaveBeenCalled();
  });

  it("resets to local when the project changes", () => {
    const onChange = vi.fn();
    const { rerender } = render(<NewSessionTarget desktopId="d1" projectId="p1" value={sandbox} onChange={onChange} />);
    rerender(<NewSessionTarget desktopId="d1" projectId="p2" value={sandbox} onChange={onChange} />);
    expect(onChange).toHaveBeenCalledWith(null);
  });

  it("resets to local when the desktop changes", () => {
    const onChange = vi.fn();
    const { rerender } = render(<NewSessionTarget desktopId="d1" projectId="p1" value={sandbox} onChange={onChange} />);
    rerender(<NewSessionTarget desktopId="d2" projectId="p1" value={sandbox} onChange={onChange} />);
    expect(onChange).toHaveBeenCalledWith(null);
  });

  it.each(["cloud", "cloudStart"] as const)("resets to local when the %s capability turns off", (name) => {
    const onChange = vi.fn();
    const { rerender } = render(<NewSessionTarget desktopId="d1" projectId="p1" value={sandbox} onChange={onChange} />);
    caps[name] = false;
    rerender(<NewSessionTarget desktopId="d1" projectId="p1" value={sandbox} onChange={onChange} />);
    expect(onChange).toHaveBeenCalledWith(null);
  });

  it("does not call onChange for a local target", () => {
    const onChange = vi.fn();
    const { rerender } = render(<NewSessionTarget desktopId="d1" projectId="p1" value={null} onChange={onChange} />);
    caps.cloudStart = false;
    rerender(<NewSessionTarget desktopId="d1" projectId="p2" value={null} onChange={onChange} />);
    expect(onChange).not.toHaveBeenCalled();
  });
});
