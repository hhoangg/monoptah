import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { AttachmentChip } from "./AttachmentChip";

describe("AttachmentChip", () => {
  it("shows a file name with a remove button", async () => {
    const user = userEvent.setup();
    const onRemove = vi.fn();
    render(<AttachmentChip attachment={{ name: "report.pdf", mimeType: "application/pdf" }} onRemove={onRemove} />);
    expect(screen.getByText("report.pdf")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Remove report.pdf" }));
    expect(onRemove).toHaveBeenCalledTimes(1);
  });

  it("shows an image thumbnail, with a preview button only when asked", async () => {
    const user = userEvent.setup();
    const onPreview = vi.fn();
    const attachment = { name: "shot.png", mimeType: "image/png", previewUrl: "blob:thumb" };
    const { container, rerender } = render(<AttachmentChip attachment={attachment} />);
    expect(container.querySelector("img")?.getAttribute("src")).toBe("blob:thumb");
    expect(screen.queryByRole("button")).toBeNull();
    rerender(<AttachmentChip attachment={attachment} onPreview={onPreview} />);
    await user.click(screen.getByRole("button", { name: "Open shot.png" }));
    expect(onPreview).toHaveBeenCalledTimes(1);
  });

  it("does not render an image for a non-image or a missing preview", () => {
    const { container } = render(<AttachmentChip attachment={{ name: "a.txt", mimeType: "text/plain", previewUrl: "blob:x" }} />);
    expect(container.querySelector("img")).toBeNull();
  });
});
