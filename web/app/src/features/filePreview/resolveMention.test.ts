import { describe, expect, it } from "vitest";
import { routeMention } from "./resolveMention";

const project = { path: "/Users/me/work/app" };

describe("routeMention", () => {
  it("sends a project-relative text or source file to the Files route", () => {
    expect(routeMention("src/main.ts", project)).toEqual({ to: "project", path: "src/main.ts" });
    expect(routeMention("./src/main.ts", project)).toEqual({ to: "project", path: "src/main.ts" });
    expect(routeMention("Cargo.toml", project)).toEqual({ to: "project", path: "Cargo.toml" });
    expect(routeMention("notes.txt", project)).toEqual({ to: "project", path: "notes.txt" });
  });

  it("sends an absolute path under the project to the Files route as a relative path", () => {
    expect(routeMention("/Users/me/work/app/src/main.ts", project)).toEqual({ to: "project", path: "src/main.ts" });
    expect(routeMention("/Users/me/work/app/src/main.ts", { path: "/Users/me/work/app/" })).toEqual({ to: "project", path: "src/main.ts" });
  });

  it("previews a path outside the project, ~ paths, and a sibling folder with the same prefix", () => {
    expect(routeMention("/Users/me/other/main.ts", project)).toEqual({ to: "preview" });
    expect(routeMention("/Users/me/work/app2/main.ts", project)).toEqual({ to: "preview" });
    expect(routeMention("~/Documents/x/main.ts", project)).toEqual({ to: "preview" });
    expect(routeMention("../x/main.ts", project)).toEqual({ to: "preview" });
  });

  it("previews html, markdown, svg, images, binaries and unknown files even inside the project", () => {
    for (const path of ["report.html", "out/index.htm", "docs/findings.md", "diagram.svg", "chart.png", "tool.bin", "Makefile.unknownext", ".env", "archive.zip"]) {
      expect(routeMention(path, project), path).toEqual({ to: "preview" });
    }
  });

  it("previews everything when there is no project (Mono chats)", () => {
    expect(routeMention("src/main.ts", undefined)).toEqual({ to: "preview" });
  });
});
