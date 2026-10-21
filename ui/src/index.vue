<template>
  <main class="vph">
    <div v-if="preview" class="vph-preview">
      Interactive preview · sample data · no router changes
    </div>
    <section
      v-if="level === 'unsupported'"
      class="vph-panel vph-compat vph-compat-unsupported"
      role="alert"
    >
      <h2>This firmware is not supported.</h2>
      <p>
        <strong>Firmware {{ compat.firmware || 'unknown' }}</strong>
      </p>
      <p>{{ compat.message }}</p>
      <ul v-if="compat.features.length" class="vph-features" aria-label="Features">
        <li v-for="f in compat.features" :key="f.name">
          {{ f.name }}: <strong>{{ f.available ? 'Available' : 'Off' }}</strong>
        </li>
      </ul>
      <p>Update Wayline below. A newer release may support this firmware.</p>
      <p class="vph-muted">Run wayline-ctl doctor on the router and send the output.</p>
      <div class="vph-updates">
        <button type="button" class="vph-button" :disabled="busy" @click="checkUpdates">
          Check for update
        </button>
        <button
          v-if="updateAvailable"
          type="button"
          class="vph-button vph-primary"
          :disabled="busy"
          @click="installUpdate"
        >
          Install update
        </button>
        <span v-if="updateText" class="vph-muted">{{ updateText }}</span>
      </div>
    </section>
    <div
      v-else-if="showCompat"
      class="vph-panel vph-compat"
      :class="level === 'monitoring' ? 'vph-compat-monitoring' : 'vph-compat-note'"
      role="status"
    >
      <p>
        <strong>Firmware {{ compat.firmware || 'unknown' }}</strong> {{ compat.message }}
      </p>
      <ul v-if="compat.features.length" class="vph-features" aria-label="Features">
        <li v-for="f in compat.features" :key="f.name">
          {{ f.name }}: <strong>{{ f.available ? 'Available' : 'Off' }}</strong>
        </li>
      </ul>
      <div class="vph-updates">
        <button type="button" class="vph-button" :disabled="busy" @click="checkUpdates">
          Check for update
        </button>
        <button
          v-if="updateAvailable"
          type="button"
          class="vph-button vph-primary"
          :disabled="busy"
          @click="installUpdate"
        >
          Install update
        </button>
        <span v-if="updateText" class="vph-muted">{{ updateText }}</span>
      </div>
    </div>
    <header class="vph-heading">
      <div>
        <div class="vph-eyebrow">VPN</div>
        <h1>Wayline</h1>
        <p>Compare your tunnels and choose an exit.</p>
      </div>
      <button
        ref="settingsButton"
        class="vph-button"
        :disabled="!can.configure"
        :aria-expanded="settingsOpen ? 'true' : 'false'"
        aria-controls="vph-settings"
        @click="openSettings"
      >
        Settings
      </button>
    </header>
    <p class="vph-statusline" role="status">{{ statusLine }}</p>
    <p v-if="!can.configure && ready" class="vph-muted vph-hint">{{ configureReason }}</p>
    <div role="alert">
      <div v-if="!ready && readError" class="vph-panel vph-fail">
        <h2>Wayline could not read the tunnel state.</h2>
        <p class="vph-fail-text">{{ readError }}</p>
        <p class="vph-muted">Run wayline-ctl doctor on the router and send the output.</p>
        <button type="button" class="vph-button" :disabled="loading" @click="retry">Retry</button>
        <div class="vph-updates">
          <button type="button" class="vph-button" :disabled="busy" @click="checkUpdates">
            Check for update
          </button>
          <button
            v-if="updateAvailable"
            type="button"
            class="vph-button vph-primary"
            :disabled="busy"
            @click="installUpdate"
          >
            Install update
          </button>
          <span v-if="updateText" class="vph-muted">{{ updateText }}</span>
        </div>
      </div>
      <div v-if="error || (ready && readError)" ref="alert" class="vph-error">
        <span>{{ error || readError }}</span>
        <button v-if="error" type="button" class="vph-link" @click="error = ''">Dismiss</button>
      </div>
    </div>
    <div role="status">
      <div v-if="notice" class="vph-notice">
        <span>{{ notice }}</span>
        <button type="button" class="vph-link" @click="notice = ''">Dismiss</button>
      </div>
    </div>

    <form
      v-if="settingsOpen"
      id="vph-settings"
      ref="settings"
      class="vph-panel vph-settings"
      aria-labelledby="vph-settings-title"
      @submit.prevent="saveSettings"
      @keydown.esc="closeSettings"
    >
      <h2 id="vph-settings-title">Settings</h2>
      <label class="vph-check"
        ><input type="checkbox" v-model="draft.enabled" /> Measure tunnel latency</label
      >
      <div class="vph-fields">
        <label
          >Comparison target<input
            ref="firstField"
            v-model.trim="draft.target"
            required
            maxlength="253"
            placeholder="1.1.1.1"
            autocomplete="off"
            autocapitalize="off"
            spellcheck="false"
          /><small
            >Use an IPv4 address or a host name. The monitor uses the same target for every
            tunnel.</small
          ><small v-if="resolvedText(draft.target)">{{ resolvedText(draft.target) }}</small></label
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
        ><input type="checkbox" v-model="draft.autoExit" /> Switch the exit automatically</label
      >
      <p class="vph-muted">
        Wayline moves the best healthy tunnel to the top priority. It looks only at monitored
        tunnels that are eligible. A tunnel must be at least 20% and 10 ms better. Wayline waits 5
        minutes between switches. If the top tunnel is not healthy, it switches after 1 minute. You
        can still choose an exit yourself.
      </p>
      <p v-if="draft.autoExit" class="vph-warning" role="note">
        Warning: this changes the native VPN priority by itself. Existing connections do not move.
      </p>
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
          <label :for="'vph-target-' + i">Target {{ i + 1 }}</label>
          <div>
            <input
              :id="'vph-target-' + i"
              v-model.trim="draft.targets[i]"
              required
              maxlength="253"
              autocomplete="off"
              autocapitalize="off"
              spellcheck="false"
              placeholder="IP address or host name"
            /><small v-if="resolvedText(draft.targets[i])">{{
              resolvedText(draft.targets[i])
            }}</small>
          </div>
          <button
            type="button"
            class="vph-link"
            :aria-label="'Remove target ' + (i + 1)"
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
        <small v-if="draft.targets.length === 4">You can add up to 4 targets.</small>
        <label class="vph-check"
          ><input type="checkbox" v-model="draft.directWan" /> Include direct WAN
          measurements</label
        >
        <small
          >Direct WAN probes bypass the VPN. If the WAN probe cannot bypass the VPN, the monitor
          shows Unavailable.</small
        >
      </div>
      <label class="vph-check" :class="{ 'vph-off': !endpointAllowed }"
        ><input
          type="checkbox"
          v-model="draft.endpointDiagnostics"
          :disabled="!endpointAllowed"
          aria-describedby="vph-endpoint-help"
        />
        Measure VPN endpoints</label
      >
      <small id="vph-endpoint-help"
        >Measure the public address of each monitored WireGuard server over WAN. This shows the
        route to the server. It does not show latency through the tunnel.<span
          v-if="!endpointAllowed"
        >
          Turn on Internet diagnostics and direct WAN first.</span
        ></small
      >
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
          <p v-if="updateText" class="vph-muted">{{ updateText }}</p>
          <button
            v-if="updateAvailable"
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
        <button type="button" class="vph-button" @click="closeSettings">Cancel</button
        ><button class="vph-button vph-primary" :disabled="busy">
          {{ busy ? 'Saving…' : 'Save settings' }}
        </button>
      </footer>
    </form>

    <nav class="vph-tabs" aria-label="Wayline views">
      <button
        :class="{ active: tab === 'tunnels' }"
        :aria-pressed="tab === 'tunnels' ? 'true' : 'false'"
        @click="tab = 'tunnels'"
      >
        Exit tunnels</button
      ><button
        :class="{ active: tab === 'diagnostics' }"
        :aria-pressed="tab === 'diagnostics' ? 'true' : 'false'"
        @click="tab = 'diagnostics'"
      >
        Internet diagnostics
      </button>
    </nav>
    <template v-if="tab === 'tunnels'">
      <div class="vph-scope">
        <span
          >Exit preference for <strong>{{ state.scope || 'Scope unavailable' }}</strong></span
        ><span class="vph-muted"
          >Target {{ state.config.target }}
          <span v-if="resolvedText(state.config.target)"
            >({{ resolvedText(state.config.target) }})</span
          >
          · every {{ state.config.interval }}s</span
        >
      </div>
      <p v-if="!can.select && selectReason" class="vph-muted vph-hint">{{ selectReason }}</p>
      <div v-if="state.auto && state.auto.enabled" class="vph-panel vph-info">
        <strong>Automatic exit switching is on.</strong>
        <span v-if="state.auto.last">
          Last switch
          <time :title="new Date(state.auto.last.at).toLocaleString()">{{
            ago(state.auto.last.at)
          }}</time
          >: {{ tunnelName(state.auto.last.from) }} → {{ tunnelName(state.auto.last.to) }}.
          {{ state.auto.last.reason }}</span
        >
        <span v-else class="vph-muted"> No switch yet.</span>
      </div>
      <div
        class="vph-panel vph-table-wrap"
        tabindex="0"
        role="region"
        aria-label="Exit tunnels table"
      >
        <table>
          <thead>
            <tr>
              <th scope="col">Tunnel</th>
              <th scope="col">Latency</th>
              <th scope="col">Loss</th>
              <th scope="col" class="vph-spark-column">Recent latency</th>
              <th scope="col"><span class="vph-sr">Actions</span></th>
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
                  :aria-pressed="selectedTunnel === t.id ? 'true' : 'false'"
                  :aria-label="'Show history for ' + t.name"
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
                <div v-if="pendingStop === t.id" class="vph-actions">
                  <button
                    ref="confirm"
                    class="vph-button vph-danger"
                    :disabled="busy"
                    :aria-label="'Confirm stop ' + t.name"
                    @click="setTunnelEnabled(t)"
                  >
                    Stop tunnel
                  </button>
                  <button class="vph-button" @click="pendingStop = ''">Keep running</button>
                </div>
                <div v-else class="vph-actions">
                  <span v-if="t.preferred" class="vph-muted">Highest priority</span
                  ><button
                    v-else
                    class="vph-button"
                    :disabled="busy || !t.eligible || !can.select"
                    :aria-label="'Use ' + t.name + ' as exit'"
                    @click="selectExit(t)"
                  >
                    {{ busyTunnel === t.id && busyAction === 'select' ? 'Applying…' : 'Use exit' }}
                  </button>
                  <button
                    class="vph-button"
                    :disabled="busy || !t.controllable || !can.control"
                    :aria-label="(t.enabled ? 'Stop ' : 'Start ') + t.name"
                    @click="askToggle(t)"
                  >
                    {{ toggleLabel(t) }}
                  </button>
                </div>
                <small v-if="pendingStop === t.id"
                  >Stop {{ t.name }}? Traffic can use WAN under your GL.iNet policy.</small
                >
                <small v-if="exitHint(t)">{{ exitHint(t) }}</small>
                <small v-if="!t.controllable">Native tunnel control is unavailable.</small>
                <small v-else-if="!can.control">{{ controlReason }}</small>
              </td>
            </tr>
          </tbody>
        </table>
        <div v-if="!state.tunnels.length" class="vph-empty" role="status">
          {{ emptyText }}
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
        <button class="vph-button" :disabled="!can.configure" @click="openSettings">
          Enable in settings
        </button>
      </section>
      <div
        v-else
        class="vph-panel vph-table-wrap"
        tabindex="0"
        role="region"
        aria-label="Internet diagnostics table"
      >
        <table>
          <thead>
            <tr>
              <th scope="col">Destination</th>
              <th scope="col" v-if="state.config.directWan">Direct WAN</th>
              <th scope="col" v-for="t in state.tunnels" :key="t.id">{{ t.name }}</th>
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
                  :aria-pressed="selectedTarget === target ? 'true' : 'false'"
                  :aria-label="'Show history for ' + target"
                  @click="
                    selectedTarget = target;
                    loadHistory();
                  "
                >
                  {{ target }}
                </button>
                <div v-if="resolvedText(target)" class="vph-muted vph-status">
                  {{ resolvedText(target) }}
                </div>
              </td>
              <td v-if="state.config.directWan" class="vph-number">
                {{ diagnostic(target, 'wan') }}
              </td>
              <td v-for="t in state.tunnels" :key="t.id" class="vph-number">
                {{ diagnostic(target, t.id) }}
              </td>
            </tr>
            <tr
              v-for="e in endpoints"
              :key="'endpoint-' + e.tunnel + e.address"
              class="vph-endpoint"
            >
              <td>
                Endpoint of {{ e.name }}
                <div class="vph-muted vph-status">{{ e.address }}</div>
              </td>
              <td class="vph-number">{{ diagnostic(e.address, 'wan') }}</td>
              <td v-for="t in state.tunnels" :key="t.id" class="vph-number vph-muted">
                <span aria-hidden="true">—</span><span class="vph-sr">Not measured</span>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
      <p v-if="state.config.endpointDiagnostics" class="vph-muted">
        {{
          endpoints.length
            ? 'Endpoint rows show the route to the VPN server over WAN. They do not show latency through the tunnel.'
            : 'No VPN endpoints found yet. Endpoint rows show the route to the server, not latency through the tunnel.'
        }}
      </p>
    </template>

    <section v-if="tab === 'tunnels' || state.config.diagnostics" class="vph-panel vph-history">
      <header class="vph-chart-heading">
        <div>
          <h2>{{ selectedName }} <span class="vph-muted">latency</span></h2>
          <p v-if="storedHistory" class="vph-stored">Stored history (tunnel stopped)</p>
          <p class="vph-muted">
            {{ graphTarget }} ·
            {{ range === 1440 ? '5-minute aggregates' : '1-minute aggregates' }} · gaps indicate
            missing measurements
          </p>
        </div>
        <div class="vph-ranges" role="group" aria-label="History range">
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
          tabindex="0"
          :aria-label="
            'Latency history for ' +
            selectedName +
            ' to ' +
            graphTarget +
            '. Use the arrow keys to read values.'
          "
          @mousemove="hoverAt"
          @click="hoverAt"
          @keydown="keyHover"
          @blur="hover = null"
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
        <div v-if="chartNote" class="vph-chart-note" role="status">{{ chartNote }}</div>
        <div
          v-if="hover"
          role="status"
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
          :aria-valuetext="clock(windowStart) + ' to ' + clock(windowStart + range * 60000)"
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
      {{ preview ? 'Design preview' : 'Wayline' }} · {{ state.version || '0.1.0' }}
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
  autoExit: false,
  endpointDiagnostics: false,
});
const validTarget = (value) =>
  typeof value === 'string' &&
  value.length > 0 &&
  value.length <= 253 &&
  /^[A-Za-z0-9.-]+$/.test(value);
export default {
  name: 'WaylineDashboard',
  props: { api: Object, preview: Boolean },
  data() {
    return {
      state: {
        config: defaults(),
        tunnels: [],
        capabilities: { configure: false, select: false, control: false },
        compat: { level: 'full', firmware: '', message: '' },
        scope: '',
        revision: '',
        auto: { enabled: false, last: null },
        endpoints: [],
        resolved: {},
      },
      loaded: false,
      ready: false,
      updateInfo: null,
      pendingStop: '',
      busyAction: '',
      noticeTimer: null,
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
      historyLoaded: false,
      ranges: [
        { label: '15m', minutes: 15 },
        { label: '1h', minutes: 60 },
        { label: '6h', minutes: 360 },
        { label: '24h', minutes: 1440 },
      ],
    };
  },
  computed: {
    compat() {
      const c = this.state.compat || {};
      const level = ['monitoring', 'unsupported'].includes(c.level) ? c.level : 'full';
      return {
        level,
        verified: c.verified !== false,
        firmware: c.firmware || '',
        message: c.message || '',
        features: Array.isArray(c.features) ? c.features.filter((f) => f && f.name) : [],
      };
    },
    showCompat() {
      return this.level !== 'full' || !this.compat.verified;
    },
    updateAvailable() {
      return !!(
        (this.updateInfo && this.updateInfo.available) ||
        (this.state.update || {}).available
      );
    },
    updateText() {
      return (
        (this.updateInfo && this.updateInfo.message) || (this.state.update || {}).message || ''
      );
    },
    level() {
      return this.compat.level;
    },
    can() {
      const c = this.state.capabilities || {};
      const full = this.level === 'full';
      return {
        configure: !!c.configure && this.level !== 'unsupported',
        select: !!c.select && full,
        control: !!c.control && full,
      };
    },
    configureReason() {
      if (this.level === 'unsupported')
        return 'This firmware is not supported. Settings are unavailable.';
      return 'Settings are unavailable. You do not have permission to change them.';
    },
    selectReason() {
      if (this.level === 'monitoring')
        return "This firmware's VPN module is not verified. Monitoring still works.";
      if (this.level === 'unsupported')
        return 'This firmware is not supported. Use exit is unavailable.';
      return (this.state.capabilities || {}).reason || '';
    },
    controlReason() {
      if (this.level === 'monitoring')
        return "This firmware's VPN module is not verified. Monitoring still works.";
      if (this.level === 'unsupported')
        return 'This firmware is not supported. Start and Stop are unavailable.';
      return 'Start and Stop are unavailable.';
    },
    statusLine() {
      if (!this.ready)
        return this.readError ? 'The tunnel state is not available.' : 'Reading tunnel state…';
      if (this.readError) return 'Showing the last known state. Trying again.';
      return 'Tunnel state is up to date.';
    },
    selectedName() {
      const t = this.state.tunnels.find((t) => t.id === this.selectedTunnel);
      return t ? t.name : 'Tunnel';
    },
    endpoints() {
      return this.state.config.endpointDiagnostics && Array.isArray(this.state.endpoints)
        ? this.state.endpoints
        : [];
    },
    endpointAllowed() {
      return this.draft.diagnostics && this.draft.directWan;
    },
    selectedTunnelState() {
      return this.state.tunnels.find((t) => t.id === this.selectedTunnel);
    },
    storedHistory() {
      const t = this.selectedTunnelState;
      return !!t && !t.connected && !!t.historyAvailable;
    },
    emptyText() {
      if (!this.ready && !this.readError) return 'Reading tunnel state…';
      if (this.readError) return 'Tunnel state could not be read.';
      return 'No configured VPN tunnels available.';
    },
    chartNote() {
      if (this.historyError) return this.historyError;
      if (!this.selectedTunnel) return 'Choose a tunnel to see its history.';
      if (
        !this.history.length ||
        this.history.every((s) => s.points.every((p) => p.median === null))
      )
        return this.historyLoaded ? 'No measurements in this range.' : 'Loading history…';
      return '';
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
    error(value) {
      if (value) this.$nextTick(() => this.reveal());
    },
    notice(value) {
      clearTimeout(this.noticeTimer);
      if (value && !value.endsWith('…'))
        this.noticeTimer = setTimeout(() => (this.notice = ''), 10000);
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
    clearTimeout(this.noticeTimer);
    document.removeEventListener('visibilitychange', this.visibility);
    if (this.resize) this.resize.disconnect();
  },
  methods: {
    call(method, params) {
      return new Promise((resolve, reject) => {
        let request;
        try {
          if (this.api) request = this.api[method](params || {});
          else if (typeof this.$rpcRequest !== 'function')
            throw Error('The router page API is missing. Reload the page or update the firmware.');
          else
            request = this.$rpcRequest('call', ['sid', 'wayline', method, params || {}], {
              timeout: method === 'set_tunnel_enabled' ? 45000 : 12000,
            });
        } catch (e) {
          reject(e);
          return;
        }
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
        if (!next || typeof next !== 'object') throw Error('The router sent an empty state.');
        next.tunnels = Array.isArray(next.tunnels) ? next.tunnels : [];
        next.config = next.config || {};
        next.config.tunnels = Array.isArray(next.config.tunnels) ? next.config.tunnels : [];
        next.capabilities = Object.assign(
          { configure: false, select: false, control: false },
          next.capabilities,
        );
        next.tunnels.forEach((tunnel) => {
          tunnel.recent = Array.isArray(tunnel.recent) ? tunnel.recent : [];
        });
        next.config = Object.assign(defaults(), next.config);
        next.auto = next.auto || { enabled: false, last: null };
        next.endpoints = Array.isArray(next.endpoints) ? next.endpoints : [];
        next.resolved = next.resolved || {};
        this.state = next;
        this.ready = true;
        if (!next.config.targets.includes(this.selectedTarget))
          this.selectedTarget = next.config.targets[0] || next.config.target;
        if (this.offset === 0) this.anchor = Math.floor(Date.now() / 60000) * 60000;
        if (!next.tunnels.some((t) => t.id === this.selectedTunnel))
          this.selectedTunnel =
            (next.tunnels.find((t) => t.preferred) || next.tunnels[0] || {}).id || '';
        this.readError = '';
        await this.loadHistory();
      } catch (e) {
        this.readError = 'Wayline is unavailable. ' + this.describe(e, 'Try again shortly.');
      } finally {
        this.loading = false;
        this.loaded = true;
        if (!this.stopped && !document.hidden) this.timer = setTimeout(this.refresh, 5000);
      }
    },
    describe(e, fallback) {
      if (typeof e === 'string' && e) return e;
      return (e && e.message) || fallback;
    },
    retry() {
      clearTimeout(this.timer);
      this.refresh();
    },
    visibility() {
      clearTimeout(this.timer);
      if (!document.hidden) this.refresh();
    },
    reveal() {
      const el = this.$refs.alert;
      if (el && el.scrollIntoView) el.scrollIntoView({ block: 'nearest' });
    },
    openSettings() {
      if (!this.can.configure) return;
      this.draft = Object.assign(defaults(), JSON.parse(JSON.stringify(this.state.config)));
      this.settingsOpen = !this.settingsOpen;
      if (this.settingsOpen)
        this.$nextTick(() => this.$refs.firstField && this.$refs.firstField.focus());
    },
    closeSettings() {
      this.settingsOpen = false;
      this.$nextTick(() => this.$refs.settingsButton && this.$refs.settingsButton.focus());
    },
    resolvedText(target) {
      const map = this.state.resolved || {};
      if (!target || !Object.prototype.hasOwnProperty.call(map, target)) return '';
      return map[target] ? '→ ' + map[target] : 'cannot resolve';
    },
    tunnelName(id) {
      const t = this.state.tunnels.find((t) => t.id === id || t.monitorId === id || t.name === id);
      return t ? t.name : id;
    },
    ago(at) {
      const s = Math.max(0, Math.round((Date.now() - at) / 1000));
      if (s < 60) return 'just now';
      if (s < 3600) return Math.round(s / 60) + ' min ago';
      if (s < 86400) return Math.round(s / 3600) + ' h ago';
      return Math.round(s / 86400) + ' d ago';
    },
    exitHint(t) {
      if (t.preferred || !this.can.select) return '';
      if (!t.enabled) return 'Start this tunnel to use it as an exit.';
      if (!t.eligible) return t.reason || 'This tunnel is not ready.';
      return '';
    },
    toggleLabel(t) {
      if (this.busyTunnel === t.id && this.busyAction === 'toggle')
        return t.enabled ? 'Stopping…' : 'Starting…';
      return t.enabled ? 'Stop' : 'Start';
    },
    askToggle(t) {
      if (!t.enabled) return this.setTunnelEnabled(t);
      this.pendingStop = t.id;
      this.$nextTick(() => {
        const c = [].concat(this.$refs.confirm || [])[0];
        if (c) c.focus();
      });
    },
    async saveSettings() {
      if (this.busy) return;
      this.error = '';
      const targets = [this.draft.target].concat(this.draft.diagnostics ? this.draft.targets : []);
      if (
        targets.some((t) => !validTarget(t)) ||
        (this.draft.diagnostics &&
          new Set(this.draft.targets.map((t) => t.toLowerCase())).size !==
            this.draft.targets.length)
      ) {
        this.error =
          'Use an IPv4 address or a host name for each target. Each diagnostics target must be unique.';
        return;
      }
      if (this.draft.enabled && !this.draft.tunnels.length) {
        this.error = 'Choose at least one tunnel to monitor.';
        return;
      }
      this.busy = true;
      try {
        await this.call('set_config', {
          config: Object.assign({}, this.draft, {
            endpointDiagnostics: this.draft.endpointDiagnostics && this.endpointAllowed,
          }),
          expected_revision: this.state.revision,
        });
        this.closeSettings();
        this.notice = 'Settings saved.';
        await this.refresh();
      } catch (e) {
        this.error = e.message || 'Settings could not be saved.';
      } finally {
        this.busy = false;
      }
    },
    async selectExit(t) {
      if (this.busy || t.preferred || !t.eligible || !this.can.select) return;
      this.busy = true;
      this.busyTunnel = t.id;
      this.busyAction = 'select';
      this.error = '';
      this.notice = 'Applying exit preference…';
      try {
        await this.call('select_tunnel', {
          section_id: t.id,
          expected_revision: this.state.revision,
        });
        this.notice = t.name + ' now has the highest priority. Existing connections do not move.';
        await this.refresh();
      } catch (e) {
        this.notice = '';
        this.error =
          'Selection outcome needs checking. ' + (e.message || 'Refresh before trying again.');
        await this.refresh();
      } finally {
        this.busy = false;
        this.busyTunnel = '';
        this.busyAction = '';
      }
    },
    async setTunnelEnabled(t) {
      if (this.busy || !t.controllable || !this.can.control) return;
      this.busy = true;
      this.busyTunnel = t.id;
      this.busyAction = 'toggle';
      this.pendingStop = '';
      this.error = '';
      this.notice = (t.enabled ? 'Stopping ' : 'Starting ') + t.name + '…';
      try {
        const result = await this.call('set_tunnel_enabled', {
          section_id: t.id,
          enabled: !t.enabled,
          expected_revision: this.state.revision,
        });
        this.notice = result.enabled
          ? t.name + ' is enabled. Connection and measurements can take time.'
          : t.name + ' is stopped. GL.iNet traffic policy applies.';
        if (result.monitor_pending) this.notice += ' Monitoring will reconcile within one minute.';
      } catch (e) {
        this.notice = '';
        this.error =
          'Tunnel outcome needs checking. ' + (e.message || 'Refresh before trying again.');
      } finally {
        await this.refresh();
        this.busy = false;
        this.busyTunnel = '';
        this.busyAction = '';
      }
    },
    async installUpdate() {
      this.busy = true;
      this.error = '';
      try {
        const r = await this.call('install_update');
        this.updateInfo = r;
        this.notice = r.message;
      } catch (e) {
        this.error = this.describe(e, 'Update unavailable.');
      } finally {
        this.busy = false;
      }
    },
    async checkUpdates() {
      this.busy = true;
      this.error = '';
      try {
        const r = await this.call('check_update');
        this.updateInfo = r;
        this.notice = r.message;
      } catch (e) {
        this.error = this.describe(e, 'Update check unavailable.');
      } finally {
        this.busy = false;
      }
    },
    async loadHistory() {
      const request = ++this.historyRequest;
      const selected = this.state.tunnels.find((tunnel) => tunnel.id === this.selectedTunnel);
      if (selected && !selected.connected && !selected.historyAvailable) {
        this.history = [];
        this.historyError = 'No history is stored for this tunnel. Start it to collect history.';
        this.historyLoaded = true;
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
          this.historyLoaded = true;
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
    keyHover(e) {
      if (e.key === 'Escape') {
        this.hover = null;
        return;
      }
      if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
      e.preventDefault();
      const dir = e.key === 'ArrowRight' ? 1 : -1;
      const from = this.hover
        ? this.hover.x
        : dir > 0
          ? 45 - (this.chartWidth - 63) / 30
          : this.chartWidth - 18 + (this.chartWidth - 63) / 30;
      this.hoverX(from + (dir * (this.chartWidth - 63)) / 30);
    },
    hoverAt(e) {
      const rect = e.currentTarget.getBoundingClientRect();
      this.hoverX(((e.clientX - rect.left) * this.chartWidth) / rect.width);
    },
    hoverX(raw) {
      const x = Math.max(45, Math.min(this.chartWidth - 18, raw));
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
  --vph-brand-text: color-mix(in srgb, var(--vph-brand) 65%, var(--vph-text));
  --vph-brand-strong: color-mix(in srgb, var(--vph-brand) 72%, #000);
  --vph-danger: #c62d3f;
  --vph-danger-text: color-mix(in srgb, var(--vph-danger) 70%, var(--vph-text));
  --vph-warn-text: color-mix(in srgb, #b7791f 55%, var(--vph-text));
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
  opacity: 0.55;
}
.vph-off {
  opacity: 0.6;
}
.vph-sr {
  position: absolute;
  width: 1px;
  height: 1px;
  overflow: hidden;
  clip: rect(0 0 0 0);
  white-space: nowrap;
}
.vph svg:focus-visible,
.vph-table-wrap:focus-visible {
  outline: 2px solid var(--vph-brand);
  outline-offset: 2px;
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
  background: color-mix(in srgb, #e0a800 20%, var(--vph-card));
  color: var(--vph-warn-text);
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
  background: var(--vph-brand-strong);
  border-color: var(--vph-brand-strong);
  color: #fff;
}
.vph-danger {
  border-color: var(--vph-danger);
  color: var(--vph-danger-text);
}
.vph-link,
.vph-name {
  border: 0;
  background: none;
  padding: 0;
  color: var(--vph-brand-text);
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
  color: var(--vph-brand-text);
}
.vph-scope {
  margin: 18px 0 12px;
}
.vph-table-wrap {
  overflow-x: auto;
  min-height: 120px;
}
.vph-hint {
  margin: 8px 0;
  font-size: 12px;
}
.vph-info {
  padding: 12px 18px;
}
.vph-warning {
  color: var(--vph-warn-text);
  font-size: 13px;
}
.vph-stored {
  color: var(--vph-brand-text);
  font-size: 12px;
  margin: 2px 0 0;
}
.vph-endpoint td {
  background: color-mix(in srgb, var(--vph-muted) 5%, var(--vph-card));
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
  background: color-mix(in srgb, var(--vph-brand) 5%, var(--vph-card));
}
.vph tr.selected td:first-child {
  box-shadow: inset 3px 0 var(--vph-brand);
}
.vph-number {
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}
.vph .vph-number small {
  display: inline;
  margin: 0;
  font-size: 12px;
}
.vph-badge {
  display: inline-block;
  border-radius: 4px;
  font-size: 11px;
  color: var(--vph-brand-text);
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
.vph-actions {
  display: flex;
  flex-wrap: wrap;
  justify-content: flex-end;
  align-items: center;
  gap: 8px;
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
  color: var(--vph-brand-text);
}
.vph-chart-note {
  position: absolute;
  left: 45px;
  right: 18px;
  top: 100px;
  text-align: center;
  color: var(--vph-muted);
  pointer-events: none;
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
  align-items: start;
  max-width: 500px;
  margin: 9px 0;
}
.vph-target input {
  margin-top: 0 !important;
}
.vph-target > label,
.vph-target > button {
  padding-top: 9px;
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
.vph-error,
.vph-notice {
  display: flex;
  justify-content: space-between;
  gap: 16px;
  padding: 12px;
  border-radius: 4px;
  margin: 12px 0;
}
.vph-error {
  color: var(--vph-danger-text);
  background: color-mix(in srgb, var(--vph-danger) 10%, var(--vph-card));
  border: 1px solid color-mix(in srgb, var(--vph-danger) 35%, var(--vph-card));
}
.vph-notice {
  color: var(--vph-brand-text);
  background: color-mix(in srgb, var(--vph-brand) 8%, var(--vph-card));
}
.vph-error .vph-link,
.vph-notice .vph-link {
  padding: 0;
  white-space: nowrap;
  color: inherit;
  text-decoration: underline;
}
.vph-footnote {
  font-size: 12px;
  color: var(--vph-muted);
  padding: 12px 0 25px;
}
.vph-statusline {
  margin: 0 0 8px !important;
  font-size: 13px;
  color: var(--vph-muted);
}
.vph-compat {
  display: flex;
  flex-wrap: wrap;
  gap: 4px 12px;
  padding: 12px;
  border-radius: 5px;
  margin-bottom: 12px;
}
.vph-compat-monitoring {
  color: var(--vph-warn-text);
  background: color-mix(in srgb, #e0a800 18%, var(--vph-card));
  border: 1px solid color-mix(in srgb, #e0a800 45%, var(--vph-card));
}
.vph-compat-unsupported {
  color: var(--vph-danger-text);
  background: color-mix(in srgb, var(--vph-danger) 10%, var(--vph-card));
  border: 1px solid color-mix(in srgb, var(--vph-danger) 35%, var(--vph-card));
}
.vph-compat {
  padding: 14px 18px;
}
.vph-compat p {
  margin: 4px 0 10px;
}
.vph-compat-note {
  color: var(--vph-text);
  background: color-mix(in srgb, var(--vph-brand) 6%, var(--vph-card));
}
.vph-features {
  list-style: none;
  display: flex;
  flex-wrap: wrap;
  gap: 4px 20px;
  padding: 0;
  margin: 8px 0 12px;
}
.vph-updates {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px 12px;
  margin: 10px 0 4px;
}
.vph-fail {
  padding: 20px;
  border-color: color-mix(in srgb, var(--vph-danger) 45%, var(--vph-card));
}
.vph-fail h2 {
  color: var(--vph-danger-text);
}
.vph-fail-text {
  overflow-wrap: anywhere;
}
.vph-button:hover:enabled {
  border-color: var(--vph-brand);
}
.vph-button {
  min-height: 36px;
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
  .vph-action {
    min-width: 0;
  }
  .vph-button,
  .vph-tabs button,
  .vph-ranges button {
    min-height: 44px;
  }
  .vph-link {
    min-height: 44px;
  }
  .vph-action small {
    max-width: none;
  }
  .vph-info {
    padding: 12px;
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
