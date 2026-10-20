import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { fileURLToPath } from 'node:url'

/**
 * 原型构建配置（一次性，勿并入正式构建）
 *
 * 为什么不复用 vite.config.ts：那份的产物 `ui/islands/ui.js` **要入库**，且 CI
 * 会逐字节比对同源性。原型产物绝不能混进那两个文件，所以这里单起一份配置，
 * 输出到 `ui/prototype/`（不纳管、可随时删）。
 *
 * 用法见同目录 README.md。
 */
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '@ui': fileURLToPath(new URL('../ui-kit/src', import.meta.url)),
    },
    dedupe: ['react', 'react-dom', '@base-ui/react'],
  },
  define: {
    'process.env.NODE_ENV': JSON.stringify('production'),
  },
  build: {
    outDir: '../ui/prototype',
    emptyOutDir: true,
    lib: {
      entry: 'src/prototype/mount.prototype.tsx',
      name: 'WbPrototype',
      formats: ['iife'],
      fileName: () => 'prototype.js',
      cssFileName: 'prototype',
    },
    minify: false,
    sourcemap: true,
    target: 'es2020',
  },
})
