import { defineConfig } from 'vitest/config'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  test: {
    environment: 'node',
    // Coverage configuration consumed by `vitest run --coverage` (CI
    // `test:coverage` script). Reporter `cobertura` produces the same
    // XML shape as the Rust workspace's llvm-cov output, so both
    // streams flow into Codecov without per-language config tweaks.
    coverage: {
      provider: 'v8',
      reporter: ['text-summary', 'cobertura', 'html'],
      reportsDirectory: './coverage',
      // Ignore non-app paths from the report so the percentage tracks
      // production code, not tooling/configs/test fixtures.
      exclude: [
        'node_modules/',
        'dist/',
        'coverage/',
        'src/**/*.test.{ts,tsx}',
        'src/**/*.spec.{ts,tsx}',
        '**/*.config.{ts,js}',
        'eslint.config.{ts,js}',
        'src/main.tsx',
        'src/vite-env.d.ts',
      ],
      include: ['src/**/*.{ts,tsx}'],
    },
  },
  server: {
    proxy: {
      '/api': {
        target: 'http://localhost:8080',
        changeOrigin: true,
        rewrite: (path) => path.replace(/^\/api/, ''),
      },
    },
  },
})
