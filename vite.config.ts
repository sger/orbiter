import { defineConfig } from "vite";
import tailwindcss from "@tailwindcss/vite";

// Vite's defaults already handle this project, apart from Tailwind and the watcher.
//
// `tauri dev` runs this server alongside the Cargo build, so the watcher would otherwise walk
// `target/` while Cargo is writing it. On Windows those files are locked while open and the
// watcher dies with EBUSY, taking the dev server with it. Nothing under either directory is a
// frontend source, so there is no reason to watch them anywhere.
export default defineConfig({
  plugins: [tailwindcss()],
  server: {
    watch: {
      ignored: ["**/target/**", "**/src-tauri/**"],
    },
  },
});
