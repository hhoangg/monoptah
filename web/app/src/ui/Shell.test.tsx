import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { act, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { getLinkStore } from "@/link/store";
import { DesktopSwitcher } from "@/features/desktops/DesktopSwitcher";
import { mountApp } from "@/features/desktops/testMount";
import { clearSlots, registerSlot } from "./slots";

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

describe("app shell frame and navigation", () => {
  it("names the account entry Account and keeps Desktops and Install in the header nav", async () => {
    mountApp("/desktops");
    const nav = await screen.findByRole("navigation", { name: "Main" });
    expect(within(nav).getByRole("link", { name: "Account" })).toHaveAttribute("href", "/settings");
    expect(within(nav).getByRole("link", { name: "Desktops" })).toHaveAttribute("aria-current", "page");
    expect(within(nav).getByRole("link", { name: "Install" })).toBeInTheDocument();
    expect(within(nav).queryByRole("link", { name: "Settings" })).toBeNull();
  });

  it("marks Account as current on /admin", async () => {
    mountApp("/admin");
    const nav = await screen.findByRole("navigation", { name: "Main" });
    expect(within(nav).getByRole("link", { name: "Account" })).toHaveAttribute("data-current", "true");
  });

  it("has no header row on /d routes: the nav icons share the desktop switcher row, right aligned", async () => {
    clearSlots();
    registerSlot("desktop-switcher", DesktopSwitcher);
    try {
      mountApp("/d/dk-frame/settings");
      const switcher = await screen.findByRole("button", { name: /Switch desktop/ });
      const nav = screen.getByRole("navigation", { name: "Main" });
      expect(screen.queryByRole("banner")).toBeNull();
      expect(screen.queryByRole("link", { name: "Monoptah" })).toBeNull();
      // One row: the switcher and the nav share the same parent, nav last.
      const row = nav.parentElement as HTMLElement;
      expect(row).toContainElement(switcher);
      expect(row.lastElementChild).toBe(nav);
      expect(within(nav).getByRole("link", { name: "Desktops" })).toHaveAttribute("href", "/desktops");
      expect(within(nav).getByRole("link", { name: "Account" })).toHaveAttribute("href", "/settings");
      // The switcher row is inside the single main landmark.
      expect(screen.getByRole("main")).toContainElement(nav);
    } finally {
      clearSlots();
    }
  });

  it("is a fixed-height column on /d routes and a normal page elsewhere", async () => {
    mountApp("/d/dk-frame/settings");
    await screen.findByRole("main");
    expect(screen.getByRole("main").parentElement).toHaveClass("h-dvh", "overflow-hidden");
    expect(screen.getByRole("main")).toHaveClass("min-h-0", "flex-1");
  });

  it("blocks a desktop that is not in the account's list with a way out (B-6)", async () => {
    mountApp("/d/dk-removed");
    // The link of a removed desktop is turned away (revoked); a link that is merely connecting keeps its own screens.
    await screen.findByTestId("route-stub");
    act(() => getLinkStore("dk-removed").setState({ blocked: "revoked", status: "closed" }));
    expect(await screen.findByRole("heading", { level: 1, name: "This desktop is no longer in your account" })).toBeInTheDocument();
    expect(screen.getByText("It was revoked or removed. Pair it again to use it from the web.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Go to Desktops" })).toHaveAttribute("href", "/desktops");
    expect(screen.getByRole("link", { name: "Pair a desktop" })).toHaveAttribute("href", "/desktops");
    expect(screen.queryByTestId("route-stub")).toBeNull();
  });

  it("treats a closed link with a directory 404 as gone too, but not while it is still connecting (B-6)", async () => {
    mountApp("/d/dk-removed");
    await screen.findByTestId("route-stub");
    act(() => getLinkStore("dk-removed").setState({ status: "connecting", lastError: { code: "directory_404" } }));
    expect(screen.queryByTestId("desktop-gone-screen")).toBeNull();
    act(() => getLinkStore("dk-removed").setState({ status: "closed", blocked: null, lastError: { code: "directory_404" } }));
    expect(await screen.findByRole("heading", { level: 1, name: "This desktop is no longer in your account" })).toBeInTheDocument();
    expect(screen.queryByTestId("route-stub")).toBeNull();
  });
});
