import { playwright } from "@vitest/browser-playwright";
import { build } from "esbuild";
import { defineConfig } from "vitest/config";

const ALL_BROWSERS = ["chromium", "firefox", "webkit"] as const;
const selected = process.env.OPPORTUNE_WEB_BROWSERS
  ? process.env.OPPORTUNE_WEB_BROWSERS.split(",").map((b) => b.trim()).filter(Boolean)
  : ALL_BROWSERS;

export default defineConfig({
  server: {
    fs: {
      // The suite reads the goldens and adapter fixtures from their crates beside this
      // harness. Nothing else outside the project root is served.
      allow: ["..", "../../../opportune-testkit", "../../../opportune-adapters"],
    },
  },
  optimizeDeps: {
    exclude: ["opportune"],
  },
  plugins: [{
    name: "opportune-worker-bundle",
    async buildStart() {
      await build({
        absWorkingDir: import.meta.dirname,
        entryPoints: ["test/worker.ts"],
        outfile: ".vitest/worker.js",
        bundle: true,
        format: "esm",
        // Neutral excludes esbuild's implicit `browser` condition: the package's
        // declaration order would otherwise select that loader even here.
        platform: "neutral",
        conditions: ["worker"],
        target: "es2024",
        logLevel: "silent",
      });
    },
  }],
  test: {
    browser: {
      enabled: true,
      headless: true,
      provider: playwright(),
      instances: selected.map((browser) => ({
        browser: browser as (typeof ALL_BROWSERS)[number],
      })),
    },
  },
});
