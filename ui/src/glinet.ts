import { sheets } from '\0wayline-styles';
import type { Root } from 'react-dom/client';
import type { GlinetRpcSource, Transport } from './sources/glinet-rpc.ts';
import type { DataSource } from './sources/source.ts';

interface Host {
  $el: HTMLElement;
  api?: Record<string, (params: unknown) => unknown> | null;
  source?: DataSource | null;
  preview?: boolean;
  $rpcRequest?: (method: string, params: unknown[], options: { timeout: number }) => unknown;
  waylineRoot?: Root | null;
  waylineGone?: boolean;
}

type CreateElement = (tag: string, data?: Record<string, unknown>) => unknown;

const styleId = 'wayline-style';

function inject(): void {
  const css = sheets.join('\n');
  const existing = document.getElementById(styleId);
  if (existing) {
    if (existing.textContent !== css) existing.textContent = css;
    return;
  }
  const style = document.createElement('style');
  style.id = styleId;
  style.textContent = css;
  document.head.appendChild(style);
}

const sourceFor = (host: Host, Rpc: typeof GlinetRpcSource): DataSource => {
  if (host.source) return host.source;
  const transport: Transport = (object, method, params, timeout) => {
    if (host.api) return host.api[method](params);
    if (typeof host.$rpcRequest !== 'function')
      throw Error('The router page API is missing. Reload the page or update the firmware.');
    return host.$rpcRequest('call', ['sid', object, method, params], { timeout });
  };
  return new Rpc(transport);
};

async function draw(host: Host): Promise<void> {
  const [{ createElement }, { createRoot }, { App }, { GlinetRpcSource }] = await Promise.all([
    import('react'),
    import('react-dom/client'),
    import('./app.tsx'),
    import('./sources/glinet-rpc.ts'),
  ]);
  if (host.waylineGone) return;
  inject();
  const root = createRoot(host.$el);
  host.waylineRoot = root;
  root.render(
    createElement(App, { source: sourceFor(host, GlinetRpcSource), preview: !!host.preview }),
  );
}

export default {
  name: 'Wayline',
  props: { api: Object, preview: Boolean, source: Object },
  render(this: Host, create: CreateElement) {
    return create('div', { class: 'wl-host' });
  },
  mounted(this: Host) {
    this.waylineGone = false;
    void draw(this);
  },
  beforeDestroy(this: Host) {
    this.waylineGone = true;
    this.waylineRoot?.unmount();
    this.waylineRoot = null;
  },
};
