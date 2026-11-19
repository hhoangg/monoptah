// The markdown preview asks the desktop for an image with the same key the desktop lists (`../img/a.png`, an absolute
// path, a `file://` URL), and never for a remote one. Whether the desktop reads it (inside the project) is its call.
import { QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { createQueryClient } from "@/lib/queryClient";
import { MarkdownPreview } from "./MarkdownPreview";
import type { MentionedPage, MentionRequester } from "./mentionedFile";

const PNG_PAGE = { path: "/p/img/a.png", name: "a.png", size: 3, kind: "image", mime: "image/png", encoding: "base64", data: "AQID", offset: 0, total: 3 } as unknown as MentionedPage;

describe("markdown images", () => {
  it("sends back ../, absolute and file:// keys as written, and no remote url", async () => {
    const asked: Array<string | undefined> = [];
    const request: MentionRequester = (params) => {
      asked.push(params.asset);
      if (params.asset === "../../out.png") return Promise.reject(Object.assign(new Error("forbidden"), { code: "forbidden" }));
      return Promise.resolve(PNG_PAGE);
    };
    const text = "![a](../img/a.png) ![b](/p/img/b.png?v=1) ![c](file:///p/img/c%20d.png) ![e](https://e.test/e.png) ![f](../../out.png)";
    render(
      <QueryClientProvider client={createQueryClient()}>
        <MarkdownPreview text={text} path="docs/g.md" request={request} cacheKey={["t"]} />
      </QueryClientProvider>,
    );
    await waitFor(() => expect(screen.getAllByTestId("preview-md-image")).toHaveLength(3));
    expect([...asked].sort()).toEqual(["../../out.png", "../img/a.png", "/p/img/b.png", "/p/img/c d.png"].sort());
    // The refused one stays as its alt text, and the remote one never produced a request.
    expect(screen.getAllByTestId("preview-md-image-missing")).toHaveLength(1);
  });
});
