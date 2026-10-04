/// <reference types="vitest/config" />
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

const backend = 'http://127.0.0.1:8080';

export default defineConfig({
  plugins: [react()],
  build: { outDir: 'dist', emptyOutDir: true, sourcemap: false },
  server: {
    host: '127.0.0.1',
    proxy: {
      '/api': backend,
      '/health': backend,
      '/openapi.json': backend,
      '/openapi.yml': backend,
    },
  },
  test: { environment: 'node', include: ['src/**/*.test.ts'] },
});
