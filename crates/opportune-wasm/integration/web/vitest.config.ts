import { playwright } from "@vitest/browser-playwright";
import { defineConfig } from "vitest/config";

const ALL_BROWSERS = ["chromium", "firefox", "webkit"] as const;
const selected = process.env.OPPORTUNE_WEB_BROWSERS
  ? process.env.OPPORTUNE_WEB_BROWSERS.split(",").map((b) => b.trim()).filter(Boolean)
  : ALL_BROWSERS;

export default defineConfig({
  server: {
    fs: {
      allow: ["..", "../../..", "../../../.."],
    },
  },
  optimizeDeps: {
    exclude: ["opportune"],
  },
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
