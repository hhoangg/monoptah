/* Blocking theme bootstrap (design-system.md 5.1 rule 1). Loaded from <head> with a plain
   <script src="/theme-init.js">, so it runs before first paint and needs no inline script.
   Reads monoptah.appearance, monoptah.theme and monoptah.accent from localStorage, resolves
   "system" through prefers-color-scheme and sets data-scheme, data-theme, data-accent and
   <meta name="theme-color"> on <html>. Keep BACKGROUNDS in step with src/styles/tokens.css
   (a test compares them) and the validation lists with src/styles/themes.ts. */
(function () {
  var BACKGROUNDS = {
    mono: ["#131313", "#f7f7f7"],
    ocean: ["#0e1216", "#f3f6f9"],
    grove: ["#0e1612", "#f3f9f6"],
    ember: ["#16110e", "#f9f5f3"],
    iris: ["#110e16", "#f5f3f9"],
    rose: ["#160e11", "#f9f3f5"],
  };
  var APPEARANCES = ["system", "light", "dark"];
  var ACCENTS = ["theme", "blue", "teal", "green", "orange", "red", "pink", "violet"];

  function read(key) {
    try {
      return window.localStorage.getItem(key);
    } catch {
      return null; // storage blocked: fall back to defaults
    }
  }
  function pick(value, allowed, fallback) {
    return allowed.indexOf(value) !== -1 ? value : fallback;
  }

  var appearance = pick(read("monoptah.appearance"), APPEARANCES, "system");
  var theme = pick(read("monoptah.theme"), Object.keys(BACKGROUNDS), "mono");
  var accent = pick(read("monoptah.accent"), ACCENTS, "theme");

  var dark = true;
  if (appearance === "light") dark = false;
  else if (appearance === "system" && typeof window.matchMedia === "function") {
    dark = window.matchMedia("(prefers-color-scheme: dark)").matches;
  }
  var scheme = dark ? "dark" : "light";

  var root = document.documentElement;
  root.setAttribute("data-scheme", scheme);
  root.setAttribute("data-theme", theme);
  root.setAttribute("data-accent", accent);

  var meta = document.querySelector('meta[name="theme-color"]');
  if (meta) meta.setAttribute("content", BACKGROUNDS[theme][dark ? 0 : 1]);
})();
