import { defineConfig } from 'vite';
import wasmPack from 'vite-plugin-wasm-pack';

export default defineConfig({
  // pass your local crate path to the plugin
  // plugins: [wasmPack('../rust-wasm-vite')]
  // server: {
  //   headers: {

  //   }
  // }
});
