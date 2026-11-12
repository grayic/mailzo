import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// In a browser the screens reach the engine through the dev host. In the Mac
// app they call it directly, and this proxy is not used.
// INBOX_DEVHOST_SHIFT moves both ports, so a second copy can run beside one already going.
declare const process: { env: Record<string, string | undefined> };
const shift = Number(process.env.INBOX_DEVHOST_SHIFT ?? 0);

export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  server: {
    port: 5177 + shift,
    strictPort: true,
    host: "127.0.0.1",
    proxy: {
      "/api": {
        target: `http://127.0.0.1:${4790 + shift}`,
        rewrite: (path) => path.replace(/^\/api/, ""),
      },
    },
  },
  build: { outDir: "dist", target: "safari15" },
});
