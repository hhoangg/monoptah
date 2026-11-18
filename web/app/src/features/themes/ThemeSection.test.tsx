import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";
import { useAppearance } from "@/lib/appearance";
import { STORAGE_KEYS } from "@/styles/themes";
import { clearSlots, Slot } from "@/ui/slots";
import { ThemeSection } from "./ThemeSection";

beforeEach(() => {
  useAppearance.getState().setTheme("mono");
  useAppearance.getState().setAccent("theme");
  window.localStorage.clear();
});

function themeRadios() {
  return Array.from(screen.getByRole("radiogroup", { name: "Theme" }).querySelectorAll<HTMLElement>('[role="radio"]'));
}

describe("ThemeSection", () => {
  it("lists six themes and eight accent options as radios", () => {
    render(<ThemeSection />);
    expect(themeRadios().map((el) => el.textContent)).toEqual(["Classic", "Cobalt", "Jade", "Copper", "Violet", "Blush"]);
    const accents = screen.getByRole("radiogroup", { name: "Accent color" });
    expect(accents.querySelectorAll('[role="radio"]')).toHaveLength(8);
    expect(screen.getByRole("radio", { name: "Theme default" })).toHaveAttribute("aria-checked", "true");
  });

  it("applies a theme and an accent at once and persists them", async () => {
    const user = userEvent.setup();
    render(<ThemeSection />);
    const cobalt = themeRadios()[1]!;
    await user.click(cobalt);
    expect(document.documentElement.getAttribute("data-theme")).toBe("ocean");
    expect(cobalt).toHaveAttribute("aria-checked", "true");
    expect(window.localStorage.getItem(STORAGE_KEYS.theme)).toBe("ocean");

    await user.click(screen.getByRole("radio", { name: "Orange" }));
    expect(document.documentElement.getAttribute("data-accent")).toBe("orange");
    expect(screen.getByRole("radio", { name: "Orange" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("radio", { name: "Theme default" })).toHaveAttribute("aria-checked", "false");
    expect(window.localStorage.getItem(STORAGE_KEYS.accent)).toBe("orange");
  });

  it("moves and selects with the arrow keys and keeps one tab stop per group", async () => {
    const user = userEvent.setup();
    render(<ThemeSection />);
    const radios = themeRadios();
    expect(radios.filter((el) => el.tabIndex === 0)).toHaveLength(1);
    radios[0]!.focus();
    await user.keyboard("{ArrowRight}");
    expect(useAppearance.getState().theme).toBe("ocean");
    expect(radios[1]).toHaveFocus();
    await user.keyboard("{End}");
    expect(useAppearance.getState().theme).toBe("rose");
    await user.keyboard("{ArrowRight}");
    expect(useAppearance.getState().theme).toBe("mono");
  });

  it("is rendered by the settings-sections registration", async () => {
    clearSlots();
    await import("./register");
    render(<Slot name="settings-sections" />);
    expect(screen.getByRole("radiogroup", { name: "Theme" })).toBeInTheDocument();
    clearSlots();
  });
});
