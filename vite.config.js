import { defineConfig } from "vite";
import { resolve } from "path";

export default defineConfig({
  clearScreen: false,
  server: {
    strictPort: true,
    port: 5173,
  },
  build: {
    target: ["es2020", "chrome100", "safari13"],
    rollupOptions: {
      input: {
        main: resolve(__dirname, "index.html"),
        blank: resolve(__dirname, "blank.html"),
        empty: resolve(__dirname, "empty.html"),
        divider: resolve(__dirname, "divider.html"),
        intro: resolve(__dirname, "intro.html"),
      },
    },
  },
});
