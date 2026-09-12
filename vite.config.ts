import { defineConfig } from "vite";
import tailwindcss from "@tailwindcss/vite";

// Vite's defaults already handle this project; the only addition is Tailwind.
export default defineConfig({
  plugins: [tailwindcss()],
});
