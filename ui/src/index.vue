<template>
  <main class="vph">
    <div v-if="preview" class="vph-preview">
      Interactive preview with sample data. No router changes.
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
    <div v-if="staleBundle" class="vph-notice" role="status">
      This page is from Wayline {{ bundleVersion }}, but the router runs {{ state.version }}.
      <button type="button" class="vph-button" @click="reloadPage">Reload page</button>
    </div>
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
        >
        <span class="vph-muted"
          >Target {{ state.config.target
          }}<template v-if="resolvedText(state.config.target)"
            >. {{ resolvedText(state.config.target) }}</template
          >. Checked every {{ state.config.interval }} seconds.</span
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
          >: from {{ tunnelName(state.auto.last.from) }} to {{ tunnelName(state.auto.last.to) }}.
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
              <th scope="col" class="vph-spark-column">Recent</th>
              <th scope="col" class="vph-action-head"><span class="vph-sr">Actions</span></th>
            </tr>
          </thead>
          <tbody>
            <tr
              v-for="t in state.tunnels"
              :key="t.id"
              :class="{ selected: selectedTunnel === t.id, 'vph-active': t.preferred }"
            >
              <td class="vph-tunnel">
                <div class="vph-name-line">
                  <span
                    class="vph-state-dot"
                    :class="'vph-state-' + tunnelTone(t)"
                    aria-hidden="true"
                  ></span>
                  <button
                    class="vph-name"
                    :aria-pressed="selectedTunnel === t.id ? 'true' : 'false'"
                    :aria-label="'Highlight ' + t.name + ' in the latency graph'"
                    @click="highlight(t.id)"
                  >
                    {{ t.name }}</button
                  ><span v-if="t.preferred" class="vph-badge">Active exit</span>
                </div>
                <div class="vph-muted vph-status">{{ t.stateLabel }}{{ tunnelEgress(t) }}</div>
                <div v-if="notEligible(t)" class="vph-muted vph-status">
                  Not eligible: {{ t.reason || 'not ready yet' }}
                </div>
              </td>
              <td class="vph-metric" data-label="Latency">
                <span class="vph-number"
                  >{{ latency(t.median) }}<small v-if="t.median !== null"> ms</small></span
                >
                <span v-if="lossShown(t)" class="vph-loss" :title="lossTitle(t)"
                  >{{ lossText(t) }} loss</span
                >
                <span class="vph-bar" aria-hidden="true"
                  ><span
                    class="vph-bar-fill"
                    :class="{ 'vph-bar-best': t.id === fastestId }"
                    :style="{ width: barWidth(t) + '%' }"
                  ></span
                ></span>
              </td>
              <td class="vph-spark-column">
                <svg
                  v-if="sparkStats(t.recent)"
                  class="vph-spark"
                  viewBox="0 0 128 24"
                  preserveAspectRatio="none"
                  role="img"
                  :aria-label="sparkLabel(t)"
                >
                  <title>{{ sparkLabel(t) }}</title>
                  <rect
                    v-for="b in sparkBars(t.recent)"
                    :key="b.i"
                    :x="b.x"
                    :y="b.y"
                    :width="b.w"
                    :height="b.h"
                    :class="{ 'vph-spark-lost': b.lost }"
                  />
                </svg>
                <span v-else class="vph-muted">Waiting for samples</span>
              </td>
              <td class="vph-action" :class="{ 'vph-confirming': pendingStop === t.id }">
                <div v-if="pendingStop === t.id" class="vph-confirm">
                  <span class="vph-confirm-text">Stop {{ t.name }}? Traffic may use WAN.</span>
                  <div class="vph-actions">
                    <button
                      ref="confirm"
                      type="button"
                      class="vph-control vph-danger"
                      :disabled="busy"
                      :aria-label="'Confirm stop ' + t.name"
                      @click="setTunnelEnabled(t)"
                    >
                      Stop
                    </button>
                    <button type="button" class="vph-control" @click="pendingStop = ''">
                      Cancel
                    </button>
                  </div>
                </div>
                <div v-else class="vph-actions">
                  <button
                    v-if="!t.preferred && t.enabled"
                    type="button"
                    class="vph-control"
                    :disabled="useDisabled(t)"
                    :title="useReason(t)"
                    :aria-label="'Use ' + t.name + ' as exit'"
                    @click="selectExit(t)"
                  >
                    {{
                      busyTunnel === t.id && busyAction === 'select' ? 'Applying…' : 'Use as exit'
                    }}
                  </button>
                  <span
                    v-if="busyTunnel === t.id && busyAction === 'toggle'"
                    class="vph-muted vph-busy"
                    role="status"
                    >{{ t.enabled ? 'Stopping…' : 'Starting…' }}</span
                  >
                  <button
                    v-else
                    type="button"
                    role="switch"
                    class="vph-switch"
                    :aria-checked="t.enabled ? 'true' : 'false'"
                    :disabled="toggleDisabled(t)"
                    :title="toggleReason(t)"
                    @click="askToggle(t)"
                  >
                    <span class="vph-sr">{{ (t.enabled ? 'Stop ' : 'Start ') + t.name }}</span>
                    <span class="vph-track" aria-hidden="true"></span>
                  </button>
                </div>
              </td>
            </tr>
          </tbody>
        </table>
        <div v-if="!state.tunnels.length" class="vph-empty" role="status">
          {{ emptyText }}
        </div>
      </div>
      <p v-if="wanEgress" class="vph-muted vph-note">
        {{ egressText(wanEgress, 'Direct WAN exit IP') }}
        <span v-if="wanEgress.error" class="vph-egress-error">{{ wanEgress.error }}</span>
      </p>
      <p class="vph-muted vph-note">
        Changing the active exit applies to new connections. Stopping a tunnel can send traffic over
        WAN under your GL.iNet policy.
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
              <th scope="col" v-if="state.config.directWan">{{ wanHeader }}</th>
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
                {{ wanCell(target) }}
              </td>
              <td v-for="t in state.tunnels" :key="t.id" class="vph-number">
                {{ diagnostic(target, t.id) }}
              </td>
            </tr>
            <tr v-for="e in endpoints" :key="'endpoint-' + e.tunnel" class="vph-endpoint">
              <td>
                Endpoint of {{ e.name }}
                <div class="vph-muted vph-status">
                  {{ e.address || e.reason || 'No address yet' }}
                </div>
              </td>
              <td v-if="state.config.directWan" class="vph-number">
                {{ e.address ? wanCell(e.address) : '—' }}
              </td>
              <td v-for="t in state.tunnels" :key="t.id" class="vph-number vph-muted">
                <span aria-hidden="true">—</span><span class="vph-sr">Not measured</span>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
      <p v-if="state.config.directWan && !wanAvailable" class="vph-muted" role="note">
        {{ wanNote }}
      </p>
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
          <h2 v-if="allMode">Latency to {{ graphTarget }} through each tunnel</h2>
          <h2 v-else>{{ selectedName }} <span class="vph-muted">latency</span></h2>
          <p v-if="!allMode && storedHistory" class="vph-stored">Stored history (tunnel stopped)</p>
          <p v-if="allMode" class="vph-muted">
            {{ range === 1440 ? 'Five-minute' : 'One-minute' }} medians. Gaps mean no measurement.
          </p>
          <p v-else class="vph-muted">
            Showing {{ graphTarget }}. {{ range === 1440 ? 'Five-minute' : 'One-minute' }}
            averages. Gaps mean no measurement.
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
            allMode
              ? 'Latency to ' +
                graphTarget +
                ' through each tunnel. Use the arrow keys to read values.'
              : 'Latency history for ' +
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
            Each line shows one tunnel's median latency. The highlighted tunnel also shows its
            minimum to maximum band. Missing measurements appear as gaps.
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
            v-for="series in drawOrder"
            :key="series.id"
            :class="{
              'vph-wan-series': series.id === 'wan',
              'vph-series-selected': isSelected(series),
              'vph-series-dim': isDimmed(series),
            }"
            :style="{ '--series': seriesColor(series) }"
          >
            <path v-if="hasBand(series)" :d="band(series.points)" class="vph-band" />
            <path :d="line(series.points)" class="vph-line" />
            <circle
              v-for="dot in dots(series.points)"
              :key="dot.key"
              :cx="dot.x"
              :cy="dot.y"
              r="2.5"
              class="vph-dot"
            />
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
          :style="{ left: Math.min(Math.max(hover.x, 0), chartWidth - 268) + 'px' }"
        >
          <strong>{{ clock(hover.at) }}</strong>
          <div v-for="row in hoverRows" :key="row.id" :class="{ 'vph-tip-selected': row.selected }">
            <span
              class="vph-swatch vph-tip-swatch"
              :class="{ 'vph-swatch-wan': row.id === 'wan' }"
              :style="{ '--series': row.color }"
              aria-hidden="true"
            ></span
            >{{ row.name }}{{ row.selected ? ' (selected)' : '' }}:
            {{ row.value === null ? 'No measurement' : row.value + ' ms' }}
          </div>
        </div>
      </div>
      <div class="vph-chart-footer">
        <ul v-if="legendItems.length" class="vph-legend" aria-label="Tunnels in the graph">
          <li v-for="item in legendItems" :key="item.id">
            <button
              v-if="item.tunnel"
              type="button"
              class="vph-legend-item"
              :class="{ active: item.selected }"
              :aria-pressed="item.selected ? 'true' : 'false'"
              @click="highlight(item.id)"
            >
              <span class="vph-swatch" :style="{ '--series': item.color }" aria-hidden="true"></span
              >{{ item.name }}<span v-if="item.preferred" class="vph-muted"> Active exit</span
              ><span v-if="item.stored" class="vph-muted"> Stored</span>
            </button>
            <span v-else class="vph-legend-item vph-muted">
              <span class="vph-swatch vph-swatch-wan" aria-hidden="true"></span>{{ item.name }}
            </span>
          </li>
        </ul>
        <span class="vph-muted vph-range-label">{{
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
      {{ preview ? 'Design preview' : 'Wayline' }}, version {{ state.version || '0.1.0' }}
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
const isNumber = (value) => typeof value === 'number' && Number.isFinite(value);
const palette = [
  '#e69f00',
  '#0072b2',
  '#d55e00',
  '#cc79a7',
  '#56b4e9',
  '#b8860b',
  '#5b4bb7',
  '#a6761d',
];
const historyInterval = 30000;
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
        wan: { available: true, interface: null, reason: '' },
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
      historyAt: 0,
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
    latencyMax() {
      const values = this.state.tunnels.map((t) => t.median).filter((n) => typeof n === 'number');
      return values.length ? Math.max(...values) : 0;
    },
    fastestId() {
      const measured = this.state.tunnels.filter((t) => typeof t.median === 'number');
      if (!measured.length) return '';
      return measured.reduce((a, b) => (b.median < a.median ? b : a)).id;
    },
    endpoints() {
      return this.state.config.endpointDiagnostics && Array.isArray(this.state.endpoints)
        ? this.state.endpoints
        : [];
    },
    endpointAllowed() {
      return this.draft.diagnostics && this.draft.directWan;
    },
    wanAvailable() {
      return !!(this.state.wan && this.state.wan.available);
    },
    wanHeader() {
      const wan = this.state.wan || {};
      return this.wanAvailable && wan.interface
        ? 'Direct WAN (' + wan.interface + ')'
        : 'Direct WAN';
    },
    wanNote() {
      const reason = (this.state.wan || {}).reason;
      return 'Direct WAN is unavailable. ' + (reason || 'The router did not give a reason.');
    },
    bundleVersion() {
      return WAYLINE_VERSION;
    },
    staleBundle() {
      return Boolean(this.state.version) && this.state.version !== WAYLINE_VERSION;
    },
    wanEgress() {
      return this.egressFor('wan');
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
        this.history.every((s) => s.points.every((p) => !isNumber(p.median)))
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
          if (isNumber(p.max)) n = Math.max(n, p.max);
          if (isNumber(p.median)) n = Math.max(n, p.median);
        }),
      );
      return Math.ceil((n * 1.15) / 25) * 25;
    },
    allMode() {
      return this.tab === 'tunnels';
    },
    drawOrder() {
      const others = this.history.filter((s) => !this.isSelected(s));
      return others.concat(this.history.filter((s) => this.isSelected(s)));
    },
    legendItems() {
      return this.history.map((s) => {
        const t = this.state.tunnels.find((tunnel) => tunnel.id === s.id);
        return {
          id: s.id,
          name: s.name,
          tunnel: s.id !== 'wan',
          selected: this.isSelected(s),
          color: this.seriesColor(s),
          preferred: !!t && !!t.preferred,
          stored: !!t && !t.connected,
        };
      });
    },
    hoverRows() {
      if (!this.hover) return [];
      const rows = this.history.map((s) => {
        let nearest = null;
        for (const p of s.points)
          if (!nearest || Math.abs(p.at - this.hover.at) < Math.abs(nearest.at - this.hover.at))
            nearest = p;
        return {
          id: s.id,
          name: s.name,
          color: this.seriesColor(s),
          selected: this.isSelected(s),
          value: nearest && isNumber(nearest.median) ? nearest.median : null,
        };
      });
      return rows.sort((a, b) => {
        if (a.value === null || b.value === null) return (a.value === null) - (b.value === null);
        return a.value - b.value || a.name.localeCompare(b.name);
      });
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
        next.wan = Object.assign({ available: true, interface: null, reason: '' }, next.wan);
        this.state = next;
        this.ready = true;
        if (!next.config.targets.includes(this.selectedTarget))
          this.selectedTarget = next.config.targets[0] || next.config.target;
        if (!next.tunnels.some((t) => t.id === this.selectedTunnel))
          this.selectedTunnel =
            (next.tunnels.find((t) => t.preferred) || next.tunnels[0] || {}).id || '';
        this.readError = '';
        if (Date.now() - this.historyAt >= historyInterval) await this.loadHistory();
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
      return map[target] ? 'Resolves to ' + map[target] : 'Cannot resolve';
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
    tunnelTone(t) {
      if (!t.enabled) return 'stopped';
      return t.connected ? 'connected' : 'offline';
    },
    notEligible(t) {
      return !t.preferred && t.enabled && !t.eligible && this.can.select;
    },
    tunnelEgress(t) {
      const e = this.egressFor(t.id);
      if (!e || !e.ip || e.error) return '';
      return ', exit IP ' + e.ip + (e.location ? ' in ' + e.location : '');
    },
    lossShown(t) {
      return !!t.samples && t.lost > 0;
    },
    lossText(t) {
      const pct = (t.lost / t.samples) * 100;
      if (pct < 1) return '<1%';
      return Math.round(pct) + '%';
    },
    lossTitle(t) {
      return t.lost + ' of ' + t.samples + ' samples lost';
    },
    barWidth(t) {
      if (typeof t.median !== 'number' || !this.latencyMax) return 0;
      return Math.max(2, (t.median / this.latencyMax) * 100);
    },
    useDisabled(t) {
      return this.busy || !t.eligible || !this.can.select;
    },
    useReason(t) {
      if (!this.can.select) return this.selectReason;
      if (!t.eligible) return t.reason || 'This tunnel is not ready.';
      return '';
    },
    toggleDisabled(t) {
      return this.busy || !t.controllable || !this.can.control;
    },
    toggleReason(t) {
      if (!t.controllable) return 'Native tunnel control is unavailable.';
      if (!this.can.control) return this.controlReason;
      return '';
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
      let saved = false;
      try {
        await this.call('set_config', {
          config: Object.assign({}, this.draft, {
            endpointDiagnostics: this.draft.endpointDiagnostics && this.endpointAllowed,
          }),
          expected_revision: this.state.revision,
        });
        saved = true;
      } catch (e) {
        this.error = this.describe(e, 'Settings could not be saved.');
      } finally {
        this.busy = false;
      }
      if (saved) {
        this.closeSettings();
        this.notice = 'Settings saved.';
      }
      await this.refresh();
      if (!saved && this.settingsOpen)
        this.draft = Object.assign(defaults(), JSON.parse(JSON.stringify(this.state.config)));
    },
    reloadPage() {
      fetch('/views/gl-sdk4-ui-wayline.common.js', { cache: 'reload' }).finally(() =>
        window.location.reload(),
      );
    },
    egressFor(id) {
      const paths = (this.state.egress && this.state.egress.paths) || [];
      return paths.find((p) => p.id === id) || null;
    },
    egressText(p, label) {
      if (!p.ip) return label + ' unavailable.';
      let text = label + ' ' + p.ip;
      if (p.location) text += ' in ' + p.location;
      if (p.colo) text += ' (' + p.colo + ')';
      return text;
    },
    wanCell(target) {
      return this.wanAvailable ? this.diagnostic(target, 'wan') : 'Unavailable';
    },
    async selectExit(t) {
      if (this.busy || t.preferred || !t.eligible || !this.can.select) return;
      this.busy = true;
      this.busyTunnel = t.id;
      this.busyAction = 'select';
      this.error = '';
      this.notice = 'Changing the exit to ' + t.name + '…';
      try {
        await this.call('select_tunnel', {
          section_id: t.id,
          expected_revision: this.state.revision,
        });
        this.notice = 'Now using ' + t.name + ' as the exit.';
        await this.refresh();
      } catch {
        this.notice = '';
        this.error =
          'Could not confirm the exit change. Refresh, then check the active exit before trying again.';
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
      } catch {
        this.notice = '';
        this.error =
          'Could not confirm the change to ' +
          t.name +
          '. Refresh, then check its state before trying again.';
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
      if (this.offset === 0) this.anchor = Math.floor(Date.now() / 60000) * 60000;
      const request = ++this.historyRequest;
      this.historyAt = Date.now();
      const selected = this.state.tunnels.find((tunnel) => tunnel.id === this.selectedTunnel);
      if (!this.allMode && selected && !selected.connected && !selected.historyAvailable) {
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
          all: this.allMode,
          start: this.windowStart,
          end: this.windowStart + this.range * 60000,
          step: this.range === 1440 ? 5 : 1,
          include_wan: !this.allMode && this.state.config.directWan,
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
      const x = 45 + ((at - this.windowStart) / (this.range * 60000)) * (this.chartWidth - 63);
      return Math.min(this.chartWidth - 18, Math.max(45, x));
    },
    chartY(v) {
      return 200 - (v / this.yMax) * 176;
    },
    line(points) {
      let d = '',
        move = true;
      points.forEach((p) => {
        if (!isNumber(p.median)) {
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
    dots(points) {
      return points
        .filter(
          (p, i) =>
            isNumber(p.median) &&
            !isNumber((points[i - 1] || {}).median) &&
            !isNumber((points[i + 1] || {}).median),
        )
        .map((p) => ({ key: p.at, x: this.chartX(p.at), y: this.chartY(p.median) }));
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
        if (isNumber(p.median) && isNumber(p.min) && isNumber(p.max)) group.push(p);
        else flush();
      });
      flush();
      return d;
    },
    sparkStats(values) {
      const valid = values.filter(isNumber);
      if (valid.length < 2) return null;
      const sorted = valid.slice().sort((a, b) => a - b);
      const half = Math.floor(sorted.length / 2);
      const median = sorted.length % 2 ? sorted[half] : (sorted[half - 1] + sorted[half]) / 2;
      return {
        min: sorted[0],
        max: sorted[sorted.length - 1],
        median,
        lost: values.length - valid.length,
      };
    },
    sparkLabel(t) {
      const s = this.sparkStats(t.recent);
      const head = 'Recent latency for ' + t.name + ': ';
      if (!s) return head + 'waiting for samples.';
      const lost = s.lost === 1 ? '1 lost sample' : s.lost + ' lost samples';
      return (
        head +
        'min ' +
        Math.round(s.min) +
        ' ms, median ' +
        Math.round(s.median) +
        ' ms, max ' +
        Math.round(s.max) +
        ' ms, ' +
        lost +
        '.'
      );
    },
    sparkBars(values) {
      const s = this.sparkStats(values);
      if (!s) return [];
      const span = s.max - s.min || 1;
      const recent = values.slice(-64);
      return recent.map((n, i) => {
        const x = 128 - (recent.length - i) * 2;
        if (!isNumber(n)) return { i, x, y: 0, w: 1.4, h: 24, lost: true };
        const h = 5 + (19 * (n - s.min)) / span;
        return { i, x, y: 24 - h, w: 1.4, h, lost: false };
      });
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
      this.hover = { x, at };
    },
    highlight(id) {
      if (this.selectedTunnel === id) return;
      this.selectedTunnel = id;
      if (!this.allMode) this.loadHistory();
    },
    isSelected(series) {
      return series.id === this.selectedTunnel;
    },
    isDimmed(series) {
      return (
        series.id !== 'wan' &&
        !this.isSelected(series) &&
        this.history.some((s) => this.isSelected(s))
      );
    },
    hasBand(series) {
      return this.isSelected(series) || series.id === 'wan';
    },
    seriesColor(series) {
      if (series.id === 'wan') return 'var(--vph-muted)';
      if (this.isSelected(series)) return 'var(--vph-brand)';
      const index = this.state.tunnels.findIndex((t) => t.id === series.id);
      return palette[(index < 0 ? 0 : index) % palette.length];
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
.vph-chart-footer .vph-muted {
  margin-left: 12px;
}
.vph-heading {
  margin: 22px 0 28px;
}
.vph-heading p,
.vph-muted,
.vph small {
  color: var(--vph-muted);
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
  font-weight: 500;
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
  padding: 12px 16px;
  border-bottom: 1px solid var(--vph-border);
  text-align: left;
}
.vph td {
  font-size: 13px;
  height: 56px;
  padding: 8px 16px;
  border-bottom: 1px solid var(--vph-border);
  vertical-align: middle;
}
.vph tr:last-child td {
  border-bottom: 0;
}
.vph tbody tr:hover {
  background: color-mix(in srgb, var(--vph-muted) 4%, var(--vph-card));
}
.vph tr.selected {
  background: color-mix(in srgb, var(--vph-brand) 6%, var(--vph-card));
}
.vph tr.vph-active td:first-child {
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
.vph-name-line {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}
.vph-state-dot {
  flex: none;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--vph-muted);
}
.vph-state-connected {
  background: var(--vph-brand);
}
.vph-state-offline {
  background: #d69e2e;
}
.vph-badge {
  flex: none;
  display: inline-block;
  border-radius: 4px;
  font-size: 11px;
  line-height: 16px;
  white-space: nowrap;
  color: var(--vph-brand-text);
  background: color-mix(in srgb, var(--vph-brand) 12%, var(--vph-card));
  padding: 1px 8px;
}
.vph-status {
  font-size: 12px;
  margin-top: 4px;
}
.vph-metric {
  min-width: 128px;
}
.vph-metric .vph-number {
  display: block;
}
.vph-loss {
  display: block;
  margin-top: 2px;
  font-size: 12px;
  font-variant-numeric: tabular-nums;
  color: var(--vph-danger-text);
}
.vph-bar {
  display: block;
  width: 96px;
  height: 4px;
  margin-top: 6px;
  border-radius: 2px;
  background: color-mix(in srgb, var(--vph-muted) 14%, var(--vph-card));
  overflow: hidden;
}
.vph-bar-fill {
  display: block;
  height: 100%;
  border-radius: 2px;
  background: color-mix(in srgb, var(--vph-muted) 45%, var(--vph-card));
}
.vph-bar-best {
  background: var(--vph-brand);
}
.vph-action {
  min-width: 232px;
  text-align: right;
}
.vph-actions {
  display: flex;
  flex-wrap: wrap;
  justify-content: flex-end;
  align-items: center;
  gap: 8px;
}
.vph-confirm {
  display: flex;
  flex-wrap: wrap;
  justify-content: flex-end;
  align-items: center;
  gap: 8px 12px;
}
.vph-confirm-text {
  flex-basis: 100%;
  font-size: 12px;
  color: var(--vph-muted);
}
.vph .vph-control {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-height: 32px;
  padding: 0 12px;
  font-size: 13px;
  border: 1px solid var(--vph-border);
  border-radius: 5px;
  background: var(--vph-card);
  color: var(--vph-text);
  white-space: nowrap;
}
.vph .vph-switch {
  position: relative;
  flex: none;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 36px;
  height: 32px;
  padding: 0;
  border: 0;
  background: none;
}
.vph-track {
  position: relative;
  display: block;
  width: 36px;
  height: 20px;
  border-radius: 10px;
  background: color-mix(in srgb, var(--vph-muted) 35%, var(--vph-card));
  transition: background-color 0.18s ease;
}
.vph-track::after {
  content: '';
  position: absolute;
  top: 2px;
  left: 2px;
  width: 16px;
  height: 16px;
  border-radius: 50%;
  background: var(--vph-card);
  transition: transform 0.18s ease;
}
.vph-switch[aria-checked='true'] .vph-track {
  background: var(--vph-brand);
}
.vph-switch[aria-checked='true'] .vph-track::after {
  transform: translateX(16px);
}
.vph-busy {
  font-size: 12px;
  white-space: nowrap;
  line-height: 32px;
}
.vph .vph-note {
  font-size: 12px;
  margin: 12px 0 16px;
}
.vph-spark {
  width: 128px;
  height: 24px;
  display: block;
}
.vph-spark rect {
  fill: var(--vph-muted);
  opacity: 0.55;
}
.vph-spark .vph-spark-lost {
  fill: var(--vph-danger);
  opacity: 0.35;
}
.vph-dot {
  fill: var(--series, var(--vph-brand));
}
.vph-wan-series .vph-dot {
  fill: var(--vph-muted);
}
.vph-egress {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px 14px;
  margin: 16px 0 0;
}
.vph-egress-error {
  color: var(--vph-danger-text);
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
  stroke: var(--series, var(--vph-brand));
  stroke-width: 1.8;
  fill: none;
  stroke-linejoin: round;
}
.vph-series-selected .vph-line {
  stroke-width: 2.8;
}
.vph-series-dim {
  opacity: 0.55;
}
.vph-band {
  fill: var(--series, var(--vph-brand));
  opacity: 0.14;
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
  width: 268px;
  white-space: nowrap;
  padding: 10px 12px;
  background: var(--vph-card);
  border: 1px solid var(--vph-border);
  border-radius: 5px;
  font-size: 12px;
  pointer-events: none;
}
.vph-tip-selected {
  color: var(--vph-text);
  font-weight: 600;
}
.vph-chart-footer {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 6px 16px;
  font-size: 12px;
  margin-top: 5px;
}
.vph-legend {
  display: flex;
  flex-wrap: wrap;
  gap: 2px 8px;
  margin: 0;
  padding: 0;
  list-style: none;
}
.vph-legend-item {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  min-height: 32px;
  border: 0;
  border-radius: 4px;
  background: none;
  padding: 4px 8px;
  color: var(--vph-text);
  font: inherit;
  font-size: 12px;
  text-align: left;
}
button.vph-legend-item {
  cursor: pointer;
}
.vph-legend-item.active {
  background: color-mix(in srgb, var(--vph-brand) 10%, var(--vph-card));
  color: var(--vph-brand-text);
}
.vph-legend-item .vph-muted {
  margin-left: 6px;
}
.vph-swatch {
  display: inline-block;
  width: 15px;
  height: 3px;
  vertical-align: middle;
  background: var(--series, var(--vph-brand));
  margin-right: 6px;
  border-radius: 2px;
}
.vph-legend-item .vph-swatch {
  margin-right: 4px;
}
.vph-swatch-wan {
  height: 0;
  background: none;
  border-top: 2px dashed var(--vph-muted);
}
.vph-tip-swatch {
  width: 10px;
  height: 10px;
  border-radius: 50%;
  margin-right: 6px;
}
.vph-tip-swatch.vph-swatch-wan {
  height: 0;
  width: 12px;
  border-radius: 0;
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
  .vph-button,
  .vph-tabs button,
  .vph-ranges button {
    min-height: 44px;
  }
  .vph-link {
    min-height: 44px;
  }
  .vph-info {
    padding: 12px;
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
  .vph-chart-footer {
    flex-direction: column;
    align-items: flex-start;
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
@media (max-width: 639px) {
  .vph thead {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
  }
  .vph-spark-column {
    display: none;
  }
  .vph tbody tr {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 16px;
    padding: 12px 16px;
    border-bottom: 1px solid var(--vph-border);
  }
  .vph tbody tr:last-child {
    border-bottom: 0;
  }
  .vph td {
    height: auto;
    padding: 0;
    border-bottom: 0;
  }
  .vph .vph-tunnel {
    flex: 1 0 100%;
    min-width: 0;
  }
  .vph td[data-label]::before {
    content: attr(data-label);
    margin-right: 6px;
    font-size: 12px;
    color: var(--vph-muted);
  }
  .vph-metric {
    min-width: 0;
  }
  .vph-metric .vph-bar {
    width: 72px;
  }
  .vph-loss {
    margin-left: 0;
  }
  .vph .vph-action {
    min-width: 0;
    margin-left: auto;
  }
  .vph .vph-action.vph-confirming {
    flex: 1 0 100%;
  }
}
@media (pointer: coarse) {
  .vph .vph-control,
  .vph .vph-legend-item {
    min-height: 44px;
  }
  .vph .vph-switch {
    width: 44px;
    height: 44px;
  }
}
@media (prefers-reduced-motion: reduce) {
  .vph-track,
  .vph-track::after {
    transition: none;
  }
}
</style>
