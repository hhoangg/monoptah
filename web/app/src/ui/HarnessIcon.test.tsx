import { render } from "@testing-library/react";
import { readdirSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { HARNESS_IDS } from "@monoptah/protocol";
import { describe, expect, it } from "vitest";
import { HARNESS_ICONS, HarnessIcon, MONOCHROME_HARNESSES, PROVIDER_NOTICE, ProviderNotice } from "./HarnessIcon";

const PROVIDERS = resolve(import.meta.dirname, "../assets/providers");

describe("provider notice", () => {
  it("is the NOTICE file, rendered by the app so it ships in the build", () => {
    expect(PROVIDER_NOTICE).toBe(readFileSync(join(PROVIDERS, "NOTICE"), "utf8").trim());
    const { getByTestId } = render(<ProviderNotice />);
    expect(getByTestId("provider-notice")).toHaveTextContent("are trademarks of their respective owners");
  });
});

describe("HarnessIcon", () => {
  it("has a logo for each of the 11 protocol harnesses", () => {
    expect(Object.keys(HARNESS_ICONS).sort()).toEqual([...HARNESS_IDS].sort());
    expect(HARNESS_IDS).toHaveLength(11);
  });

  it("renders a full-colour provider as an img and never injects markup", () => {
    const { container } = render(<HarnessIcon harness="claude" />);
    const img = container.querySelector("img");
    expect(img).not.toBeNull();
    expect(img?.getAttribute("alt")).toBe("");
    expect(container.querySelector("svg")).toBeNull();
  });

  it("renders a monochrome provider as a currentColor mask", () => {
    const { container } = render(<HarnessIcon harness="cursor" />);
    expect(container.querySelector("img")).toBeNull();
    const mark = container.querySelector("span > span") as HTMLElement;
    expect(mark.style.backgroundColor).toBe("currentcolor");
    expect(MONOCHROME_HARNESSES.has("cursor")).toBe(true);
  });

  it("renders nothing for an unknown harness id", () => {
    const { container } = render(<HarnessIcon harness="mystery" />);
    expect(container.firstChild).toBeNull();
  });

  it("ships the trademark NOTICE and only inert SVG files", () => {
    const notice = readFileSync(join(PROVIDERS, "NOTICE"), "utf8");
    expect(notice).toMatch(/trademarks of their respective owners/);
    const svgs = readdirSync(PROVIDERS).filter((f) => f.endsWith(".svg"));
    expect(svgs).toHaveLength(11);
    for (const f of svgs) {
      expect(readFileSync(join(PROVIDERS, f), "utf8")).not.toMatch(/<script|<image|href=|onload/i);
    }
  });
});
