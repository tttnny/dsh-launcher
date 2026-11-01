import { fileURLToPath, URL } from 'node:url'
import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

// https://vite.dev/config/
// The Vite root is src/, so the HTML entry lives at src/index.html instead of
// cluttering the repository root. The build still emits dist/index.html, which
// is the page src-tauri/src/windows.rs loads in the window.
export default defineConfig({
  root: fileURLToPath(new URL('./src', import.meta.url)),
  plugins: [vue()],
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },
  // Tauri expects a fixed port in development.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // Rust build output and editor/atomic-save temp dirs must not trigger
      // rebuilds.
      ignored: ['**/src-tauri/**', '**/.*/**', '**/.*.tmpdir/**'],
    },
  },
  envPrefix: ['VITE_', 'TAURI_'],
  build: {
    // outDir sits outside the root, so Vite needs permission to clean it.
    outDir: fileURLToPath(new URL('./dist', import.meta.url)),
    emptyOutDir: true,
    target: 'es2021',
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
})
