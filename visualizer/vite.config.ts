import vue from "@vitejs/plugin-vue";
import { defineConfig } from "vite";

const dashboardTarget = process.env.HZR_VISUALIZER_PROXY ?? "http://127.0.0.1:47391";

export default defineConfig({
  plugins: [vue()],
  publicDir: "public",
  server: {
    host: "127.0.0.1",
    port: 47392,
    strictPort: true,
    proxy: {
      "/v1/dashboard": dashboardTarget,
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    sourcemap: false,
    rollupOptions: {
      output: {
        entryFileNames: "assets/app.js",
        chunkFileNames: "assets/chunk-[name].js",
        assetFileNames: (asset) =>
          asset.names.some((name) => name.endsWith(".css"))
            ? "assets/app.css"
            : "assets/[name][extname]",
      },
    },
  },
});
