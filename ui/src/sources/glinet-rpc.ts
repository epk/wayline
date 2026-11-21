import { decodeHistory, latencyDoc, telemetryDoc, wifiDoc } from './arrow.ts';
import type { DataSource, HistoryQuery, LatencyQuery, UpdateReply } from './source.ts';

export type Transport = (
  object: string,
  method: string,
  params: Record<string, unknown>,
  timeout: number,
) => unknown;

export const reads = ['get_state', 'get_history', 'get_wifi_history', 'get_telemetry_history'];
export const writeObject = 'wayline_' + WAYLINE_VERSION.replace(/\./g, '_');
export const objectFor = (method: string): string =>
  reads.includes(method) ? 'wayline' : writeObject;
export const timeoutFor = (method: string): number =>
  method === 'set_tunnel_enabled' ? 75000 : method === 'select_tunnel' ? 45000 : 12000;
export const staleMessage = 'The Wayline version changed. Reload the page before another change.';

export class GlinetRpcSource implements DataSource {
  private version = '';
  private readonly transport: Transport;

  constructor(transport: Transport) {
    this.transport = transport;
  }

  private call<T>(method: string, params: object = {}): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      const timeout = timeoutFor(method);
      let request: unknown;
      try {
        if (!reads.includes(method) && this.version && this.version !== WAYLINE_VERSION)
          throw Error(staleMessage);
        request = this.transport(objectFor(method), method, { ...params }, timeout);
      } catch (e) {
        reject(e);
        return;
      }
      const clock = setTimeout(
        () => reject(Error('The request timed out. Refresh its status before another change.')),
        timeout,
      );
      Promise.resolve(request).then(
        (value) => {
          clearTimeout(clock);
          resolve(value as T);
        },
        (failure: unknown) => {
          clearTimeout(clock);
          reject(failure);
        },
      );
    });
  }

  async getState(): Promise<unknown> {
    const state = await this.call<{ version?: unknown }>('get_state');
    if (state && typeof state === 'object' && typeof state.version === 'string')
      this.version = state.version;
    return state;
  }

  async getHistory(query: LatencyQuery) {
    return decodeHistory(await this.call('get_history', { ...query, format: 'arrow' }), latencyDoc);
  }

  async getWifiHistory(query: HistoryQuery) {
    return decodeHistory(
      await this.call('get_wifi_history', { ...query, format: 'arrow' }),
      wifiDoc,
    );
  }

  async getTelemetryHistory(query: HistoryQuery) {
    return decodeHistory(
      await this.call('get_telemetry_history', { ...query, format: 'arrow' }),
      telemetryDoc,
    );
  }

  setConfig(request: object) {
    return this.call('set_config', request);
  }

  selectTunnel(request: object) {
    return this.call('select_tunnel', request);
  }

  setTunnelEnabled(request: object) {
    return this.call<{ enabled?: boolean; monitor_pending?: boolean }>(
      'set_tunnel_enabled',
      request,
    );
  }

  purgeHistory(request: object) {
    return this.call('purge_history', request);
  }

  checkUpdate() {
    return this.call<UpdateReply>('check_update');
  }

  installUpdate() {
    return this.call<UpdateReply>('install_update');
  }
}
