import type { CSSProperties } from "react";
// Sonner injects its stylesheet with a <style> element at runtime, which the production CSP (style-src 'self')
// blocks, leaving toasts unstyled. The same file is bundled here as a normal stylesheet instead.
import "sonner/dist/styles.css";
import { Toaster as Sonner, type ToasterProps } from "sonner";
import { useAppearance } from "@/lib/appearance";

// Toasts (design-system.md section 8): glass-like surface, radius 16, 1px stroke, no shadow
// (the shadow of sonner's own stylesheet is removed in styles/overlays.css).
export function Toaster(props: ToasterProps) {
  const scheme = useAppearance((state) => state.scheme);
  return (
    <Sonner
      theme={scheme}
      position="top-center"
      // Below the header pill, which the default offset would cover on phones.
      offset={{ top: "4.5rem" }}
      mobileOffset={{ top: "4.5rem" }}
      style={
        {
          "--normal-bg": "color-mix(in srgb, var(--surface) 92%, transparent)",
          "--normal-text": "var(--content)",
          "--normal-border": "var(--stroke)",
          "--border-radius": "16px",
        } as CSSProperties
      }
      {...props}
    />
  );
}

export { toast } from "sonner";
