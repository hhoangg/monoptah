import { build } from "esbuild";
import { copyFile, readFile } from "node:fs/promises";

const hostVersion = (await readFile("host/VERSION", "utf8")).trim();

await build({
  entryPoints: ["host/cli.ts"],
  outfile: "build/host/monocode-host.mjs",
  bundle: true,
  platform: "node",
  format: "esm",
  target: "node24",
  loader: { ".ps1": "text" },
  define: {
    "import.meta.hot": "undefined",
    __HOST_VERSION__: JSON.stringify(hostVersion),
  },
  sourcemap: true,
});
await copyFile("host/provider-guard.mjs", "build/host/provider-guard.mjs");
