(() => {
  const anchor = Math.floor(Date.now() / 60000) * 60000;
  const names = {
    home: 'Home · Amsterdam',
    backup: 'Backup · Frankfurt',
    tokyo: 'Tokyo',
    wan: 'Direct WAN',
  };
  let revision = 1;
  const state = {
    version: '0.1.0-alpha.1',
    revision: '1',
    scope: 'All clients',
    capabilities: { configure: true, select: true },
    config: {
      enabled: true,
      target: '1.1.1.1',
      interval: 15,
      tunnels: ['home', 'backup', 'tokyo'],
      diagnostics: false,
      targets: ['1.1.1.1', '8.8.8.8', '9.9.9.9'],
      directWan: true,
      historyHours: 24,
      autoUpdates: false,
    },
    tunnels: [
      {
        id: 'home',
        name: names.home,
        preferred: true,
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
    ],
    diagnostics: {
      '1.1.1.1': { home: 32, backup: 48, tokyo: 131, wan: 14 },
      '8.8.8.8': { home: 39, backup: 55, tokyo: 124, wan: 18 },
      '9.9.9.9': { home: 35, backup: 49, tokyo: 136, wan: 16 },
    },
  };
  const copy = (o) => JSON.parse(JSON.stringify(o));
  const delay = (o) => new Promise((resolve) => setTimeout(() => resolve(copy(o)), 180));
  function series(id, target, start, end, step) {
    const targetBias = target === '8.8.8.8' ? 7 : target === '9.9.9.9' ? 3 : 0;
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
    get_state: () => Promise.resolve(copy(state)),
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
    get_history: ({ tunnel, target, start, end, step, include_wan }) =>
      Promise.resolve({
        series: [
          series(tunnel, target, start, end, step),
          ...(include_wan ? [series('wan', target, start, end, step)] : []),
        ],
      }),
    check_update: () => delay({ message: 'Preview only. No update was downloaded.' }),
    install_update: () => delay({ message: 'Preview only. No update was installed.' }),
  };
})();
