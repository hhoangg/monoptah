import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { mountApp } from "@/features/desktops/testMount";

// design-system.md section 9: a skip link as the first focusable element, and one `main` landmark.
describe("app shell landmarks", () => {
  it("has a skip link that is the first focusable element and moves focus to the single main", async () => {
    mountApp("/login");
    const skip = await screen.findByRole("link", { name: "Skip to content" });
    const main = screen.getByRole("main");
    expect(screen.getAllByRole("main")).toHaveLength(1);
    expect(skip).toHaveAttribute("href", `#${main.id}`);
    const user = userEvent.setup();
    await user.tab();
    expect(skip).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(main).toHaveFocus();
  });

  it("renders no `main` outside the shell (screens would nest a second landmark)", () => {
    const root = join(import.meta.dirname, "..");
    const offenders: string[] = [];
    const walk = (dir: string) => {
      for (const name of readdirSync(dir)) {
        const path = join(dir, name);
        if (statSync(path).isDirectory()) {
          if (name !== "dev") walk(path);
        } else if (/\.tsx$/.test(name) && !/\.test\.tsx$/.test(name) && !path.endsWith(join("ui", "Shell.tsx"))) {
          if (/<main[\s>]/.test(readFileSync(path, "utf8"))) offenders.push(path);
        }
      }
    };
    walk(root);
    expect(offenders).toEqual([]);
  });
});
