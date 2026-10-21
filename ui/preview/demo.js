(() => {
  const anchor = Math.floor(Date.now() / 60000) * 60000;
  const names = {
    home: 'Tunnel A',
    backup: 'Tunnel B',
    tokyo: 'Tunnel C',
    zurich: 'Tunnel D',
    wan: 'Direct WAN',
  };
  const query = new URLSearchParams(window.location.search);
  const level = ['monitoring', 'unsupported', 'newer'].includes(query.get('compat'))
    ? query.get('compat')
    : 'full';
  const wanDown = query.get('wan') === 'unavailable';
  const updateFailed = query.get('update') === 'failed';
  const release = '0.1.0-alpha.10';
  const directExit = query.get('exit') === 'direct';
  const wanMs = wanDown ? null : 14.37;
  const compatMessages = {
    full: '',
    monitoring: 'This firmware is newer than the tested range. Monitoring works.',
    unsupported: 'This firmware is older than 4.9.0. Wayline cannot control its VPN module.',
    newer: 'The VPN module is unverified. Monitoring and updates work. Tunnel controls are off.',
  };
  const featureList = (on) =>
    ['Monitoring', 'History', 'Exit selection', 'Start and Stop', 'Signed updates'].map(
      (name, i) => ({
        name,
        available: i < on || i === 4,
      }),
    );
  const features = { full: 4, newer: 2, monitoring: 2, unsupported: 0 };
  let revision = 1;
  const state = {
    version: '0.1.0-alpha.10',
    update: updateFailed
      ? {
          current: '0.1.0-alpha.10',
          latest: release,
          available: true,
          phase: 'failed',
          message: 'The package signature did not match. Nothing was installed.',
          failed: true,
          needsAttention: true,
          checkedAt: Math.floor(Date.now() / 1000) - 120,
          updatedAt: Math.floor(Date.now() / 1000) - 120,
        }
      : {
          current: '0.1.0-alpha.10',
          latest: null,
          available: false,
          phase: 'idle',
          message: '',
          failed: false,
          needsAttention: false,
          checkedAt: 0,
          updatedAt: 0,
        },
    revision: '1',
    scope: 'All clients',
    compat: {
      level: level === 'newer' ? 'monitoring' : level,
      verified: level !== 'newer',
      firmware: level === 'unsupported' ? '4.8.1' : '4.9.2',
      message: compatMessages[level],
      features: level === 'full' ? [] : featureList(features[level]),
    },
    capabilities: {
      configure: level !== 'unsupported',
      select: level === 'full',
      control: level === 'full',
      update: true,
    },
    history: {
      days: 7,
      tiers: [
        { step: 15, keptSeconds: 21600 },
        { step: 60, keptSeconds: 259200 },
        { step: 300, keptSeconds: 604800 },
      ],
    },
    config: {
      enabled: true,
      interval: 5,
      tunnels: ['home', 'backup', 'tokyo'],
      targets: ['1.1.1.1', '9.9.9.9', 'example.com', 'missing.invalid'],
      directWan: true,
      historyDays: 7,
      autoUpdates: false,
      autoExit: true,
      autoTunnels: ['home', 'backup'],
      endpointDiagnostics: true,
    },
    egress: {
      checkedAt: Math.floor(Date.now() / 1000) - 90,
      paths: [
        {
          id: 'home',
          name: names.home,
          interface: 'wg-home',
          ip: '203.0.113.5',
          location: null,
          error: null,
        },
        {
          id: 'backup',
          name: names.backup,
          interface: 'wg-backup',
          ip: '198.51.100.7',
          location: null,
          error: null,
        },
        {
          id: 'tokyo',
          name: names.tokyo,
          interface: 'wg-tokyo',
          ip: null,
          location: null,
          error: 'The IP check timed out after 6 seconds.',
        },
        {
          id: 'wan',
          name: names.wan,
          interface: wanDown ? null : 'eth1',
          ip: wanDown ? null : '203.0.113.80',
          location: null,
          error: wanDown ? 'Direct WAN is unavailable.' : null,
        },
      ],
    },
    auto: {
      enabled: true,
      last: {
        at: Date.now() - 12 * 60000,
        from: 'backup',
        to: 'home',
        reason: 'Lower latency',
      },
    },
    wan: wanDown
      ? {
          available: false,
          interface: null,
          reason: 'The WAN probe cannot bypass the VPN on this firmware.',
        }
      : { available: true, interface: 'eth1', reason: '' },
    uplinks: [
      {
        id: 'wan',
        interface: wanDown ? null : 'eth1',
        device: wanDown ? null : 'eth1',
        address: null,
        active: true,
        state: wanDown ? 'route_unavailable' : 'reachable',
        median: wanMs,
      },
      {
        id: 'uplink:wwan',
        interface: 'wwan',
        device: 'wwan0',
        address: '100.64.12.9',
        active: false,
        state: 'reachable',
        median: 21.84,
      },
      {
        id: 'uplink:secondwan',
        interface: 'secondwan',
        device: 'eth2',
        address: '192.0.2.77',
        active: false,
        state: 'route_mismatch',
        median: null,
      },
    ],
    events: [
      {
        at: Date.now() - 20 * 60000,
        subject: 'home',
        kind: 'server',
        text: 'VPN server address changed to 203.0.113.10',
      },
      {
        at: Date.now() - 3 * 3600000,
        subject: 'wan',
        kind: 'uplink',
        text: 'Active uplink changed from wwan to eth1',
      },
    ],
    endpoints: [
      { tunnel: 'home', name: names.home, address: '203.0.113.10', reason: '' },
      { tunnel: 'backup', name: names.backup, address: '198.51.100.24', reason: '' },
      { tunnel: 'tokyo', name: names.tokyo, address: '192.0.2.44', reason: '' },
      {
        tunnel: 'zurich',
        name: names.zurich,
        address: null,
        reason: 'The server address is not known until the tunnel connects once.',
      },
    ],
    resolved: { 'example.com': '93.184.216.34', 'missing.invalid': null },
    tunnels: [
      {
        id: 'home',
        name: names.home,
        preferred: !directExit,
        enabled: true,
        connected: true,
        controllable: true,
        stateLabel: 'Connected',
        age: '4s',
        median: 32.41,
        lost: 0,
        samples: 64,
        eligible: true,
        selectable: true,
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
        median: 24.18,
        lost: 0,
        samples: 64,
        eligible: true,
        selectable: true,
        recent: Array.from({ length: 64 }, (_, i) => 24 + Math.sin(i * 1.2) * 3),
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
        median: 131.07,
        lost: 4,
        samples: 64,
        eligible: false,
        selectable: true,
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
        selectable: false,
        selectReason: 'The tunnel is stopped.',
        historyAvailable: true,
        recent: [],
      },
    ],
    diagnostics: {
      '1.1.1.1': {
        home: 32.41,
        backup: 24.18,
        tokyo: 131.07,
        zurich: null,
        wan: wanMs,
        'uplink:wwan': 21.84,
        'uplink:secondwan': null,
      },
      '9.9.9.9': {
        home: 36.02,
        backup: 28.55,
        tokyo: 128.3,
        zurich: null,
        wan: wanDown ? null : 15.1,
        'uplink:wwan': 23.9,
        'uplink:secondwan': null,
      },
      'example.com': {
        home: 39.76,
        backup: 31.2,
        tokyo: 124.48,
        zurich: null,
        wan: wanDown ? null : 18.05,
        'uplink:wwan': 26.4,
        'uplink:secondwan': null,
      },
      'missing.invalid': {
        home: null,
        backup: null,
        tokyo: null,
        zurich: null,
        wan: null,
        'uplink:wwan': null,
        'uplink:secondwan': null,
      },
      '203.0.113.10': { wan: 21 },
      '198.51.100.24': { wan: 29 },
      '192.0.2.44': { wan: 118 },
    },
  };
  state.tunnels.forEach((tunnel) => (tunnel.monitorId = tunnel.id));
  const copy = (o) => JSON.parse(JSON.stringify(o));
  const delay = (o) => new Promise((resolve) => setTimeout(() => resolve(copy(o)), 180));
  const measurableUplinks = state.uplinks
    .filter((u) => !u.active && u.state === 'reachable')
    .map((u) => u.id);
  function series(id, target, start, end, step) {
    const targetBias = target === 'example.com' ? 7 : target === '9.9.9.9' ? 4 : 0;
    const base =
      { home: 32, backup: 24, tokyo: 130, zurich: 62, wan: 14, 'uplink:wwan': 22 }[id] || 32;
    const points = [];
    for (let at = start; at <= end; at += step * 1000) {
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
  function serverSeries(id, target, start, end, step) {
    const ratio = { home: 0.72, backup: 0.68, tokyo: 0.76 }[id];
    const through = series(id, target, start, end, step);
    return {
      id: `endpoint:${id}`,
      name: `${names[id]} server`,
      points: through.points.map((p) => {
        const n = Math.round((p.at - anchor) / 60000) + 1440;
        const median =
          ratio && p.median !== null
            ? Math.max(1, Math.round(p.median * ratio + Math.sin(n * 2.1) * 1.5))
            : null;
        return {
          at: p.at,
          median,
          min: median === null ? null : median - 2,
          max: median === null ? null : median + 3,
          lost: 0,
          sent: 4,
        };
      }),
    };
  }
  const nowSeconds = () => Math.floor(Date.now() / 1000);
  const busyPhases = ['checking', 'downloading', 'verifying', 'installing'];
  const later = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const updateFailure = {
    phase: 'failed',
    failed: true,
    message: 'The package signature did not match. Nothing was installed.',
  };
  async function runUpdate(steps) {
    for (const [ms, patch] of steps) {
      await later(ms);
      state.update = { ...state.update, ...patch(), updatedAt: nowSeconds() };
    }
  }
  function startUpdate(phase, steps) {
    if (busyPhases.includes(state.update.phase))
      return Promise.resolve({ started: false, update: copy(state.update) });
    state.update = { ...state.update, phase, message: '', failed: false };
    runUpdate(steps);
    return Promise.resolve({ started: true, update: copy(state.update) });
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
                  uplinks: [],
                  events: [],
                  auto: { enabled: false, last: null },
                }
              : copy(state),
          ),
    set_config: async ({ config, expected_revision }) => {
      if (expected_revision !== state.revision) throw Error('State changed. Refresh first.');
      const known = Object.keys(state.config);
      if (Object.keys(config).some((key) => !known.includes(key)))
        throw Error('The settings contain an unknown field.');
      state.config = copy(config);
      state.revision = String(++revision);
      return delay({ applied: true });
    },
    select_tunnel: async ({ section_id, expected_revision }) => {
      if (expected_revision !== state.revision) throw Error('State changed. Refresh first.');
      const t = state.tunnels.find((t) => t.id === section_id);
      if (!t || !t.selectable) throw Error(t ? t.selectReason : 'Unknown tunnel.');
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
          selectable: true,
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
    get_history: ({ tunnel, target, start, end, step, include_wan, all, endpoints, uplinks }) => {
      const ids = all
        ? state.tunnels.filter((t) => t.connected || t.historyAvailable).map((t) => t.id)
        : [tunnel];
      const withServers = all && endpoints && state.config.endpointDiagnostics;
      const events = state.events.filter((e) => e.at >= start && e.at <= end);
      return Promise.resolve({
        series: [
          ...ids.map((id) => series(id, target, start, end, step)),
          ...(include_wan && !wanDown ? [series('wan', target, start, end, step)] : []),
          ...(uplinks ? measurableUplinks.map((id) => series(id, target, start, end, step)) : []),
          ...(withServers ? ids.map((id) => serverSeries(id, target, start, end, step)) : []),
        ],
        events,
      });
    },
    check_update: () =>
      startUpdate('checking', [
        [
          1200,
          () => ({
            phase: 'idle',
            available: true,
            latest: release,
            checkedAt: nowSeconds(),
          }),
        ],
      ]),
    install_update: () =>
      startUpdate('downloading', [
        [1500, () => ({ phase: 'verifying' })],
        [1000, () => (updateFailed ? updateFailure : { phase: 'installing' })],
        ...(updateFailed
          ? []
          : [
              [
                1500,
                () => {
                  state.version = release;
                  return { phase: 'done', current: release, available: false, latest: release };
                },
              ],
            ]),
      ]),
  };
})();
