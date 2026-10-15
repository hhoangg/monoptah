import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { colorForPerson } from "../model/palette";
import { Avatar, initialsOf } from "./Avatar";

describe("initialsOf", () => {
  it("uses the first letters of the first and last word", () => {
    expect(initialsOf("Hoang Hoang")).toBe("HH");
    expect(initialsOf("ana maria de souza")).toBe("AS");
  });

  it("uses one letter for a single word and ignores an email domain", () => {
    expect(initialsOf("ana")).toBe("A");
    expect(initialsOf("ana.lee@example.com")).toBe("AL");
  });

  it("falls back to a question mark for nothing usable", () => {
    expect(initialsOf("")).toBe("?");
    expect(initialsOf("  ")).toBe("?");
  });
});

describe("Avatar", () => {
  it("renders the initials on the given color and hides itself from screen readers", () => {
    const markup = renderToStaticMarkup(
      createElement(Avatar, { name: "Tuan Nguyen", color: "#e3a008" }),
    );
    expect(markup).toContain(">TN<");
    expect(markup).toContain("background-color:#e3a008");
    expect(markup).toContain('aria-hidden="true"');
  });
});

describe("Avatar color", () => {
  it("derives the color from the person id when none is given", () => {
    const markup = renderToStaticMarkup(
      createElement(Avatar, { name: "Ana", personId: "ana@example.com" }),
    );
    expect(markup).toContain(
      `background-color:${colorForPerson("ana@example.com")}`,
    );
  });

  it("prefers an explicit color", () => {
    const markup = renderToStaticMarkup(
      createElement(Avatar, { name: "Ana", personId: "x", color: "#123456" }),
    );
    expect(markup).toContain("background-color:#123456");
  });
});
