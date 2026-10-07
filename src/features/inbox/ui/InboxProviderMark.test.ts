import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { expect, it } from "vitest";
import { InboxProviderMark } from "./InboxProviderMark";

it("draws the ClickUp mark inline instead of the generic inbox icon", () => {
  const clickup = renderToStaticMarkup(
    createElement(InboxProviderMark, {
      provider: "clickup",
      className: "size-4",
    }),
  );
  const fallback = renderToStaticMarkup(
    createElement(InboxProviderMark, { provider: "github" }),
  );
  expect(clickup).toContain("<svg");
  expect(clickup).toContain("#8930FD");
  expect(clickup).not.toContain("<img");
  expect(clickup).not.toBe(fallback);
});
