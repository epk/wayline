<template>
  <main class="vph">
    <div v-if="preview" class="vph-preview">
      Interactive preview · sample data · no router changes
    </div>
    <header class="vph-heading">
      <div>
        <div class="vph-eyebrow">VPN</div>
        <h1>Wayline</h1>
        <p>Compare your tunnels and choose an exit.</p>
      </div>
      <button
        class="vph-button"
        :disabled="!state.capabilities.configure"
        :aria-expanded="settingsOpen"
        @click="openSettings"
      >
        Settings
      </button>
    </header>
    <div v-if="error || readError || historyError" class="vph-error" role="alert">
      {{ error || readError || historyError }}
    </div>
    <div v-if="notice" class="vph-notice" role="status">{{ notice }}</div>

    <form v-if="settingsOpen" class="vph-panel vph-settings" @submit.prevent="saveSettings">
      <h2>Settings</h2>
      <label class="vph-check"
        ><input type="checkbox" v-model="draft.enabled" /> Measure tunnel latency</label
      >
      <div class="vph-fields">
        <label
          >Comparison target<input
            v-model.trim="draft.target"
            required
            placeholder="1.1.1.1"
            inputmode="decimal"
          /><small>The monitor uses the same target for every tunnel.</small></label
        >
        <label
          >Probe interval<select v-model.number="draft.interval">
            <option :value="15">15 seconds</option>
            <option :value="30">30 seconds</option>
            <option :value="60">60 seconds</option>
          </select></label
        >
      </div>
      <fieldset>
        <legend>Tunnels to monitor</legend>
        <label v-for="t in state.tunnels" :key="t.id" class="vph-check"
          ><input type="checkbox" :value="t.monitorId" v-model="draft.tunnels" />{{ t.name }}</label
        >
      </fieldset>
      <hr />
      <label class="vph-check"
        ><input type="checkbox" v-model="draft.diagnostics" /> Internet diagnostics
        <span class="vph-muted">Optional</span></label
      >
      <p class="vph-muted">
        Compare several destinations through your tunnels and directly through WAN.
      </p>
      <div v-if="draft.diagnostics">
        <div class="vph-target" v-for="(_, i) in draft.targets" :key="i">
          <label :for="'vph-target-' + i">Target {{ i + 1 }}</label
          ><input
            :id="'vph-target-' + i"
            v-model.trim="draft.targets[i]"
            required
            inputmode="decimal"
          /><button
            type="button"
            class="vph-link"
            :disabled="draft.targets.length === 1"
            @click="draft.targets.splice(i, 1)"
          >
            Remove
          </button>
        </div>
        <button
          type="button"
          class="vph-link"
          :disabled="draft.targets.length === 4"
          @click="draft.targets.push('')"
        >
          Add target
        </button>
        <label class="vph-check"
          ><input type="checkbox" v-model="draft.directWan" /> Include direct WAN
          measurements</label
        >
        <small
          >Direct WAN probes bypass the VPN. If the WAN probe cannot bypass the VPN, the monitor
          shows Unavailable.</small
        >
      </div>
      <hr />
      <div class="vph-fields">
        <label
          >History retention<select v-model.number="draft.historyHours">
            <option :value="24">24 hours</option>
            <option :value="0">Memory only</option></select
          ><small>The monitor saves history in batches to limit flash writes.</small></label
        >
        <div>
          <label class="vph-check"
            ><input type="checkbox" v-model="draft.autoUpdates" /> Install stable updates
            automatically</label
          ><small>The updater installs only signed releases that support this firmware.</small
          ><button type="button" class="vph-link" :disabled="busy" @click="checkUpdates">
            Check updates
          </button>
          <p v-if="state.update" class="vph-muted">{{ state.update.message }}</p>
          <button
            v-if="state.update && state.update.available"
            type="button"
            class="vph-button"
            :disabled="busy"
            @click="installUpdate"
          >
            Install update
          </button>
        </div>
      </div>
      <footer>
        <button type="button" class="vph-button" @click="settingsOpen = false">Cancel</button
        ><button class="vph-button vph-primary" :disabled="busy">
          {{ busy ? 'Saving…' : 'Save settings' }}
        </button>
      </footer>
    </form>

    <nav class="vph-tabs" aria-label="Wayline views">
      <button :class="{ active: tab === 'tunnels' }" @click="tab = 'tunnels'">Exit tunnels</button
      ><button :class="{ active: tab === 'diagnostics' }" @click="tab = 'diagnostics'">
        Internet diagnostics
      </button>
    </nav>
    <template v-if="tab === 'tunnels'">
      <div class="vph-scope">
        <span
          >Exit preference for <strong>{{ state.scope || 'Scope unavailable' }}</strong></span
        ><span class="vph-muted"
          >Target {{ state.config.target }} · every {{ state.config.interval }}s</span
        >
      </div>
      <div class="vph-panel vph-table-wrap">
        <table>
          <thead>
            <tr>
              <th>Tunnel</th>
              <th>Latency</th>
              <th>Loss</th>
              <th class="vph-spark-column">Recent latency</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            <tr
              v-for="t in state.tunnels"
              :key="t.id"
              :class="{ selected: selectedTunnel === t.id }"
            >
              <td>
                <button
                  class="vph-name"
                  @click="
                    selectedTunnel = t.id;
                    loadHistory();
                  "
                >
                  {{ t.name }}</button
                ><span v-if="t.preferred" class="vph-badge">Preferred</span>
                <div class="vph-muted vph-status">
                  {{ t.stateLabel }}<span v-if="t.age !== 'unknown'"> · {{ t.age }} ago</span>
                </div>
              </td>
              <td class="vph-number">
                {{ latency(t.median) }}<small v-if="t.median !== null"> ms</small>
              </td>
              <td class="vph-number">{{ t.lost }} / {{ t.samples }}</td>
              <td class="vph-spark-column">
                <svg
                  class="vph-spark"
                  viewBox="0 0 150 32"
                  role="img"
                  :aria-label="'Recent latency for ' + t.name"
                >
                  <path :d="spark(t.recent)" />
                </svg>
              </td>
              <td class="vph-action">
                <span v-if="t.preferred" class="vph-muted">Highest priority</span
                ><button
                  v-else
                  class="vph-button"
                  :disabled="busy || !t.eligible || !state.capabilities.select"
                  :title="t.reason || ''"
                  @click="selectExit(t)"
                >
                  {{ busyTunnel === t.id ? 'Applying…' : 'Use exit' }}
                </button>
                <button
                  class="vph-button"
                  :disabled="busy || !t.controllable || !state.capabilities.control"
                  :aria-label="(t.enabled ? 'Stop ' : 'Start ') + t.name"
                  @click="setTunnelEnabled(t)"
                >
                  {{ t.enabled ? 'Stop' : 'Start' }}
                </button>
                <small v-if="!t.eligible && t.connected">{{ t.reason }}</small>
                <small v-if="!t.controllable">Native tunnel control is unavailable.</small>
              </td>
            </tr>
          </tbody>
        </table>
        <div v-if="!state.tunnels.length" class="vph-empty">
          {{ loading ? 'Reading tunnel state…' : 'No configured VPN tunnels available.' }}
        </div>
      </div>
      <p class="vph-muted">
        Preference changes apply to this traffic scope. Applications with existing connections may
        need to reconnect.
      </p>
      <p class="vph-muted">
        Start and Stop use GL.iNet's VPN manager. Starting preserves priority. Stopping can permit
        WAN traffic under your GL.iNet policy.
      </p>
    </template>
    <template v-else>
      <section v-if="!state.config.diagnostics" class="vph-panel vph-empty">
        <h2>Internet diagnostics</h2>
        <p>See how destinations perform through each tunnel and directly through WAN.</p>
        <button class="vph-button" :disabled="!state.capabilities.configure" @click="openSettings">
          Enable in settings
        </button>
      </section>
      <div v-else class="vph-panel vph-table-wrap">
        <table>
          <thead>
            <tr>
              <th>Destination</th>
              <th v-if="state.config.directWan">Direct WAN</th>
              <th v-for="t in state.tunnels" :key="t.id">{{ t.name }}</th>
            </tr>
          </thead>
          <tbody>
            <tr
              v-for="target in state.config.targets"
              :key="target"
              :class="{ selected: selectedTarget === target }"
            >
              <td>
                <button
                  class="vph-name"
                  @click="
                    selectedTarget = target;
                    loadHistory();
                  "
                >
                  {{ target }}
                </button>
              </td>
              <td v-if="state.config.directWan" class="vph-number">
                {{ diagnostic(target, 'wan') }}
              </td>
              <td v-for="t in state.tunnels" :key="t.id" class="vph-number">
                {{ diagnostic(target, t.id) }}
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </template>

    <section v-if="tab === 'tunnels' || state.config.diagnostics" class="vph-panel vph-history">
      <header class="vph-chart-heading">
        <div>
          <h2>{{ selectedName }} <span class="vph-muted">latency</span></h2>
          <p class="vph-muted">
            {{ graphTarget }} ·
            {{ range === 1440 ? '5-minute aggregates' : '1-minute aggregates' }} · gaps indicate
            missing measurements
          </p>
        </div>
        <div class="vph-ranges" aria-label="History range">
          <button
            v-for="r in ranges"
            :key="r.minutes"
            :class="{ active: range === r.minutes }"
            :aria-pressed="range === r.minutes"
            @click="
              range = r.minutes;
              offset = 0;
              loadHistory();
            "
          >
            {{ r.label }}
          </button>
        </div>
      </header>
      <div class="vph-chart" ref="chart" @mouseleave="hover = null">
        <svg
          :viewBox="'0 0 ' + chartWidth + ' 236'"
          role="img"
          :aria-label="'Latency history for ' + selectedName + ' to ' + graphTarget"
          @mousemove="hoverAt"
          @click="hoverAt"
        >
          <title>Latency in milliseconds over time</title>
          <desc>
            Line shows median latency, band shows minimum to maximum. Missing measurements appear as
            gaps.
          </desc>
          <g v-for="tick in [0, 1, 2, 3, 4]" :key="tick">
            <line
              x1="45"
              :x2="chartWidth - 18"
              :y1="chartY((tick * yMax) / 4)"
              :y2="chartY((tick * yMax) / 4)"
              class="vph-grid"
            />
            <text x="35" :y="chartY((tick * yMax) / 4) + 4" text-anchor="end">
              {{ Math.round((tick * yMax) / 4) }}
            </text>
          </g>
          <text x="12" y="14">ms</text>
          <g
            v-for="series in history"
            :key="series.id"
            :class="{ 'vph-wan-series': series.id === 'wan' }"
          >
            <path :d="band(series.points)" class="vph-band" />
            <path :d="line(series.points)" class="vph-line" />
          </g>
          <text
            v-for="fraction in [0, 0.5, 1]"
            :key="fraction"
            :x="45 + fraction * (chartWidth - 63)"
            y="224"
            :text-anchor="fraction === 0 ? 'start' : fraction === 1 ? 'end' : 'middle'"
          >
            {{ clock(windowStart + fraction * range * 60000) }}
          </text>
          <line v-if="hover" :x1="hover.x" :x2="hover.x" y1="24" y2="200" class="vph-guide" />
        </svg>
        <div
          v-if="hover"
          class="vph-tooltip"
          :style="{ left: Math.min(Math.max(hover.x, 0), chartWidth - 190) + 'px' }"
        >
          <strong>{{ clock(hover.at) }}</strong>
          <div v-for="item in hover.values" :key="item.name">
            {{ item.name }}: {{ item.value === null ? 'No measurement' : item.value + ' ms' }}
          </div>
        </div>
      </div>
      <div class="vph-chart-footer">
        <span
          ><span class="vph-swatch"></span>{{ selectedName }}
          <span v-if="history.some((s) => s.id === 'wan')" class="vph-muted">
            · dashed: direct WAN</span
          ></span
        ><span class="vph-muted">{{
          range === 1440
            ? 'Last 24 hours'
            : clock(windowStart) + ' – ' + clock(windowStart + range * 60000)
        }}</span>
      </div>
      <label class="vph-scroll"
        >Scroll through the last 24 hours<input
          type="range"
          :min="0"
          :max="1440 - range"
          :step="5"
          :value="1440 - range - offset"
          :disabled="range === 1440"
          @input="
            offset = 1440 - range - Number($event.target.value);
            loadHistory();
          "
      /></label>
    </section>
    <footer class="vph-footnote">
      {{ preview ? 'Design preview' : 'Wayline' }} · {{ state.version || '0.1.0'
      }}<span v-if="!state.capabilities.select && state.capabilities.reason">
        · {{ state.capabilities.reason }}</span
      >
    </footer>
  </main>
</template>

<script>
const defaults = () => ({
  enabled: false,
  target: '1.1.1.1',
  interval: 15,
  tunnels: [],
  diagnostics: false,
  targets: ['1.1.1.1', '8.8.8.8', '9.9.9.9'],
  directWan: false,
  historyHours: 24,
  autoUpdates: false,
});
const validIp = (value) => {
  const a = value.split('.');
  return (
    a.length === 4 &&
    a.every((v) => /^\d{1,3}$/.test(v) && Number(v) <= 255) &&
    Number(a[0]) > 0 &&
    Number(a[0]) < 224 &&
    Number(a[0]) !== 127 &&
    value !== '255.255.255.255'
  );
};
export default {
  name: 'WaylineDashboard',
  props: { api: Object, preview: Boolean },
  data() {
    return {
      state: {
        config: defaults(),
        tunnels: [],
        capabilities: { configure: false, select: false, control: false },
        scope: '',
        revision: '',
      },
      draft: defaults(),
      tab: 'tunnels',
      selectedTunnel: '',
      selectedTarget: '1.1.1.1',
      settingsOpen: false,
      loading: false,
      readError: '',
      historyError: '',
      busy: false,
      busyTunnel: '',
      error: '',
      notice: '',
      history: [],
      range: 60,
      offset: 0,
      anchor: Math.floor(Date.now() / 60000) * 60000,
      chartWidth: 720,
      hover: null,
      timer: null,
      stopped: false,
      historyRequest: 0,
      ranges: [
        { label: '15m', minutes: 15 },
        { label: '1h', minutes: 60 },
        { label: '6h', minutes: 360 },
        { label: '24h', minutes: 1440 },
      ],
    };
  },
  computed: {
    selectedName() {
      const t = this.state.tunnels.find((t) => t.id === this.selectedTunnel);
      return t ? t.name : 'Tunnel';
    },
    graphTarget() {
      return this.tab === 'diagnostics' ? this.selectedTarget : this.state.config.target;
    },
    windowStart() {
      return this.anchor - (this.offset + this.range) * 60000;
    },
    yMax() {
      let n = 25;
      this.history.forEach((s) =>
        s.points.forEach((p) => {
          if (p.max !== null) n = Math.max(n, p.max);
        }),
      );
      return Math.ceil((n * 1.15) / 25) * 25;
    },
  },
  watch: {
    tab() {
      this.loadHistory();
    },
  },
  mounted() {
    this.refresh();
    document.addEventListener('visibilitychange', this.visibility);
    this.resize =
      typeof ResizeObserver === 'undefined'
        ? null
        : new ResizeObserver((entries) => {
            if (entries[0]) this.chartWidth = Math.max(280, entries[0].contentRect.width);
          });
    this.$nextTick(() => {
      if (this.$refs.chart && this.resize) this.resize.observe(this.$refs.chart);
    });
  },
  beforeDestroy() {
    this.stopped = true;
    clearTimeout(this.timer);
    document.removeEventListener('visibilitychange', this.visibility);
    if (this.resize) this.resize.disconnect();
  },
  methods: {
    call(method, params) {
      const request = this.api
        ? this.api[method](params || {})
        : this.$rpcRequest('call', ['sid', 'wayline', method, params || {}], {
            timeout: method === 'set_tunnel_enabled' ? 45000 : 12000,
          });
      return new Promise((resolve, reject) => {
        const timer = setTimeout(
          () => reject(Error('The request timed out. Refresh its status before another change.')),
          method === 'set_tunnel_enabled' ? 45000 : 12000,
        );
        Promise.resolve(request).then(
          (value) => {
            clearTimeout(timer);
            resolve(value);
          },
          (error) => {
            clearTimeout(timer);
            reject(error);
          },
        );
      });
    },
    async refresh() {
      if (this.loading || this.stopped || document.hidden) return;
      clearTimeout(this.timer);
      this.loading = true;
      try {
        const next = await this.call('get_state');
        if (this.stopped) return;
        next.tunnels = Array.isArray(next.tunnels) ? next.tunnels : [];
        next.config.tunnels = Array.isArray(next.config.tunnels) ? next.config.tunnels : [];
        next.tunnels.forEach((tunnel) => {
          tunnel.recent = Array.isArray(tunnel.recent) ? tunnel.recent : [];
        });
        this.state = next;
        if (this.offset === 0) this.anchor = Math.floor(Date.now() / 60000) * 60000;
        if (!next.tunnels.some((t) => t.id === this.selectedTunnel))
          this.selectedTunnel =
            (next.tunnels.find((t) => t.preferred) || next.tunnels[0] || {}).id || '';
        this.readError = '';
        await this.loadHistory();
      } catch (e) {
        this.readError = 'Wayline is unavailable. ' + (e.message || 'Try again shortly.');
      } finally {
        this.loading = false;
        if (!this.stopped && !document.hidden) this.timer = setTimeout(this.refresh, 5000);
      }
    },
    visibility() {
      clearTimeout(this.timer);
      if (!document.hidden) this.refresh();
    },
    openSettings() {
      this.draft = JSON.parse(JSON.stringify(this.state.config));
      this.settingsOpen = !this.settingsOpen;
    },
    async saveSettings() {
      if (this.busy) return;
      this.error = '';
      const targets = [this.draft.target].concat(this.draft.diagnostics ? this.draft.targets : []);
      if (
        targets.some((t) => !validIp(t)) ||
        new Set(this.draft.targets).size !== this.draft.targets.length
      ) {
        this.error = 'Use unique unicast IPv4 addresses for the targets.';
        return;
      }
      if (this.draft.enabled && !this.draft.tunnels.length) {
        this.error = 'Choose at least one tunnel to monitor.';
        return;
      }
      this.busy = true;
      try {
        await this.call('set_config', {
          config: this.draft,
          expected_revision: this.state.revision,
        });
        this.settingsOpen = false;
        this.notice = 'Settings saved.';
        await this.refresh();
      } catch (e) {
        this.error = e.message || 'Settings could not be saved.';
      } finally {
        this.busy = false;
      }
    },
    async selectExit(t) {
      if (this.busy || t.preferred || !t.eligible || !this.state.capabilities.select) return;
      this.busy = true;
      this.busyTunnel = t.id;
      this.error = '';
      try {
        await this.call('select_tunnel', {
          section_id: t.id,
          expected_revision: this.state.revision,
        });
        this.notice = 'Exit preference updated.';
        await this.refresh();
      } catch (e) {
        this.error =
          'Selection outcome needs checking. ' + (e.message || 'Refresh before trying again.');
        await this.refresh();
      } finally {
        this.busy = false;
        this.busyTunnel = '';
      }
    },
    async setTunnelEnabled(t) {
      if (this.busy || !t.controllable || !this.state.capabilities.control) return;
      this.busy = true;
      this.error = '';
      this.notice = t.enabled ? 'Stopping tunnel…' : 'Starting tunnel…';
      try {
        const result = await this.call('set_tunnel_enabled', {
          section_id: t.id,
          enabled: !t.enabled,
          expected_revision: this.state.revision,
        });
        this.notice = result.enabled
          ? 'Tunnel enabled. Connection and measurements can take time.'
          : 'Tunnel stopped. GL.iNet traffic policy applies.';
        if (result.monitor_pending) this.notice += ' Monitoring will reconcile within one minute.';
      } catch (e) {
        this.error =
          'Tunnel outcome needs checking. ' + (e.message || 'Refresh before trying again.');
      } finally {
        await this.refresh();
        this.busy = false;
      }
    },
    async installUpdate() {
      this.busy = true;
      try {
        const r = await this.call('install_update');
        this.notice = r.message;
      } catch (e) {
        this.error = e.message || 'Update unavailable.';
      } finally {
        this.busy = false;
      }
    },
    async checkUpdates() {
      this.busy = true;
      try {
        const r = await this.call('check_update');
        this.notice = r.message;
      } catch (e) {
        this.error = e.message || 'Update check unavailable.';
      } finally {
        this.busy = false;
      }
    },
    async loadHistory() {
      const request = ++this.historyRequest;
      const selected = this.state.tunnels.find((tunnel) => tunnel.id === this.selectedTunnel);
      if (selected && !selected.connected) {
        this.history = [];
        this.historyError = 'History is unavailable until this tunnel has a connected path.';
        this.hover = null;
        return;
      }
      if (!this.selectedTunnel || (this.tab === 'diagnostics' && !this.state.config.diagnostics))
        return;
      try {
        const r = await this.call('get_history', {
          tunnel: this.selectedTunnel,
          target: this.graphTarget,
          start: this.windowStart,
          end: this.windowStart + this.range * 60000,
          step: this.range === 1440 ? 5 : 1,
          include_wan: this.tab === 'diagnostics' && this.state.config.directWan,
        });
        if (request === this.historyRequest) {
          this.history = Array.isArray(r.series) ? r.series : [];
          this.history.forEach((series) => {
            series.points = Array.isArray(series.points) ? series.points : [];
          });
          this.historyError = '';
          this.hover = null;
          this.$nextTick(() => {
            if (this.$refs.chart && this.resize) this.resize.observe(this.$refs.chart);
          });
        }
      } catch (e) {
        if (request === this.historyRequest) {
          this.history = [];
          this.historyError = e.message || 'History unavailable.';
        }
      }
    },
    latency(n) {
      return n === null ? '—' : Math.round(n);
    },
    clock(t) {
      return new Date(t).toLocaleTimeString([], {
        hour: '2-digit',
        minute: '2-digit',
        hour12: false,
      });
    },
    diagnostic(target, id) {
      const d = (this.state.diagnostics || {})[target] || {};
      return d[id] === undefined || d[id] === null ? 'Unavailable' : Math.round(d[id]) + ' ms';
    },
    chartX(at) {
      return 45 + ((at - this.windowStart) / (this.range * 60000)) * (this.chartWidth - 63);
    },
    chartY(v) {
      return 200 - (v / this.yMax) * 176;
    },
    line(points) {
      let d = '',
        move = true;
      points.forEach((p) => {
        if (p.median === null) {
          move = true;
          return;
        }
        d +=
          (move ? 'M' : 'L') +
          this.chartX(p.at).toFixed(1) +
          ',' +
          this.chartY(p.median).toFixed(1);
        move = false;
      });
      return d;
    },
    band(points) {
      let d = '',
        group = [];
      const flush = () => {
        if (group.length) {
          d +=
            'M' +
            group
              .map((p) => this.chartX(p.at).toFixed(1) + ',' + this.chartY(p.min).toFixed(1))
              .join('L') +
            'L' +
            group
              .slice()
              .reverse()
              .map((p) => this.chartX(p.at).toFixed(1) + ',' + this.chartY(p.max).toFixed(1))
              .join('L') +
            'Z';
          group = [];
        }
      };
      points.forEach((p) => {
        if (p.median === null) flush();
        else group.push(p);
      });
      flush();
      return d;
    },
    spark(values) {
      let d = '',
        move = true;
      const max = Math.max(25, ...values.filter((n) => n !== null)) * 1.1;
      values.forEach((n, i) => {
        if (n === null) {
          move = true;
          return;
        }
        d +=
          (move ? 'M' : 'L') +
          ((i / Math.max(1, values.length - 1)) * 150).toFixed(1) +
          ',' +
          (30 - (n / max) * 28).toFixed(1);
        move = false;
      });
      return d;
    },
    hoverAt(e) {
      const rect = e.currentTarget.getBoundingClientRect();
      const x = Math.max(
        45,
        Math.min(this.chartWidth - 18, ((e.clientX - rect.left) * this.chartWidth) / rect.width),
      );
      const at = this.windowStart + ((x - 45) / (this.chartWidth - 63)) * this.range * 60000;
      this.hover = {
        x,
        at,
        values: this.history.map((s) => {
          let nearest = null;
          for (const p of s.points)
            if (!nearest || Math.abs(p.at - at) < Math.abs(nearest.at - at)) nearest = p;
          return { name: s.name, value: nearest ? nearest.median : null };
        }),
      };
    },
  },
};
</script>

<style>
.vph {
  --vph-bg: var(--background-color, #f4f6f8);
  --vph-card: var(--card-background-color, #fff);
  --vph-text: var(--text-color, #253644);
  --vph-muted: var(--label-color, #667984);
  --vph-border: var(--table-border, #e3e9ed);
  --vph-brand: var(--primary-color, #159b9b);
  color: var(--vph-text);
  font:
    14px/1.5 -apple-system,
    BlinkMacSystemFont,
    'Segoe UI',
    sans-serif;
  width: 100%;
  box-sizing: border-box;
  min-width: 0;
  max-width: 1120px;
  margin: auto;
}
.vph * {
  box-sizing: border-box;
}
.vph h1 {
  font-size: 26px;
  font-weight: 600;
  line-height: 1.25;
  margin: 3px 0 7px;
}
.vph h2 {
  font-size: 16px;
  font-weight: 600;
  margin: 0;
}
.vph p {
  margin: 6px 0 16px;
}
.vph button,
.vph input,
.vph select {
  font: inherit;
}
.vph button {
  cursor: pointer;
}
.vph button:disabled {
  cursor: default;
  opacity: 0.45;
}
.vph button:focus-visible,
.vph input:focus-visible,
.vph select:focus-visible {
  outline: 2px solid var(--vph-brand);
  outline-offset: 3px;
}
.vph-heading,
.vph-chart-heading,
.vph-scope,
.vph-chart-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 20px;
  flex-wrap: wrap;
}
.vph-heading {
  margin: 22px 0 28px;
}
.vph-heading p,
.vph-eyebrow,
.vph-muted,
.vph small {
  color: var(--vph-muted);
}
.vph-eyebrow {
  font-size: 12px;
  letter-spacing: 0.08em;
}
.vph-preview {
  background: #fff4d3;
  color: #73551a;
  text-align: center;
  padding: 8px;
  border-radius: 5px;
}
.vph-button {
  border: 1px solid var(--vph-border);
  border-radius: 5px;
  background: var(--vph-card);
  color: var(--vph-text);
  padding: 8px 15px;
  white-space: nowrap;
}
.vph-primary {
  background: var(--vph-brand);
  border-color: var(--vph-brand);
  color: #fff;
}
.vph-link,
.vph-name {
  border: 0;
  background: none;
  padding: 0;
  color: var(--vph-brand);
  text-align: left;
}
.vph-name {
  color: var(--vph-text);
  font-weight: 600;
}
.vph-link {
  padding: 6px 0;
}
.vph-panel {
  background: var(--vph-card);
  border: 1px solid var(--vph-border);
  border-radius: 7px;
  margin: 16px 0;
}
.vph-tabs {
  display: flex;
  gap: 26px;
  border-bottom: 1px solid var(--vph-border);
  margin: 22px 0;
}
.vph-tabs button {
  background: none;
  border: 0;
  border-bottom: 2px solid transparent;
  padding: 10px 0;
  color: var(--vph-muted);
}
.vph-tabs button.active {
  border-color: var(--vph-brand);
  color: var(--vph-brand);
}
.vph-scope {
  margin: 18px 0 12px;
}
.vph-table-wrap {
  overflow-x: auto;
}
.vph table {
  width: 100%;
  border-collapse: collapse;
  text-align: left;
}
.vph th {
  font-size: 12px;
  font-weight: 500;
  color: var(--vph-muted);
  padding: 13px 18px;
  border-bottom: 1px solid var(--vph-border);
}
.vph td {
  padding: 17px 18px;
  border-bottom: 1px solid var(--vph-border);
  vertical-align: middle;
}
.vph tr:last-child td {
  border-bottom: 0;
}
.vph tr.selected {
  background: color-mix(in srgb, var(--vph-brand) 3%, var(--vph-card));
}
.vph-number {
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}
.vph-number small {
  font-size: 12px;
}
.vph-badge {
  display: inline-block;
  border-radius: 4px;
  font-size: 11px;
  color: var(--vph-brand);
  background: color-mix(in srgb, var(--vph-brand) 9%, var(--vph-card));
  padding: 2px 7px;
  margin-left: 9px;
}
.vph-status {
  font-size: 12px;
  margin-top: 5px;
}
.vph-action {
  text-align: right;
  min-width: 195px;
}
.vph-action > button + button,
.vph-action > span + button {
  margin-left: 8px;
}
.vph-action small {
  display: block;
  max-width: 175px;
  margin: 4px 0 0 auto;
  font-size: 11px;
}
.vph-spark {
  width: 150px;
  height: 32px;
  display: block;
}
.vph-spark path {
  fill: none;
  stroke: var(--vph-brand);
  stroke-width: 1.5;
}
.vph-history {
  padding: 22px;
}
.vph-chart-heading {
  align-items: flex-start;
}
.vph-chart-heading p {
  font-size: 12px;
}
.vph-ranges {
  display: flex;
  gap: 3px;
}
.vph-ranges button {
  border: 0;
  border-radius: 4px;
  background: none;
  padding: 6px 10px;
  color: var(--vph-muted);
}
.vph-ranges button.active {
  background: color-mix(in srgb, var(--vph-brand) 10%, var(--vph-card));
  color: var(--vph-brand);
}
.vph-chart {
  position: relative;
}
.vph-chart svg {
  width: 100%;
  display: block;
  overflow: visible;
}
.vph-chart svg text {
  font:
    12px -apple-system,
    BlinkMacSystemFont,
    'Segoe UI',
    sans-serif;
  fill: var(--vph-muted);
}
.vph-grid {
  stroke: var(--vph-border);
  stroke-width: 1;
}
.vph-line {
  stroke: var(--vph-brand);
  stroke-width: 1.8;
  fill: none;
  stroke-linejoin: round;
}
.vph-band {
  fill: var(--vph-brand);
  opacity: 0.09;
}
.vph-wan-series .vph-line {
  stroke: var(--vph-muted);
  stroke-dasharray: 5 4;
}
.vph-wan-series .vph-band {
  fill: var(--vph-muted);
  opacity: 0.05;
}
.vph-guide {
  stroke: var(--vph-muted);
  stroke-width: 1;
  stroke-dasharray: 3 3;
}
.vph-tooltip {
  position: absolute;
  top: 25px;
  width: 180px;
  padding: 10px 12px;
  background: var(--vph-card);
  border: 1px solid var(--vph-border);
  border-radius: 5px;
  font-size: 12px;
  pointer-events: none;
}
.vph-chart-footer {
  font-size: 12px;
  margin-top: 5px;
}
.vph-swatch {
  display: inline-block;
  width: 15px;
  height: 2px;
  vertical-align: middle;
  background: var(--vph-brand);
  margin-right: 6px;
}
.vph-scroll {
  display: block;
  color: var(--vph-muted);
  font-size: 12px;
  margin-top: 20px;
}
.vph-scroll input {
  width: 100%;
  margin: 8px 0 0;
  accent-color: var(--vph-brand);
}
.vph-settings {
  padding: 24px;
}
.vph-settings h2 {
  margin-bottom: 18px;
}
.vph-fields {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 24px;
  margin: 16px 0;
}
.vph-fields label {
  display: block;
}
.vph input:not([type='checkbox']):not([type='range']),
.vph select {
  display: block;
  width: 100%;
  border: 1px solid var(--vph-border);
  border-radius: 4px;
  padding: 9px 11px;
  margin-top: 7px;
  background: var(--vph-card);
  color: var(--vph-text);
}
.vph small {
  display: block;
  font-size: 12px;
  margin-top: 5px;
}
.vph-check {
  display: flex !important;
  align-items: center;
  gap: 9px;
  margin: 13px 0;
}
.vph-check input {
  accent-color: var(--vph-brand);
  width: 16px;
  height: 16px;
  margin: 0;
}
.vph fieldset {
  border: 0;
  padding: 0;
  margin: 16px 0;
}
.vph legend {
  font-size: 12px;
  color: var(--vph-muted);
  margin-bottom: 3px;
}
.vph fieldset .vph-check {
  display: inline-flex !important;
  margin-right: 24px;
}
.vph hr {
  border: 0;
  border-top: 1px solid var(--vph-border);
  margin: 22px 0;
}
.vph-target {
  display: grid;
  grid-template-columns: 70px 1fr 70px;
  gap: 15px;
  align-items: center;
  max-width: 500px;
  margin: 9px 0;
}
.vph-target input {
  margin-top: 0 !important;
}
.vph-settings footer {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
  margin-top: 22px;
}
.vph-empty {
  text-align: center;
  padding: 40px 24px;
}
.vph-empty p {
  max-width: 440px;
  margin: 10px auto 22px;
  color: var(--vph-muted);
}
.vph-error {
  padding: 12px;
  color: #9f2836;
  background: #fff0f1;
  border-radius: 4px;
  margin: 12px 0;
}
.vph-notice {
  padding: 12px;
  color: var(--vph-brand);
  background: color-mix(in srgb, var(--vph-brand) 8%, var(--vph-card));
  border-radius: 4px;
  margin: 12px 0;
}
.vph-footnote {
  font-size: 12px;
  color: var(--vph-muted);
  padding: 12px 0 25px;
}
.vph-button:hover:enabled {
  border-color: var(--vph-brand);
}
@media (max-width: 700px) {
  .vph-heading {
    margin-bottom: 18px;
  }
  .vph h1 {
    font-size: 23px;
  }
  .vph-spark-column {
    display: none;
  }
  .vph td,
  .vph th {
    padding: 14px 11px;
  }
  .vph-badge {
    display: block;
    width: max-content;
    margin: 4px 0 0;
  }
  .vph-fields {
    grid-template-columns: 1fr;
    gap: 14px;
  }
  .vph-history,
  .vph-settings {
    padding: 17px 12px;
  }
  .vph-target {
    grid-template-columns: 58px 1fr 62px;
    gap: 8px;
  }
  .vph-chart-heading {
    gap: 4px;
  }
  .vph input,
  .vph select {
    font-size: 16px !important;
  }
  .vph-scope {
    gap: 6px;
  }
  .vph-preview {
    font-size: 12px;
  }
  .vph-tabs {
    gap: 22px;
  }
}
</style>
