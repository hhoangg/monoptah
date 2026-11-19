// S-08 patch 2 (spikes/s-08-csp/FINDINGS.md): a drop-in replacement for `react-style-singleton`, which Radix Dialog
// (through vaul's scroll lock) uses to write the scrollbar-gap rule. The original appends a <style> element at runtime,
// which `style-src 'self'` blocks. This version adopts a constructed stylesheet instead: CSSOM construction is not
// subject to the inline-style check. Same exports and semantics as the original. Needs adopted stylesheets (Safari 16.4+).
// vite.config.ts aliases `react-style-singleton` to this file.
import { useEffect, type FC } from "react";

export interface Sheet {
  add(css: string): void;
  remove(): void;
}

export const stylesheetSingleton = (): Sheet => {
  let counter = 0;
  let sheet: CSSStyleSheet | null = null;
  return {
    add(css) {
      if (counter === 0) {
        sheet = new CSSStyleSheet();
        sheet.replaceSync(css);
        document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
      }
      counter++;
    },
    remove() {
      counter--;
      if (counter === 0 && sheet) {
        const gone = sheet;
        document.adoptedStyleSheets = document.adoptedStyleSheets.filter((other) => other !== gone);
        sheet = null;
      }
    },
  };
};

export const styleHookSingleton = () => {
  const sheet = stylesheetSingleton();
  return (styles: string, isDynamic?: boolean) => {
    useEffect(() => {
      sheet.add(styles);
      return () => sheet.remove();
    }, [styles, isDynamic]);
  };
};

export const styleSingleton = (): FC<{ styles: string; dynamic?: boolean }> => {
  const useStyle = styleHookSingleton();
  return function StyleSingleton({ styles, dynamic }) {
    useStyle(styles, dynamic);
    return null;
  };
};
