// xterm.js (the DOM renderer and the scrollbar) puts its theme, cell size and scrollbar rules into <style> elements it
// creates at run time. The production CSP (`style-src 'self'`, no 'unsafe-inline', security.md 8.1) blocks every inline
// <style>, and the terminal would render with no colors and wrong cell sizes. `vite-plugins/xtermStaticStyles.ts` replaces
// those three `createElement("style")` calls in the xterm bundle with `createSheetStyle()`: a hidden element that stands
// in for the <style> element (xterm only appends it, sets `textContent` and calls `remove()`), while the CSS text goes
// into a constructed stylesheet that is adopted by the document. CSSOM construction is not subject to the inline style
// check, the same way as `src/patches/style-singleton.ts`. Needs adopted stylesheets (Safari 16.4+).

/** True when the browser can adopt a constructed stylesheet (jsdom and Safari before 16.4 cannot). */
function canAdopt(): boolean {
  return typeof CSSStyleSheet === "function" && "adoptedStyleSheets" in document && typeof CSSStyleSheet.prototype.replaceSync === "function";
}

export function createSheetStyle(): HTMLElement {
  // Without adoptable sheets fall back to the real element (a test environment, which has no CSP).
  if (!canAdopt()) return document.createElement("style");
  const holder = document.createElement("div");
  holder.hidden = true;
  const sheet = new CSSStyleSheet();
  let css = "";
  let adopted = false;
  const drop = (): void => {
    if (!adopted) return;
    adopted = false;
    document.adoptedStyleSheets = document.adoptedStyleSheets.filter((other) => other !== sheet);
  };
  Object.defineProperty(holder, "textContent", {
    configurable: true,
    get: () => css,
    set: (value: string | null) => {
      css = value ?? "";
      sheet.replaceSync(css);
      if (!adopted) {
        adopted = true;
        document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
      }
    },
  });
  const remove = holder.remove.bind(holder);
  holder.remove = (): void => {
    drop();
    remove();
  };
  return holder;
}
