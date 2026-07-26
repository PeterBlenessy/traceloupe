import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "node:path";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react(), tailwindcss()],

  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      //
      // `.claude/worktrees/**` matters just as much: this repo runs several
      // agents, each in a git worktree created INSIDE the project root, and each
      // holding a full copy of the tree (src/, dist/, docs/, node_modules/).
      // Without this, another agent creating a worktree or running a build makes
      // Vite issue a full `page reload` in whatever dev app is running — which
      // silently wipes frontend state. That is what made a running Safety Scan's
      // progress bar and finding counter freeze while the backend scanned on
      // perfectly happily: the page had been reloaded out from under it.
      //
      // The pattern must be anchored to THIS root, not a bare `**/.claude/**`:
      // a dev server started inside a worktree has its own root *under*
      // `.claude/worktrees/<slug>`, so the unanchored glob matched every one of
      // its own source files and HMR went silently dead — edits kept serving the
      // stale bundle. Anchoring ignores only the `.claude` nested in the current
      // root, which is what "other agents' worktrees" actually means.
      ignored: [
        path.resolve(__dirname, "src-tauri") + "/**",
        path.resolve(__dirname, ".claude") + "/**",
      ],
    },
  },
}));
