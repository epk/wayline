import '../preview/demo.js';
import { createAgent } from '@ui-sdk/react/agent';
import { decompress } from 'fzstd';
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from '../src/app.tsx';
import { setLoader, type ChunkName } from '../src/runtime.ts';
import { setZstd } from '../src/sources/arrow.ts';
import type { DataSource } from '../src/sources/source.ts';
import { stop } from '../src/store.ts';

type Allow = Parameters<typeof createAgent>[0] extends infer O
  ? O extends { allow?: infer A }
    ? A
    : never
  : never;

const chunks: Record<ChunkName, () => Promise<Record<string, unknown>>> = {
  paths: () => import('../src/views/paths.tsx'),
  wifi: () => import('../src/views/wifi.tsx'),
  traffic: () => import('../src/views/traffic.tsx'),
  router: () => import('../src/views/router.tsx'),
  events: () => import('../src/views/events.tsx'),
  settings: () => import('../src/settings.tsx'),
  agent: () => import('../src/agent.tsx'),
};

export async function boot(host: HTMLElement, allow: Allow) {
  setZstd(decompress);
  setLoader((name) => chunks[name]());
  const agent = createAgent({ allow });
  const source = (window as unknown as { previewSource: DataSource }).previewSource;
  const root = createRoot(host);
  await act(async () => {
    root.render(<App source={source} preview />);
  });
  return {
    agent,
    act,
    unmount: () =>
      act(() => {
        root.unmount();
        stop();
      }),
  };
}
