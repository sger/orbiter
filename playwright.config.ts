import { defineConfig } from "@playwright/test";
export default defineConfig({
  testDir: "./tests",
  use: {
    baseURL: "http://127.0.0.1:1421",
    viewport: { width: 1120, height: 840 },
    channel: "chrome",
  },
  // Its own port and its own server, never the one `npm run tauri dev` is using. Reusing that
  // one meant tests silently ran against whatever bundle it had started with: a Vite config added
  // afterwards was invisible to them, so they passed against a build nobody was shipping.
  webServer: {
    command: "npm run dev:test",
    url: "http://127.0.0.1:1421",
    reuseExistingServer: false,
  },
  reporter: "list",
});
