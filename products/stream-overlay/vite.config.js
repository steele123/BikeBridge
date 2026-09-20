import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
export default defineConfig({
  base: '/overlay/', plugins: [svelte()],
  build: { rollupOptions: { output: { entryFileNames: 'app.js', assetFileNames: 'app.[ext]' } } },
  server: { fs: { allow: ['..'] }, port: 1421, strictPort: true, proxy: {
    '/api': { target: 'http://127.0.0.1:9376', changeOrigin: true, configure: originProxy },
    '/ws': { target: 'ws://127.0.0.1:9376', ws: true, changeOrigin: true, configure: originProxy }
  } }
});
function originProxy(proxy) {
  for (const event of ['proxyReq', 'proxyReqWs']) {
    proxy.on(event, (request, incoming) => {
      if (incoming.headers.origin === `http://${incoming.headers.host}`) request.setHeader('Origin', 'http://127.0.0.1:9376');
    });
  }
}
