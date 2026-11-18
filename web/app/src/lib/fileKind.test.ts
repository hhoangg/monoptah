import { describe, expect, it } from "vitest";
import { fileKind, languageId, leafName, pathSegments } from "./fileKind";

describe("leafName", () => {
  it("returns the last segment for / and \\ paths", () => {
    expect(leafName("src/app/main.ts")).toBe("main.ts");
    expect(leafName("src\\app\\main.ts")).toBe("main.ts");
    expect(leafName("src/app/")).toBe("app");
    expect(leafName("")).toBe("");
    expect(pathSegments("/a//b/")).toEqual(["a", "b"]);
  });
});

describe("fileKind", () => {
  it.each([
    ["main.ts", "code"],
    ["lib.d.ts", "code"],
    ["index.html", "markup"],
    ["app.scss", "style"],
    ["data.json", "data"],
    ["README.md", "markdown"],
    ["logo.PNG", "image"],
    ["a.tar.gz", "archive"],
    ["run.sh", "shell"],
    ["Cargo.lock", "lock"],
    ["pnpm-lock.yaml", "lock"],
    [".env", "secret"],
    [".env.local", "secret"],
    ["Dockerfile", "config"],
    ["package.json", "config"],
    ["x.wasm", "binary"],
    ["noext", "file"],
    ["src/deep/main.rs", "code"],
  ])("%s -> %s", (name, kind) => {
    expect(fileKind(name)).toBe(kind);
  });

  it("does not treat prototype keys as names", () => {
    expect(fileKind("constructor")).toBe("file");
    expect(languageId("toString")).toBeNull();
  });
});

describe("languageId", () => {
  it("maps names and extensions to shiki ids", () => {
    expect(languageId("a/b.tsx")).toBe("tsx");
    expect(languageId("Dockerfile")).toBe("docker");
    expect(languageId("x.yml")).toBe("yaml");
    expect(languageId("lib.d.ts")).toBe("typescript");
    expect(languageId("photo.png")).toBeNull();
    expect(languageId("noext")).toBeNull();
  });
});
