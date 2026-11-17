// Drop-in replacement for `react-style-singleton` (used by Radix Dialog through vaul's scroll lock).
// The original appends a `<style>` element at runtime, which `style-src 'self'` blocks. This version
// adopts a constructed stylesheet instead: CSSOM construction is not subject to the inline-style
// check, and no `style-src` relaxation is needed. Same exports and semantics as the original.
import { useEffect, type FC } from "react";

interface Sheet { add(css: string): void; remove(): void }

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
        document.adoptedStyleSheets = document.adoptedStyleSheets.filter((s) => s !== gone);
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
    }, [styles && isDynamic]);
  };
};

export const styleSingleton = (): FC<{ styles: string; dynamic?: boolean }> => {
  const useStyle = styleHookSingleton();
  return ({ styles, dynamic }) => {
    useStyle(styles, dynamic);
    return null;
  };
};
