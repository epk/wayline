import { createRoot } from 'react-dom/client';
import { App } from './app.tsx';
import { setLoader, type Loader } from './runtime.ts';
import { setZstd } from './sources/arrow.ts';
import type { DataSource } from './sources/source.ts';

export { GlinetRpcSource } from './sources/glinet-rpc.ts';

export interface MountOptions {
  source: DataSource;
  preview: boolean;
  load: Loader;
  decompress: (bytes: Uint8Array, out: Uint8Array) => Uint8Array;
}

export function mount(el: HTMLElement, options: MountOptions): () => void {
  setLoader(options.load);
  setZstd(options.decompress);
  const root = createRoot(el);
  root.render(<App source={options.source} preview={options.preview} />);
  return () => root.unmount();
}
