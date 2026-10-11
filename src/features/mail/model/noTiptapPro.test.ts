import { readdirSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { describe, expect, it } from "vitest";

const root = resolve(__dirname, "../../../..");

function npmrcFiles(dir: string, found: string[] = []): string[] {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (["node_modules", ".git", "target", "dist"].includes(entry.name))
      continue;
    const path = join(dir, entry.name);
    if (entry.isDirectory()) npmrcFiles(path, found);
    else if (entry.name === ".npmrc") found.push(path);
  }
  return found;
}

// Only free TipTap may ship. A Pro package would arrive as a "just this once"
// transitive; this is the tripwire.
describe("free TipTap only", () => {
  it("declares nothing under @tiptap-pro/", () => {
    const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
    const names = Object.keys({
      ...pkg.dependencies,
      ...pkg.devDependencies,
      ...pkg.optionalDependencies,
      ...pkg.peerDependencies,
    });
    expect(names.filter((name) => name.startsWith("@tiptap-pro/"))).toEqual([]);
  });

  it("maps no scope to registry.tiptap.dev in any .npmrc", () => {
    for (const file of npmrcFiles(root)) {
      expect(readFileSync(file, "utf8"), file).not.toContain(
        "registry.tiptap.dev",
      );
    }
  });
});
