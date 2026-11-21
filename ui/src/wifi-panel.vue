<template>
  <section class="vph-panel vph-wifi" aria-labelledby="vph-wifi-title">
    <header class="vph-chart-heading">
      <div>
        <h2 id="vph-wifi-title">Upstream Wi-Fi</h2>
        <p class="vph-muted">{{ subtitle }}</p>
      </div>
      <div
        v-if="wifi.enabled && ranges.length"
        class="vph-ranges"
        role="group"
        aria-label="Wi-Fi history range"
      >
        <button
          v-for="r in ranges"
          :key="r.minutes"
          type="button"
          :class="{ active: range === r.minutes }"
          :aria-pressed="range === r.minutes ? 'true' : 'false'"
          @click="choose(r.minutes)"
        >
          {{ r.label }}
        </button>
      </div>
    </header>
    <div v-if="!wifi.enabled" class="vph-wifi-off">
      <p>
        Record the signal, noise, link rate, retries, and channel load of the Wi-Fi network that
        this router joins as a repeater. History follows the History setting.
      </p>
      <button
        type="button"
        class="vph-button vph-primary"
        :disabled="!canEnable || busy"
        @click="$emit('enable')"
      >
        Monitor upstream Wi-Fi
      </button>
    </div>
    <template v-else>
      <p v-if="wifi.reason" class="vph-muted" role="status">{{ wifi.reason }}</p>
      <dl v-if="link" class="vph-wifi-facts">
        <div>
          <dt>Network</dt>
          <dd>
            <strong>{{ link.ssid || 'Hidden network' }}</strong>
            <small class="vph-muted">{{ link.bssid }} · {{ wifi.interface }}</small>
          </dd>
        </div>
        <div>
          <dt>Signal</dt>
          <dd>
            <strong class="vph-number">{{ dbm(link.signal) }}</strong>
            <span class="vph-badge" :class="'vph-wifi-' + quality.tone">{{ quality.label }}</span>
            <small class="vph-muted">
              Noise {{ dbm(link.noise)
              }}<template v-if="snr !== null"> · SNR {{ snr }} dB</template>
            </small>
          </dd>
        </div>
        <div>
          <dt>Channel</dt>
          <dd>
            <strong>{{ channel }}</strong>
            <small class="vph-muted"
              >{{ link.hwmode ? '802.11' + link.hwmode : '' }} {{ link.encryption || '' }}</small
            >
          </dd>
        </div>
        <div>
          <dt>Transmit rate</dt>
          <dd>
            <strong class="vph-number">{{ mbps(link.tx.kbps) }}</strong>
            <small class="vph-muted">{{ rateDetail(link.tx) }}</small>
          </dd>
        </div>
        <div>
          <dt>Receive rate</dt>
          <dd>
            <strong class="vph-number">{{ mbps(link.rx.kbps) }}</strong>
            <small class="vph-muted">{{ rateDetail(link.rx) }}</small>
          </dd>
        </div>
        <div>
          <dt>Link</dt>
          <dd>
            <strong>{{ duration(link.connected_seconds) }}</strong>
            <small class="vph-muted"
              >{{ busyText
              }}<template v-if="link.expected_kbps">
                · expected {{ mbps(link.expected_kbps) }}</template
              ></small
            >
          </dd>
        </div>
        <div>
          <dt>Latency over this link</dt>
          <dd>
            <strong class="vph-number">{{ latencyText }}</strong>
            <small class="vph-muted">{{ latencyNote }}</small>
          </dd>
        </div>
      </dl>
      <dl v-if="summary && summary.samples" class="vph-wifi-summary" aria-label="Selected range">
        <div v-for="s in summaryItems" :key="s.label">
          <dt>{{ s.label }}</dt>
          <dd class="vph-number">{{ s.value }}</dd>
        </div>
      </dl>
      <p v-if="error" class="vph-muted" role="status">{{ error }}</p>
      <div v-else-if="loaded && !hasData" class="vph-muted">
        No Wi-Fi history in this range yet. The monitor saves 15-second summaries.
      </div>
      <div ref="charts" class="vph-wifi-charts" :hidden="!hasData">
        <div v-for="c in charts" :key="c.key" class="vph-wifi-chart">
          <h3>{{ c.title }}</h3>
          <div :ref="c.key" role="img" :aria-label="c.title + ' over the selected range'"></div>
          <p v-if="c.key === 'health' && marked" class="vph-muted vph-wifi-key">
            Marks along the top: red for a drop, brand color for a roam to another access point.
          </p>
        </div>
      </div>
    </template>
  </section>
</template>

<script>
const isNumber = (value) => typeof value === 'number' && Number.isFinite(value);
const choices = [
  { label: '15m', minutes: 15 },
  { label: '1h', minutes: 60 },
  { label: '6h', minutes: 360 },
  { label: '24h', minutes: 1440 },
  { label: '7d', minutes: 10080 },
  { label: '30d', minutes: 43200 },
];
const charts = [
  { key: 'signal', title: 'Signal and noise (dBm)', unit: ' dBm' },
  { key: 'rate', title: 'Link rate (Mbit/s)', unit: ' Mbit/s' },
  { key: 'health', title: 'Retries and channel busy (%)', unit: '%' },
];
const refreshMs = 30000;

export default {
  name: 'WifiPanel',
  props: {
    wifi: { type: Object, required: true },
    uplinks: { type: Array, default: () => [] },
    tiers: { type: Array, default: () => [] },
    call: { type: Function, required: true },
    canEnable: Boolean,
    busy: Boolean,
  },
  data() {
    return {
      range: 60,
      points: [],
      events: [],
      summary: null,
      error: '',
      loaded: false,
      request: 0,
      charts,
    };
  },
  computed: {
    link() {
      return this.wifi.link || null;
    },
    subtitle() {
      if (!this.wifi.enabled) return 'Monitoring is off.';
      const age = this.wifi.sampleAgeMs;
      const sampled = isNumber(age) ? ' Last sample ' + Math.round(age / 1000) + ' s ago.' : '';
      return 'Sampled every ' + (this.wifi.interval || 5) + ' seconds.' + sampled;
    },
    ranges() {
      return choices.filter((r) => this.stepFor(r.minutes));
    },
    snr() {
      const l = this.link;
      return l && isNumber(l.signal) && isNumber(l.noise) ? l.signal - l.noise : null;
    },
    quality() {
      const s = this.link && this.link.signal;
      if (!isNumber(s)) return { label: 'Unknown', tone: 'unknown' };
      if (s >= -55) return { label: 'Excellent', tone: 'good' };
      if (s >= -67) return { label: 'Good', tone: 'good' };
      if (s >= -75) return { label: 'Fair', tone: 'fair' };
      return { label: 'Weak', tone: 'weak' };
    },
    channel() {
      const l = this.link;
      const parts = [];
      if (l.channel) parts.push('Channel ' + l.channel);
      if (l.frequency) parts.push(this.band(l.frequency));
      if (l.width) parts.push(l.width + ' MHz');
      return parts.join(' · ') || 'Unknown';
    },
    busyText() {
      const p = this.link.busy_permille;
      return isNumber(p) ? 'Channel ' + (p / 10).toFixed(0) + '% busy' : 'Channel load unknown';
    },
    uplink() {
      return this.uplinks.find((u) => u.id === this.wifi.uplink) || null;
    },
    latencyText() {
      return this.uplink && isNumber(this.uplink.median)
        ? this.uplink.median.toFixed(2) + ' ms'
        : '—';
    },
    latencyNote() {
      if (!this.wifi.uplink) return 'The repeater is not an IPv4 uplink right now.';
      if (!this.uplink || !isNumber(this.uplink.median))
        return 'Turn on Direct internet in Settings to measure it.';
      return (this.uplink.active ? 'Active uplink ' : 'Standby uplink ') + this.uplink.interface;
    },
    hasData() {
      return this.points.some((p) => p.samples > 0);
    },
    marked() {
      return this.points.some((p) => p.disconnects || p.roams);
    },
    summaryItems() {
      const s = this.summary;
      const pct = (v) => (isNumber(v) ? (v < 10 ? v.toFixed(1) : Math.round(v)) + '%' : '—');
      return [
        { label: 'Connected', value: pct(s.uptime) },
        { label: 'Average signal', value: this.dbm(s.signal) },
        { label: 'Weakest signal', value: this.dbm(s.signalMin) },
        { label: 'Average SNR', value: isNumber(s.snr) ? s.snr.toFixed(0) + ' dB' : '—' },
        { label: 'Average transmit rate', value: this.mbps(s.txRate, 1000) },
        { label: 'Retries', value: pct(s.retries) },
        { label: 'Channel busy', value: pct(s.busy) },
        { label: 'Drops', value: String(s.disconnects) },
        { label: 'Roams', value: String(s.roams) },
        {
          label: 'Data down / up',
          value: this.bytes(s.rxBytes) + ' / ' + this.bytes(s.txBytes),
        },
      ];
    },
  },
  watch: {
    'wifi.enabled'(value) {
      if (value) {
        this.load();
        return;
      }
      clearTimeout(this.timer);
      this.request++;
      this.destroy();
      this.points = [];
      this.summary = null;
      this.loaded = false;
    },
    tiers() {
      if (!this.stepFor(this.range) && this.ranges.length) this.choose(this.ranges[0].minutes);
    },
  },
  created() {
    this.plots = {};
    this.timer = null;
  },
  mounted() {
    this.resize =
      typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(() => this.fit());
    document.addEventListener('visibilitychange', this.visible);
    if (this.wifi.enabled) this.load();
  },
  beforeDestroy() {
    this.request++;
    clearTimeout(this.timer);
    document.removeEventListener('visibilitychange', this.visible);
    if (this.resize) this.resize.disconnect();
    this.destroy();
  },
  methods: {
    stepFor(minutes) {
      const need = minutes * 60;
      const tier = this.tiers
        .filter((t) => t.keptSeconds >= need && need / t.step <= 2400)
        .sort((a, b) => a.step - b.step)[0];
      return tier ? tier.step : 0;
    },
    visible() {
      clearTimeout(this.timer);
      if (!document.hidden) this.load();
    },
    choose(minutes) {
      this.range = minutes;
      this.load();
    },
    async load() {
      clearTimeout(this.timer);
      const step = this.stepFor(this.range);
      if (!this.wifi.enabled || !step) return;
      const request = ++this.request;
      const end = Date.now();
      try {
        const r = await this.call('get_wifi_history', {
          start: end - this.range * 60000,
          end,
          step,
        });
        if (request !== this.request) return;
        this.points = Object.freeze(Array.isArray(r.points) ? r.points : []);
        this.events = Array.isArray(r.events) ? r.events : [];
        this.summary = r.summary || null;
        this.error = '';
        this.loaded = true;
        this.$nextTick(this.draw);
      } catch (e) {
        if (request !== this.request) return;
        this.error = 'Wi-Fi history is unavailable. ' + ((e && e.message) || '');
      } finally {
        if (request === this.request && !document.hidden)
          this.timer = setTimeout(this.load, refreshMs);
      }
    },
    dbm(value) {
      return isNumber(value) ? Math.round(value) + ' dBm' : '—';
    },
    mbps(value, scale) {
      if (!isNumber(value)) return '—';
      const m = scale ? value : value / 1000;
      return (m >= 100 ? Math.round(m) : m.toFixed(1)) + ' Mbit/s';
    },
    band(frequency) {
      if (frequency < 3000) return '2.4 GHz';
      return frequency < 5925 ? '5 GHz' : '6 GHz';
    },
    rateDetail(rate) {
      if (!rate || !rate.mode) return '';
      const parts = [rate.mode];
      if (isNumber(rate.mcs)) parts[0] += ' MCS ' + rate.mcs;
      if (isNumber(rate.nss)) parts.push(rate.nss + (rate.nss === 1 ? ' stream' : ' streams'));
      if (isNumber(rate.mhz)) parts.push(rate.mhz + ' MHz');
      if (rate.short_gi) parts.push('short GI');
      return parts.join(' · ');
    },
    duration(seconds) {
      if (!isNumber(seconds)) return 'Connected';
      const d = Math.floor(seconds / 86400);
      const h = Math.floor((seconds % 86400) / 3600);
      const m = Math.floor((seconds % 3600) / 60);
      if (d) return 'Up ' + d + ' d ' + h + ' h';
      if (h) return 'Up ' + h + ' h ' + m + ' min';
      return 'Up ' + m + ' min';
    },
    bytes(n) {
      if (!isNumber(n)) return '—';
      const units = ['B', 'kB', 'MB', 'GB', 'TB'];
      let i = 0;
      let v = n;
      while (v >= 1000 && i < units.length - 1) {
        v /= 1000;
        i++;
      }
      return (i && v < 10 ? v.toFixed(1) : Math.round(v)) + ' ' + units[i];
    },
    colors() {
      const style = window.getComputedStyle(this.$el);
      const read = (name) => style.getPropertyValue('--vph-' + name).trim();
      return {
        text: read('muted'),
        grid: read('border'),
        a: read('series-2'),
        b: read('series-1'),
        c: read('series-4'),
        e: read('series-3'),
        brand: read('brand'),
        danger: read('danger'),
      };
    },
    column(key) {
      return this.points.map((p) => (isNumber(p[key]) ? p[key] : null));
    },
    build(key, color) {
      const line = (label, stroke, extra) =>
        Object.assign({ label, stroke, width: 1.5, spanGaps: false, points: { size: 3 } }, extra);
      if (key === 'signal')
        return {
          series: [
            line('Average', color.a, { width: 2 }),
            line('Weakest', color.a, { width: 1, dash: [2, 3] }),
            line('Strongest', color.a, { width: 1, dash: [2, 3] }),
            line('Noise', color.b, { dash: [6, 4] }),
          ],
          data: [
            this.column('signal'),
            this.column('signalMin'),
            this.column('signalMax'),
            this.column('noise'),
          ],
          range: (u, low, high) => [
            Math.floor((isNumber(low) ? low : -100) - 5),
            Math.min(0, Math.ceil((isNumber(high) ? high : -30) + 5)),
          ],
        };
      if (key === 'rate')
        return {
          series: [
            line('Transmit', color.a, { width: 2 }),
            line('Receive', color.b),
            line('Expected', color.c, { dash: [6, 4] }),
          ],
          data: [this.column('txRate'), this.column('rxRate'), this.column('expected')],
          range: (u, low, high) => [0, high > 0 ? high * 1.15 : 100],
        };
      return {
        series: [line('Retries', color.e, { width: 2 }), line('Channel busy', color.a)],
        data: [this.column('retries'), this.column('busy')],
        range: (u, low, high) => [0, Math.min(100, Math.max(10, high * 1.2))],
      };
    },
    markers(u) {
      const ctx = u.ctx;
      const top = u.bbox.top;
      const color = this.colors();
      this.points.forEach((p) => {
        if (!p.disconnects && !p.roams) return;
        const x = Math.round(u.valToPos(p.at / 1000, 'x', true));
        ctx.save();
        ctx.fillStyle = p.disconnects ? color.danger : color.brand;
        ctx.fillRect(x - 1, top, 3, 8 * window.devicePixelRatio);
        ctx.restore();
      });
    },
    draw() {
      if (!this.hasData || !this.$refs.charts) return;
      const UPlot = require('uplot').default;
      const color = this.colors();
      const width = Math.max(280, this.$refs.charts.clientWidth);
      const x = this.points.map((p) => p.at / 1000);
      const end = Date.now() / 1000;
      const axis = (extra) =>
        Object.assign(
          {
            stroke: color.text,
            grid: { stroke: color.grid, width: 1 },
            ticks: { stroke: color.grid, width: 1 },
          },
          extra,
        );
      charts.forEach((c) => {
        const target = (this.$refs[c.key] || [])[0];
        if (!target) return;
        const built = this.build(c.key, color);
        const data = [x, ...built.data];
        const plot = this.plots[c.key];
        if (plot && plot.series.length === built.series.length + 1) {
          plot.setData(data);
          return;
        }
        if (plot) plot.destroy();
        this.plots[c.key] = new UPlot(
          {
            width,
            height: 150,
            cursor: { sync: { key: 'wayline-wifi' }, drag: { x: false, y: false } },
            legend: { live: true },
            scales: {
              x: { time: true, range: () => [end - this.range * 60, end] },
              y: { range: built.range },
            },
            axes: [axis({ grid: { show: false } }), axis({ size: 48 })],
            series: [
              {},
              ...built.series.map((s) =>
                Object.assign(s, {
                  value: (u, v) => (isNumber(v) ? +v.toFixed(1) + c.unit : '—'),
                }),
              ),
            ],
            hooks: c.key === 'health' ? { draw: [(u) => this.markers(u)] } : {},
          },
          data,
          target,
        );
      });
      if (this.resize) this.resize.observe(this.$refs.charts);
    },
    fit() {
      if (!this.$refs.charts) return;
      const width = Math.max(280, this.$refs.charts.clientWidth);
      Object.values(this.plots).forEach((p) => p.setSize({ width, height: 150 }));
    },
    destroy() {
      Object.values(this.plots).forEach((p) => p.destroy());
      this.plots = {};
    },
  },
};
</script>

<style>
.vph-wifi-off {
  display: grid;
  gap: 10px;
  justify-items: start;
}
.vph-wifi-facts,
.vph-wifi-summary {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
  gap: 12px 18px;
  margin: 12px 0;
}
.vph-wifi-facts dt,
.vph-wifi-summary dt {
  color: var(--vph-muted);
  font-size: 12px;
}
.vph-wifi-facts dd,
.vph-wifi-summary dd {
  margin: 2px 0 0;
}
.vph-wifi-facts small {
  display: block;
  overflow-wrap: anywhere;
}
.vph-wifi-summary {
  grid-template-columns: repeat(auto-fill, minmax(130px, 1fr));
  padding: 10px 0;
  border-block: 1px solid var(--vph-border);
}
.vph-wifi .vph-badge {
  margin-left: 6px;
}
.vph-wifi-weak {
  color: var(--vph-danger-text);
}
.vph-wifi-fair {
  color: var(--vph-warn-text);
}
.vph-wifi-chart h3 {
  margin: 14px 0 4px;
  font-size: 13px;
  font-weight: 600;
}
.vph-wifi-chart .u-legend {
  display: flex;
  flex-wrap: wrap;
  gap: 2px 14px;
  margin: 4px 0 0;
  font-size: 12px;
  text-align: left;
}
.vph-wifi-chart .u-legend tr {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}
.vph-wifi-chart .u-legend tr:first-child {
  display: none;
}
.vph-wifi-chart .u-legend th,
.vph-wifi-chart .u-legend td {
  display: inline;
  height: auto;
  min-width: 6ch;
  padding: 0;
  border: 0;
  font-weight: 400;
  background: none;
}
.vph-wifi-key {
  margin: 4px 0 0;
  font-size: 12px;
}
</style>
