(() => {
  const anchor = Math.floor(Date.now() / 60000) * 60000;
  const names = {
    home: 'Home · Amsterdam',
    backup: 'Backup · Frankfurt',
    tokyo: 'Tokyo',
    zurich: 'Zurich',
    wan: 'Direct WAN',
  };
  const query = new URLSearchParams(window.location.search);
  const level = ['monitoring', 'unsupported', 'newer'].includes(query.get('compat'))
    ? query.get('compat')
    : 'full';
  const compatMessages = {
    full: '',
    monitoring: 'This firmware is newer than the tested range. Monitoring works.',
    unsupported: 'This firmware is older than 4.9.0. Wayline cannot control its VPN module.',
    newer: 'The VPN module is newer than the tested one. Wayline works but is not verified.',
  };
  const featureList = (on) =>
    ['Monitoring', 'History', 'Exit selection', 'Start and Stop', 'Signed updates'].map(
      (name, i) => ({
        name,
        available: i < on || i === 4,
      }),
    );
  const features = { full: 4, newer: 4, monitoring: 2, unsupported: 0 };
  let revision = 1;
  const state = {
    version: '0.1.0-alpha.3',
    revision: '1',
    scope: 'All clients',
    compat: {
      level: level === 'newer' ? 'full' : level,
      verified: level !== 'newer',
      firmware: level === 'unsupported' ? '4.8.1' : '4.9.2',
      message: compatMessages[level],
      features: level === 'full' ? [] : featureList(features[level]),
    },
    capabilities: { configure: true, select: true, control: true },
    config: {
      enabled: true,
      target: '1.1.1.1',
      interval: 15,
      tunnels: ['home', 'backup', 'tokyo'],
      diagnostics: true,
      targets: ['1.1.1.1', '9.9.9.9', 'example.com', 'missing.invalid'],
      directWan: true,
      historyHours: 24,
      autoUpdates: false,
      autoExit: true,
      endpointDiagnostics: true,
    },
    auto: {
      enabled: true,
      last: {
        at: Date.now() - 12 * 60000,
        from: 'backup',
        to: 'home',
        reason: 'Home is 38% faster and healthy.',
      },
    },
    endpoints: [
      { tunnel: 'home', name: names.home, address: '203.0.113.10' },
      { tunnel: 'backup', name: names.backup, address: '198.51.100.24' },
    ],
    resolved: { 'example.com': '93.184.216.34', 'missing.invalid': null },
    tunnels: [
      {
        id: 'home',
        name: names.home,
        preferred: true,
        enabled: true,
        connected: true,
        controllable: true,
        stateLabel: 'Connected',
        age: '4s',
        median: 32,
        lost: 0,
        samples: 64,
        eligible: true,
        recent: Array.from({ length: 64 }, (_, i) => 32 + Math.sin(i * 1.8) * 4),
      },
      {
        id: 'backup',
        name: names.backup,
        preferred: false,
        enabled: true,
        connected: true,
        controllable: true,
        stateLabel: 'Connected',
        age: '7s',
        median: 48,
        lost: 0,
        samples: 64,
        eligible: true,
        recent: Array.from({ length: 64 }, (_, i) =>
          i === 42 ? null : 48 + Math.sin(i * 1.2) * 6 + (i > 28 && i < 34 ? 35 : 0),
        ),
      },
      {
        id: 'tokyo',
        name: names.tokyo,
        preferred: false,
        enabled: true,
        connected: true,
        controllable: true,
        stateLabel: 'Connected',
        age: '10s',
        median: 131,
        lost: 4,
        samples: 64,
        eligible: false,
        reason: 'Recent packet loss',
        recent: Array.from({ length: 64 }, (_, i) =>
          i > 37 && i < 42 ? null : 130 + Math.sin(i) * 17,
        ),
      },
      {
        id: 'zurich',
        name: names.zurich,
        preferred: false,
        enabled: false,
        connected: false,
        controllable: true,
        stateLabel: 'Stopped',
        age: 'unknown',
        median: null,
        lost: 0,
        samples: 0,
        eligible: false,
        historyAvailable: true,
        recent: [],
      },
    ],
    diagnostics: {
      '1.1.1.1': { home: 32, backup: 48, tokyo: 131, zurich: null, wan: 14 },
      '9.9.9.9': { home: 36, backup: 52, tokyo: 128, zurich: null, wan: 15 },
      'example.com': { home: 39, backup: 55, tokyo: 124, zurich: null, wan: 18 },
      'missing.invalid': { home: null, backup: null, tokyo: null, zurich: null, wan: null },
      '203.0.113.10': { wan: 21 },
      '198.51.100.24': { wan: 29 },
    },
  };
  state.tunnels.forEach((tunnel) => (tunnel.monitorId = tunnel.id));
  const copy = (o) => JSON.parse(JSON.stringify(o));
  const delay = (o) => new Promise((resolve) => setTimeout(() => resolve(copy(o)), 180));
  function series(id, target, start, end, step) {
    const targetBias = target === 'example.com' ? 7 : target === '9.9.9.9' ? 4 : 0;
    const base = { home: 32, backup: 48, tokyo: 130, wan: 14 }[id] || 32;
    const points = [];
    for (let at = start; at <= end; at += step * 60000) {
      const n = Math.round((at - anchor) / 60000) + 1440;
      const outage = id === 'home' && n > 950 && n < 964;
      const spike = id !== 'wan' && n > 640 && n < 670 ? 50 : 0;
      const value = outage
        ? null
        : Math.max(
            1,
            Math.round(base + targetBias + Math.sin(n / 13) * 4 + Math.sin(n * 1.7) * 3 + spike),
          );
      points.push({
        at,
        median: value,
        min: value === null ? null : value - 3,
        max: value === null ? null : value + 5 + (n % 91 === 0 ? 35 : 0),
        lost: outage ? 4 : 0,
        sent: 4,
      });
    }
    return { id, name: names[id], points };
  }
  window.previewApi = {
    get_state: () =>
      query.get('fail') === '1'
        ? Promise.reject(Error('RPC error: Object not found'))
        : Promise.resolve(
            level === 'unsupported'
              ? {
                  ...copy(state),
                  tunnels: [],
                  endpoints: [],
                  diagnostics: {},
                  resolved: {},
                  auto: { enabled: false, last: null },
                }
              : copy(state),
          ),
    set_config: async ({ config, expected_revision }) => {
      if (expected_revision !== state.revision) throw Error('State changed. Refresh first.');
      state.config = copy(config);
      state.revision = String(++revision);
      return delay({ applied: true });
    },
    select_tunnel: async ({ section_id, expected_revision }) => {
      if (expected_revision !== state.revision) throw Error('State changed. Refresh first.');
      const t = state.tunnels.find((t) => t.id === section_id);
      if (!t || !t.eligible) throw Error('Tunnel is not eligible.');
      if (!t.preferred) {
        state.tunnels.forEach((t) => (t.preferred = t.id === section_id));
        state.revision = String(++revision);
      }
      return delay({ applied: true });
    },
    set_tunnel_enabled: async ({ section_id, enabled, expected_revision }) => {
      if (expected_revision !== state.revision) throw Error('State changed. Refresh first.');
      const tunnel = state.tunnels.find((item) => item.id === section_id);
      if (!tunnel || !tunnel.controllable || typeof enabled !== 'boolean')
        throw Error('The tunnel state is invalid.');
      if (tunnel.enabled !== enabled) {
        Object.assign(tunnel, {
          enabled,
          connected: false,
          eligible: false,
          historyAvailable: true,
          stateLabel: enabled ? 'Enabled, offline' : 'Stopped',
          age: 'unknown',
          median: null,
          recent: [],
          samples: 0,
          lost: 0,
        });
        state.revision = String(++revision);
        Object.values(state.diagnostics).forEach((values) => (values[section_id] = null));
      }
      return delay({ applied: true, enabled });
    },
    get_history: ({ tunnel, target, start, end, step, include_wan }) =>
      Promise.resolve({
        series: [
          series(tunnel, target, start, end, step),
          ...(include_wan ? [series('wan', target, start, end, step)] : []),
        ],
      }),
    check_update: () =>
      delay({
        available: level === 'unsupported',
        message:
          level === 'unsupported'
            ? 'Wayline 0.2.0 is available. Preview only.'
            : 'Preview only. No update was downloaded.',
      }),
    install_update: () => delay({ message: 'Preview only. No update was installed.' }),
  };
})();
