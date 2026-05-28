// esbuild bundler for the FFS Obsidian plugin.
//
// Obsidian plugins ship as a single bundled `main.js` next to
// `manifest.json` in the plugin directory. esbuild handles
// TypeScript compilation + bundling in one pass. The `obsidian`
// import resolves at Obsidian's runtime (not at build time), so
// it's marked external — same for Node's built-ins that the
// JSON-RPC client uses (`net`, `child_process`).

import { build, context } from "esbuild";

const isProduction = process.env.NODE_ENV === "production";

const options = {
  entryPoints: ["src/main.ts"],
  bundle: true,
  format: "cjs",
  target: "es2022",
  platform: "node",
  external: ["obsidian", "node:net", "net", "node:child_process", "child_process"],
  outfile: "main.js",
  sourcemap: isProduction ? false : "inline",
  minify: isProduction,
  treeShaking: true,
  logLevel: "info",
};

if (process.argv.includes("--watch")) {
  const ctx = await context(options);
  await ctx.watch();
  console.log("[esbuild] watching for changes...");
} else {
  await build(options);
}
