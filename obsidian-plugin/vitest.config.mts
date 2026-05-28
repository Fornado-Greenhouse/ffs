import { defineConfig } from "vitest/config";

// vitest runs in Node; the plugin code imports `obsidian` only from
// `main.ts` (the entrypoint), which we don't unit-test directly.
// The testable units (`client.ts`, `events.ts`, `backoff.ts`,
// `settings.ts`'s renderSettings) live behind type-only imports of
// `obsidian`; alias to a stub so vitest resolves them.

export default defineConfig({
  resolve: {
    alias: {
      obsidian: new URL("./tests/obsidian-shim.ts", import.meta.url).pathname,
    },
  },
  test: {
    include: ["tests/**/*.test.ts"],
    environment: "node",
    coverage: {
      provider: "v8",
      reporter: ["text"],
      include: ["src/**/*.ts"],
      exclude: ["src/main.ts"],
    },
  },
});
