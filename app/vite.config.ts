import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// The Tauri CLI expects a fixed port and no screen clearing.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { target: "es2022", outDir: "dist", emptyOutDir: true, sourcemap: false },
  test: { environment: "node", include: ["src/**/*.test.ts"] },
});
