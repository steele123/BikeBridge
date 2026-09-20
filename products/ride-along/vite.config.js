import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: {
    fs: { allow: ['..'] },
    port: 1420, strictPort: true,
    watch: { ignored: ['**/src-tauri/**'] },
    proxy: {
      '/api': { target: 'http://127.0.0.1:9376', changeOrigin: true, configure: localOrigin },
      '/ws': { target: 'ws://127.0.0.1:9376', ws: true, changeOrigin: true, configure: localOrigin }
    }
  },
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  build: { target: 'es2022' }
});

// Browser development only. The packaged app uses its restricted native client.
function localOrigin(proxy) {
  for (const event of ['proxyReq', 'proxyReqWs']) {
    proxy.on(event, (request, incoming) => {
      if (incoming.headers.origin === `http://${incoming.headers.host}`) {
        request.setHeader('Origin', 'http://127.0.0.1:9376');
      }
    });
  }
}
