export type ChunkName = 'paths' | 'wifi' | 'traffic' | 'router' | 'events' | 'settings' | 'agent';
export type Loader = (name: ChunkName) => Promise<Record<string, unknown>>;

export const chunkNames: ChunkName[] = ['paths', 'wifi', 'traffic', 'router', 'events', 'settings'];

export const startAgentBridge = (host: Element): (() => void) => {
  if (!('modelContext' in document)) return () => undefined;
  let stop: (() => void) | null = null;
  let gone = false;
  void loadChunk('agent').then((module) => {
    if (!gone) stop = (module.startAgent as (el: Element) => () => void)(host);
  });
  return () => {
    gone = true;
    stop?.();
  };
};

let loader: Loader | null = null;

export const setLoader = (next: Loader): void => {
  loader = next;
};

export const loadChunk = (name: ChunkName): Promise<Record<string, unknown>> =>
  loader ? loader(name) : Promise.reject(Error('The Wayline chunk loader is missing.'));

const idle = (run: () => void): void => {
  const w = globalThis as { requestIdleCallback?: (cb: () => void, o?: object) => number };
  if (typeof w.requestIdleCallback === 'function') w.requestIdleCallback(run, { timeout: 8000 });
  else setTimeout(run, 2000);
};

export const prefetch = (): void => {
  idle(() => {
    chunkNames.forEach((name) => {
      void loadChunk(name).catch(() => undefined);
    });
  });
};
