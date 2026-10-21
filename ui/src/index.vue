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
      <update-panel
        compact
        :info="updateState"
        :disabled="updatesDisabled"
        :waiting="updateWaiting"
        @check="checkUpdates"
        @install="installUpdate"
        @reload="reloadPage"
      />
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
      <update-panel
        compact
        :info="updateState"
        :disabled="updatesDisabled"
        :waiting="updateWaiting"
        @check="checkUpdates"
        @install="installUpdate"
        @reload="reloadPage"
      />
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
    <section v-if="ready" class="vph-panel vph-hero" aria-label="Exit addresses">
      <div class="vph-hero-grid">
        <div
          v-for="(c, i) in heroColumns"
          :key="c.label"
          class="vph-hero-column"
          :class="i ? 'vph-hero-secondary' : 'vph-hero-primary'"
        >
          <p class="vph-hero-label vph-muted">{{ c.label }}</p>
          <p v-if="c.ip" class="vph-hero-main">
            <span v-if="c.flag" class="vph-flag vph-hero-flag" aria-hidden="true">{{
              c.flag
            }}</span>
            <span class="vph-hero-ip">{{ c.ip }}</span>
          </p>
          <p v-else class="vph-hero-main vph-hero-pending vph-muted" :title="c.error || null">
            {{ c.error ? 'Exit address unavailable' : 'Checking exit address…' }}
          </p>
          <p class="vph-hero-route">{{ c.route }}</p>
          <p v-if="c.latency !== null" class="vph-hero-latency vph-number">
            {{ latency(c.latency) }} ms to {{ selectedTarget }}
          </p>
        </div>
      </div>
      <div v-if="comparison" class="vph-hero-compare">
        <p class="vph-hero-sentence">{{ comparison.sentence }}</p>
        <div class="vph-bars" role="img" :aria-label="comparison.label">
          <div v-for="b in comparison.bars" :key="b.name" class="vph-bar-row">
            <span class="vph-bar-name vph-muted">{{ b.name }}</span>
            <span class="vph-bar-track"
              ><span class="vph-bar-fill" :class="b.cls" :style="{ width: b.width }"></span
            ></span>
            <span class="vph-bar-value vph-number">{{ b.text }}</span>
          </div>
        </div>
      </div>
      <p class="vph-hero-updated vph-muted">
        {{
          egressCheckedAt
            ? 'Exit addresses checked ' + ago(egressCheckedAt * 1000)
            : 'Checking exit addresses…'
        }}
      </p>
    </section>
    <p class="vph-statusline" role="status">{{ statusLine }}</p>
    <p v-if="!can.configure && ready" class="vph-muted vph-hint">{{ configureReason }}</p>
    <div role="alert">
      <div v-if="!ready && readError" class="vph-panel vph-fail">
        <h2>Wayline could not read the tunnel state.</h2>
        <p class="vph-fail-text">{{ readError }}</p>
        <p class="vph-muted">Run wayline-ctl doctor on the router and send the output.</p>
        <button type="button" class="vph-button" :disabled="loading" @click="retry">Retry</button>
        <update-panel
          compact
          :info="updateState"
          :disabled="updatesDisabled"
          :waiting="updateWaiting"
          @check="checkUpdates"
          @install="installUpdate"
          @reload="reloadPage"
        />
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
      <section class="vph-set" aria-labelledby="vph-set-monitoring">
        <h3 id="vph-set-monitoring">Monitoring</h3>
        <div class="vph-row">
          <label class="vph-check"
            ><input ref="firstField" type="checkbox" v-model="draft.enabled" /> Measure
            latency</label
          >
          <label class="vph-inline"
            >Measure every
            <select v-model.number="draft.interval">
              <option :value="15">15 seconds</option>
              <option :value="30">30 seconds</option>
              <option :value="60">60 seconds</option>
            </select></label
          >
        </div>
      </section>
      <section class="vph-set" aria-labelledby="vph-set-destinations">
        <h3 id="vph-set-destinations">Destinations to ping</h3>
        <p class="vph-muted">
          The primary destination is used to compare tunnels and to switch the exit.
        </p>
        <ol class="vph-dests">
          <li v-for="(target, i) in draft.targets" :key="i" class="vph-dest">
            <div class="vph-dest-field">
              <input
                v-model.trim="draft.targets[i]"
                required
                maxlength="253"
                autocomplete="off"
                autocapitalize="off"
                spellcheck="false"
                :placeholder="i ? 'IP address or host name' : '1.1.1.1'"
                :aria-label="i ? 'Destination ' + (i + 1) : 'Primary destination'"
              />
              <small v-if="resolvedText(target)">{{ resolvedText(target) }}</small>
            </div>
            <div class="vph-dest-actions">
              <span v-if="!i" class="vph-badge">Primary</span>
              <template v-else>
                <button
                  type="button"
                  class="vph-link"
                  :aria-label="'Make destination ' + (i + 1) + ' primary'"
                  @click="makePrimary(i)"
                >
                  Make primary
                </button>
                <button
                  type="button"
                  class="vph-link"
                  :aria-label="'Remove destination ' + (i + 1)"
                  @click="draft.targets.splice(i, 1)"
                >
                  Remove
                </button>
              </template>
            </div>
          </li>
        </ol>
        <button
          type="button"
          class="vph-link"
          :disabled="draft.targets.length >= 5"
          @click="draft.targets.push('')"
        >
          Add destination
        </button>
      </section>
      <section class="vph-set" aria-labelledby="vph-set-tunnels">
        <h3 id="vph-set-tunnels">Tunnels to monitor</h3>
        <div class="vph-tunnels">
          <label v-for="t in state.tunnels" :key="t.id" class="vph-check"
            ><input
              type="checkbox"
              :value="t.monitorId"
              v-model="draft.tunnels"
              @change="pruneAutomaticTunnels"
            />{{ t.name }}</label
          >
        </div>
      </section>
      <section class="vph-set" aria-labelledby="vph-set-also">
        <h3 id="vph-set-also">Also measure</h3>
        <label class="vph-check"
          ><input type="checkbox" v-model="draft.directWan" /> Direct internet without VPN (every
          uplink)</label
        >
        <p class="vph-muted">Probes bypass the VPN. Where a probe cannot, the monitor says so.</p>
        <label class="vph-check"
          ><input type="checkbox" v-model="draft.endpointDiagnostics" /> VPN servers, over
          WAN</label
        >
        <p class="vph-muted">
          The public address of each monitored server. It shows the route, not latency through the
          tunnel.
        </p>
      </section>
      <section class="vph-set" aria-labelledby="vph-set-exit">
        <h3 id="vph-set-exit">Exit</h3>
        <label class="vph-check"
          ><input type="checkbox" v-model="draft.autoExit" /> Switch the exit automatically</label
        >
        <p class="vph-muted">
          Moves to a clearly faster healthy tunnel (20% and 10 ms better, at most every 5 minutes)
          or away from an unhealthy one after 1 minute.
        </p>
        <fieldset v-if="draft.autoExit" class="vph-tunnels">
          <legend>Tunnels allowed for automatic switching</legend>
          <label v-for="t in state.tunnels" :key="t.id" class="vph-check">
            <input
              type="checkbox"
              :value="t.monitorId"
              v-model="draft.autoTunnels"
              :disabled="!draft.tunnels.includes(t.monitorId)"
            />
            {{ t.name }}
          </label>
        </fieldset>
        <p v-if="draft.autoExit" class="vph-muted">
          Choose at least two monitored tunnels. Only healthy, connected members can become the
          exit. Automatic switching pauses when the current exit is outside this pool. Stopped
          tunnels stay stopped.
        </p>
        <p v-if="draft.autoExit" class="vph-warning" role="note">
          Warning: this changes the native VPN priority by itself. Existing connections do not move.
        </p>
      </section>
      <section class="vph-set" aria-labelledby="vph-set-history">
        <h3 id="vph-set-history">History</h3>
        <select v-model.number="draft.historyHours" aria-label="Keep history">
          <option :value="24">Keep 24 hours on the router</option>
          <option :value="0">Keep in memory only</option>
        </select>
        <p class="vph-muted">The monitor saves history in batches to limit flash writes.</p>
      </section>
      <section class="vph-set" aria-labelledby="vph-set-updates">
        <h3 id="vph-set-updates">Updates</h3>
        <label class="vph-check"
          ><input type="checkbox" v-model="draft.autoUpdates" /> Install updates
          automatically</label
        >
        <p class="vph-muted">
          Checks once a day and installs the newest signed release, including alpha and beta
          releases.
        </p>
        <update-panel
          :info="updateState"
          :disabled="updatesDisabled"
          :waiting="updateWaiting"
          @check="checkUpdates"
          @install="installUpdate"
          @reload="reloadPage"
        />
      </section>
      <footer>
        <button type="button" class="vph-button" @click="closeSettings">Cancel</button
        ><button class="vph-button vph-primary" :disabled="busy">
          {{ busy ? 'Saving…' : 'Save settings' }}
        </button>
      </footer>
    </form>

    <p v-if="!can.select && selectReason" class="vph-muted vph-hint">{{ selectReason }}</p>
    <p v-if="state.monitoringIssue" class="vph-notice" role="alert">{{ state.monitoringIssue }}</p>
    <div v-if="state.auto && state.auto.enabled" class="vph-panel vph-info">
      <strong>Automatic exit switching is on.</strong>
      <p v-if="state.auto.needsAttention" role="alert">
        Automatic switching stopped after an uncertain request. Check the native VPN state. Disable
        automatic switching. Save Settings. Enable it again.
      </p>
      <p v-else-if="state.auto.paused">
        Automatic switching is paused because the current exit is outside the allowed pool.
      </p>
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
    <div class="vph-table-head">
      <div v-if="destinations.length > 1" class="vph-ranges" role="group" aria-label="Destination">
        <button
          v-for="d in destinations"
          :key="d"
          :class="{ active: selectedTarget === d }"
          :aria-pressed="selectedTarget === d ? 'true' : 'false'"
          @click="chooseTarget(d)"
        >
          {{ d }}
        </button>
      </div>
      <p class="vph-muted" :title="state.scope || null">
        Measured every {{ state.config.interval }} seconds.
      </p>
    </div>
    <div
      class="vph-panel vph-table-wrap"
      tabindex="0"
      role="region"
      aria-label="Exit tunnels table"
    >
      <table class="vph-exits">
        <thead>
          <tr>
            <th scope="col">Tunnel</th>
            <th scope="col" class="vph-col-through">Through tunnel</th>
            <th scope="col" class="vph-col-server" v-if="showServer">To server</th>
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
                  :title="t.name"
                  :aria-pressed="selectedTunnel === t.id ? 'true' : 'false'"
                  :aria-label="'Highlight ' + t.name + ' in the latency graph'"
                  @click="highlight(t.id)"
                >
                  {{ t.name }}</button
                ><span v-if="t.preferred" class="vph-badge">Active exit</span
                ><span v-if="changeFor(t.id)" class="vph-tag" :title="changeFor(t.id).text"
                  >Changed</span
                >
              </div>
              <div v-if="exitIdentity(t.id) && !isExit(t)" class="vph-muted vph-status">
                Exit IP {{ exitIdentity(t.id).ip
                }}<template v-if="exitIdentity(t.id).place.name"
                  >,
                  <span class="vph-flag" aria-hidden="true">{{
                    exitIdentity(t.id).place.flag
                  }}</span>
                  {{ exitIdentity(t.id).place.name }}</template
                >
              </div>
              <div v-else class="vph-muted vph-status">{{ t.stateLabel }}</div>
              <div v-if="rowNote(t)" class="vph-muted vph-status">{{ rowNote(t) }}</div>
            </td>
            <td class="vph-metric" data-label="Through tunnel">
              <template v-if="primarySelected">
                <span class="vph-number vph-latency"
                  >{{ latency(t.median) }}<small v-if="hasMs(t.median)"> ms</small></span
                >
                <span
                  v-if="latencyNotes[t.id]"
                  class="vph-latency-note"
                  :class="latencyNotes[t.id].cls"
                  :title="latencyNotes[t.id].title"
                  >{{ latencyNotes[t.id].text }}</span
                >
              </template>
              <span v-else-if="diagnosticValue(t.id) !== null" class="vph-number vph-latency"
                >{{ latency(diagnosticValue(t.id)) }}<small> ms</small></span
              >
              <span v-else class="vph-muted">No data yet</span>
            </td>
            <td v-if="showServer" class="vph-server" data-label="To server" :title="serverTitle(t)">
              <template v-if="serverFor(t) && serverFor(t).address">
                <span class="vph-number vph-server-ms">{{ serverMs(serverFor(t)) }}</span>
                <span class="vph-status vph-muted vph-server-address"
                  >Server {{ serverFor(t).address }}</span
                >
              </template>
              <span v-else class="vph-muted">—</span>
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
                  {{ busyTunnel === t.id && busyAction === 'select' ? 'Applying…' : 'Use as exit' }}
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
          <tr v-for="u in uplinkRows" :key="u.id" class="vph-uplink-row">
            <td class="vph-tunnel">
              <div class="vph-name-line">
                <span class="vph-name vph-wan-name">{{ uplinkName(u) }}</span
                ><span v-if="u.active" class="vph-badge">Active uplink</span
                ><span v-if="changeFor(u.id)" class="vph-tag" :title="changeFor(u.id).text"
                  >Changed</span
                >
              </div>
              <div v-if="!u.active && u.address" class="vph-muted vph-status">
                Address {{ u.address }}
              </div>
            </td>
            <td class="vph-metric">
              <span
                v-if="!u.active && unmeasurable(u)"
                class="vph-latency-text"
                title="The router sends this traffic through the active uplink."
                >Not measurable</span
              >
              <span v-else-if="uplinkValue(u) !== null" class="vph-number vph-latency"
                ><span class="vph-sr">Without VPN: </span>{{ latency(uplinkValue(u))
                }}<small> ms</small></span
              >
              <span v-else class="vph-muted">No data yet</span>
            </td>
            <td v-if="showServer"></td>
            <td class="vph-action"></td>
          </tr>
        </tbody>
      </table>
      <div v-if="!state.tunnels.length" class="vph-empty" role="status">
        {{ emptyText }}
      </div>
    </div>
    <p v-if="uplinkRows.length && !wanAvailable" class="vph-muted vph-note" role="note">
      {{ wanNote }}
    </p>
    <p class="vph-muted vph-note">
      Changing the exit affects new connections. Stopping a tunnel may send traffic over WAN.
    </p>

    <section class="vph-panel vph-history">
      <header class="vph-chart-heading">
        <div>
          <h2>Latency to {{ selectedTarget }}</h2>
          <p class="vph-muted">
            {{ range > 360 ? 'Five-minute' : 'One-minute' }} medians. Gaps mean no measurement. Drag
            across the graph to zoom.
          </p>
        </div>
        <div class="vph-chart-controls">
          <div class="vph-ranges" role="group" aria-label="Move through history">
            <button
              type="button"
              class="vph-step"
              aria-label="Earlier"
              :disabled="offset + range >= 1440"
              @click="shift(1)"
            >
              ‹
            </button>
            <button
              type="button"
              class="vph-step"
              aria-label="Later"
              :disabled="offset === 0"
              @click="shift(-1)"
            >
              ›
            </button>
            <button v-if="offset > 0" type="button" @click="goNow">Now</button>
          </div>
          <div class="vph-ranges" role="group" aria-label="History range">
            <button
              v-for="r in ranges"
              :key="r.minutes"
              type="button"
              :class="{ active: range === r.minutes }"
              :aria-pressed="range === r.minutes"
              @click="choosePreset(r.minutes)"
            >
              {{ r.label }}
            </button>
          </div>
        </div>
      </header>
      <div class="vph-chart" ref="chart" @mouseleave="hover = null">
        <svg
          :viewBox="'0 0 ' + chartWidth + ' 236'"
          role="img"
          tabindex="0"
          :aria-label="
            'Latency to ' +
            selectedTarget +
            ' through each tunnel. Arrow keys pan, Escape returns to now.'
          "
          @pointerdown="pointerDown"
          @pointermove="pointerMove"
          @pointerup="pointerUp"
          @pointercancel="drag = null"
          @dblclick="choosePreset(preset)"
          @keydown="keyNav"
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
              'vph-uplink-series': isUplink(series),
              'vph-endpoint-series': isServer(series),
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
          <g
            v-for="(m, i) in markerList"
            :key="i + ':' + m.at"
            class="vph-marker"
            role="img"
            tabindex="0"
            :aria-label="m.text"
            @mouseenter="showMarker(m)"
            @focus="showMarker(m)"
            @blur="hover = null"
          >
            <line :x1="chartX(m.at)" :x2="chartX(m.at)" y1="24" y2="200" class="vph-marker-line" />
            <circle :cx="chartX(m.at)" cy="24" r="3" class="vph-marker-dot" />
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
          <rect
            v-if="selection"
            class="vph-select"
            :x="selection.x"
            y="24"
            :width="selection.width"
            height="176"
          />
          <line v-if="hover" :x1="hover.x" :x2="hover.x" y1="24" y2="200" class="vph-guide" />
        </svg>
        <div v-if="chartNote" class="vph-chart-note" role="status">{{ chartNote }}</div>
        <div v-if="hover" role="status" class="vph-tooltip" :style="tipStyle">
          <strong>{{ clock(hover.at) }}</strong>
          <div v-if="hover.marker" class="vph-tip-event">{{ hover.marker }}</div>
          <div v-for="row in hoverRows" :key="row.id" :class="{ 'vph-tip-selected': row.selected }">
            <span
              class="vph-swatch vph-tip-swatch"
              :class="{ 'vph-swatch-wan': row.wan }"
              :style="{ '--series': row.color }"
              aria-hidden="true"
            ></span
            >{{ row.name }}: {{ row.text }}
          </div>
        </div>
      </div>
      <div class="vph-chart-footer">
        <div v-if="legendItems.length" class="vph-legend-block">
          <ul class="vph-key" aria-label="Line styles">
            <li>
              <svg class="vph-key-line" width="24" height="8" viewBox="0 0 24 8" aria-hidden="true">
                <line x1="0" y1="4" x2="24" y2="4" />
              </svg>
              Through the tunnel
            </li>
            <li v-if="serverShown">
              <svg class="vph-key-line" width="24" height="8" viewBox="0 0 24 8" aria-hidden="true">
                <line x1="0" y1="4" x2="24" y2="4" class="vph-key-dashed" />
              </svg>
              To its VPN server, over WAN
            </li>
          </ul>
          <ul class="vph-legend" aria-label="Tunnels in the graph">
            <li v-for="item in legendItems" :key="item.id">
              <button
                type="button"
                class="vph-legend-item"
                :class="{
                  active: isolated.includes(item.owner),
                  dimmed: isolated.length && !isolated.includes(item.owner),
                }"
                :aria-pressed="isolated.includes(item.owner) ? 'true' : 'false'"
                @click="isolate(item, $event)"
              >
                <span
                  v-if="item.tunnel"
                  class="vph-legend-dot"
                  :style="{ '--series': item.color }"
                  aria-hidden="true"
                ></span
                ><span v-else class="vph-swatch vph-swatch-wan" aria-hidden="true"></span
                >{{ item.name }}<span v-if="item.preferred" class="vph-muted"> Active exit</span
                ><span v-if="item.stored" class="vph-muted"> Stored</span>
              </button>
            </li>
          </ul>
          <p class="vph-muted vph-legend-hint">
            {{
              isolated.length
                ? 'Showing selected lines. Click a selected line again to show all.'
                : 'Click a line to show only it. Ctrl- or Cmd-click to add more.'
            }}
          </p>
        </div>
        <span class="vph-muted vph-range-label">{{
          clock(windowStart) + ' – ' + clock(windowStart + range * 60000)
        }}</span>
      </div>
    </section>

    <section v-if="recentEvents.length" class="vph-panel vph-changes">
      <h2>Recent changes</h2>
      <ul>
        <li v-for="(e, i) in recentEvents" :key="i">
          <time class="vph-muted" :title="new Date(e.at).toLocaleString()">{{ ago(e.at) }}</time>
          <span>{{ e.text }}</span>
        </li>
      </ul>
    </section>
    <footer class="vph-footnote">
      <a href="https://github.com/epk/wayline" target="_blank" rel="noopener noreferrer">Wayline</a>
      <a
        :href="'https://github.com/epk/wayline/releases/tag/v' + footerVersion"
        target="_blank"
        rel="noopener noreferrer"
        >{{ footerVersion }}</a
      >
    </footer>
  </main>
</template>

<script>
import UpdatePanel from './update-panel.vue';
const defaults = () => ({
  enabled: false,
  interval: 15,
  tunnels: [],
  targets: ['1.1.1.1', '8.8.8.8', '9.9.9.9'],
  directWan: false,
  historyHours: 24,
  autoUpdates: false,
  autoExit: false,
  autoTunnels: [],
  endpointDiagnostics: false,
});
const validTarget = (value) =>
  typeof value === 'string' &&
  value.length > 0 &&
  value.length <= 253 &&
  /^[A-Za-z0-9.-]+$/.test(value);
const isNumber = (value) => typeof value === 'number' && Number.isFinite(value);
const settingsFrom = (source) => {
  const settings = defaults();
  Object.keys(settings).forEach((key) => {
    if (source[key] !== undefined)
      settings[key] = Array.isArray(source[key]) ? source[key].slice() : source[key];
  });
  return settings;
};
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
const endpointPrefix = 'endpoint:';
const uplinkPrefix = 'uplink:';
const changeWindow = 60 * 60000;
const updatePhases = ['checking', 'downloading', 'verifying', 'installing'];
const targetKey = 'wayline.target';
const storage = {
  get(key) {
    try {
      return window.localStorage.getItem(key) || '';
    } catch {
      return '';
    }
  },
  set(key, value) {
    try {
      window.localStorage.setItem(key, value);
    } catch {
      return;
    }
  },
};
const validCountry = (code) => typeof code === 'string' && /^[A-Z]{2}$/.test(code) && code !== 'XX';
const flagOf = (code) =>
  String.fromCodePoint(...[...code].map((c) => 0x1f1e6 + c.charCodeAt(0) - 65));
const countryName = (code) => {
  try {
    return new Intl.DisplayNames(['en'], { type: 'region' }).of(code) || code;
  } catch {
    return code;
  }
};
export default {
  name: 'WaylineDashboard',
  components: { UpdatePanel },
  props: { api: Object, preview: Boolean },
  data() {
    return {
      state: {
        config: defaults(),
        tunnels: [],
        uplinks: [],
        events: [],
        capabilities: { configure: false, select: false, control: false },
        compat: { level: 'full', firmware: '', message: '' },
        scope: '',
        revision: '',
        auto: { enabled: false, last: null },
        endpoints: [],
        resolved: {},
        wan: { available: true, interface: null, reason: '' },
      },
      ready: false,
      updateError: '',
      updateWaiting: false,
      updateTimer: null,
      updatePolling: false,
      updateLostSince: 0,
      pendingStop: '',
      busyAction: '',
      noticeTimer: null,
      draft: defaults(),
      selectedTunnel: '',
      isolated: [],
      selectedTarget: storage.get(targetKey) || '1.1.1.1',
      settingsOpen: false,
      loading: false,
      readError: '',
      historyError: '',
      busy: false,
      busyTunnel: '',
      error: '',
      notice: '',
      history: [],
      historyMarkers: [],
      historyAt: 0,
      range: 60,
      preset: 60,
      offset: 0,
      drag: null,
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
    tipStyle() {
      const x = this.hover ? this.hover.x : 0;
      return x > this.chartWidth / 2
        ? { left: x - 12 + 'px', transform: 'translateX(-100%)' }
        : { left: x + 12 + 'px' };
    },
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
    updateState() {
      const u = this.state.update || {};
      return {
        current: u.current || this.state.version || '',
        latest: u.latest || null,
        available: !!u.available,
        phase: u.phase || 'idle',
        message: this.updateError || u.message || '',
        failed: !!this.updateError || !!u.failed,
        checkedAt: u.checkedAt || 0,
      };
    },
    updateActive() {
      return updatePhases.includes((this.state.update || {}).phase);
    },
    updatesDisabled() {
      return (this.state.capabilities || {}).update === false;
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
    activeMedian() {
      const active = this.state.tunnels.find((t) => t.preferred);
      return active && isNumber(active.median) ? active.median : null;
    },
    comparable() {
      return this.state.tunnels.filter((t) => t.connected && isNumber(t.median)).length >= 2;
    },
    primary() {
      return this.state.config.targets[0];
    },
    primarySelected() {
      return this.selectedTarget === this.primary;
    },
    selection() {
      if (!this.drag) return null;
      return {
        x: Math.min(this.drag.from, this.drag.to),
        width: Math.abs(this.drag.to - this.drag.from),
      };
    },
    exitLatency() {
      const t = this.exitTunnel;
      if (!t) return this.directLatency;
      if (this.primarySelected) return isNumber(t.median) ? t.median : null;
      return this.diagnosticValue(t.id);
    },
    directLatency() {
      return this.diagnosticValue('wan');
    },
    showWan() {
      return !!(
        this.state.config.directWan &&
        this.exitTunnel &&
        this.wanEgress &&
        (this.wanEgress.ip || this.wanEgress.error)
      );
    },
    comparison() {
      if (!this.exitTunnel) return null;
      const vpn = this.exitLatency;
      const direct = this.directLatency;
      if (!isNumber(vpn) || !isNumber(direct)) return null;
      const diff = vpn - direct;
      const scale = Math.max(vpn, direct) || 1;
      let sentence = 'About the same as going direct.';
      if (diff >= 1) sentence = 'The VPN adds ' + diff.toFixed(2) + ' ms.';
      else if (diff <= -1) sentence = 'The VPN is ' + (-diff).toFixed(2) + ' ms faster.';
      return {
        sentence,
        bars: [
          { name: 'VPN', cls: 'vph-bar-vpn', width: (vpn / scale) * 100 + '%', text: this.ms(vpn) },
          { name: 'Direct', cls: '', width: (direct / scale) * 100 + '%', text: this.ms(direct) },
        ],
        label: 'VPN ' + this.ms(vpn) + ', direct ' + this.ms(direct),
      };
    },
    exitTunnel() {
      const t = this.state.tunnels.find((item) => item.preferred);
      return t && t.enabled && t.connected ? t : null;
    },
    heroEgress() {
      return this.exitTunnel ? this.egressFor(this.exitTunnel.id) : this.wanEgress;
    },
    heroColumns() {
      const route = (place, via) => (place.name ? place.name + ', through ' : 'Through ') + via;
      const columns = [
        {
          label: 'Current exit',
          egress: this.heroEgress,
          place: this.heroPlace,
          route: route(
            this.heroPlace,
            this.exitTunnel ? this.exitTunnel.name : this.wanInterface + ' without VPN',
          ),
          latency: this.exitLatency,
        },
      ];
      if (this.showWan) {
        columns.push({
          label: 'Without VPN',
          egress: this.wanEgress,
          place: this.wanPlace,
          route: route(this.wanPlace, this.wanInterface),
          latency: this.directLatency,
        });
      }
      return columns.map((c) => ({
        label: c.label,
        ip: c.egress && !c.egress.error ? c.egress.ip : '',
        flag: c.place.flag,
        error: c.egress && c.egress.error,
        route: c.route,
        latency: c.latency,
      }));
    },
    heroPlace() {
      return this.place(this.heroEgress && this.heroEgress.location);
    },
    wanPlace() {
      return this.place(this.wanEgress && this.wanEgress.location);
    },
    wanInterface() {
      const active = (this.state.uplinks || []).find((u) => u.active);
      const name = (active && active.interface) || (this.wanEgress && this.wanEgress.interface);
      return name || 'WAN';
    },
    egressCheckedAt() {
      return (this.state.egress && this.state.egress.checkedAt) || 0;
    },
    latencyNotes() {
      const notes = {};
      this.state.tunnels.forEach((t) => {
        notes[t.id] = this.latencyNote(t);
      });
      return notes;
    },
    endpoints() {
      return this.state.config.endpointDiagnostics && Array.isArray(this.state.endpoints)
        ? this.state.endpoints
        : [];
    },
    wanAvailable() {
      return !!(this.state.wan && this.state.wan.available);
    },
    wanNote() {
      const reason = (this.state.wan || {}).reason;
      return 'Direct WAN is unavailable. ' + (reason || 'The router did not give a reason.');
    },
    uplinkRows() {
      if (!this.state.config.directWan) return [];
      return (this.state.uplinks || []).slice().sort((a, b) => Number(b.active) - Number(a.active));
    },
    showServer() {
      return !!this.state.config.endpointDiagnostics;
    },
    bundleVersion() {
      return WAYLINE_VERSION;
    },
    footerVersion() {
      return this.state.version || this.bundleVersion;
    },
    staleBundle() {
      return Boolean(this.state.version) && this.state.version !== WAYLINE_VERSION;
    },
    wanEgress() {
      return this.egressFor('wan');
    },
    emptyText() {
      if (!this.ready && !this.readError) return 'Reading tunnel state…';
      if (this.readError) return 'Tunnel state could not be read.';
      return 'No configured VPN tunnels available.';
    },
    chartNote() {
      if (this.historyError) return this.historyError;
      if (!this.selectedTunnel) return 'No tunnels to show.';
      if (
        !this.history.length ||
        this.history.every((s) => s.points.every((p) => !isNumber(p.median)))
      )
        return this.historyLoaded ? 'No measurements in this range.' : 'Loading history…';
      return '';
    },
    destinations() {
      return this.state.config.targets;
    },
    windowStart() {
      return this.anchor - (this.offset + this.range) * 60000;
    },
    yMax() {
      let n = 25;
      this.visibleHistory.forEach((s) =>
        s.points.forEach((p) => {
          if (isNumber(p.max)) n = Math.max(n, p.max);
          if (isNumber(p.median)) n = Math.max(n, p.median);
        }),
      );
      return Math.ceil((n * 1.15) / 25) * 25;
    },
    endpointHistory() {
      return !!this.state.config.endpointDiagnostics;
    },
    visibleHistory() {
      if (!this.isolated.length) return this.history;
      return this.history.filter((s) => this.isolated.includes(this.ownerOf(s)));
    },
    drawOrder() {
      const rank = (s) => {
        if (this.isServer(s)) return 0;
        if (s.id === 'wan' || this.isUplink(s)) return 1;
        return this.isSelected(s) ? 3 : 2;
      };
      return this.visibleHistory.slice().sort((a, b) => rank(a) - rank(b));
    },
    wanGraph() {
      return !!(this.state.config.directWan && this.wanAvailable);
    },
    serverShown() {
      return this.history.some((s) => this.isServer(s));
    },
    markerList() {
      const end = this.windowStart + this.range * 60000;
      return this.historyMarkers.filter((m) => m.at >= this.windowStart && m.at <= end);
    },
    recentEvents() {
      return (this.state.events || []).slice(0, 5);
    },
    legendItems() {
      return this.history
        .filter((s) => !this.isServer(s))
        .map((s) => {
          const t = this.state.tunnels.find((tunnel) => tunnel.id === s.id);
          const tunnel = s.id !== 'wan' && !this.isUplink(s);
          return {
            id: s.id,
            name: this.seriesName(s),
            owner: s.id,
            tunnel,
            selected: this.isSelected(s),
            color: this.seriesColor(s),
            preferred: tunnel && !!t && !!t.preferred,
            stored: tunnel && !!t && !t.connected,
          };
        });
    },
    hoverRows() {
      if (!this.hover) return [];
      const valueAt = (s) => {
        let nearest = null;
        for (const p of s.points)
          if (!nearest || Math.abs(p.at - this.hover.at) < Math.abs(nearest.at - this.hover.at))
            nearest = p;
        return nearest && isNumber(nearest.median) ? nearest.median : null;
      };
      const tunnels = this.visibleHistory
        .filter((s) => !this.isServer(s) && !this.isUplink(s) && s.id !== 'wan')
        .map((s) => {
          const server = this.visibleHistory.find(
            (v) => this.isServer(v) && this.ownerOf(v) === s.id,
          );
          const through = valueAt(s);
          const toServer = server ? valueAt(server) : null;
          let text =
            through === null
              ? 'No measurement through the tunnel'
              : this.ms(through) + ' through the tunnel';
          if (toServer !== null) text += ', ' + this.ms(toServer) + ' to its server';
          return {
            id: s.id,
            name: this.seriesName(s),
            color: this.seriesColor(s),
            selected: this.isSelected(s),
            wan: false,
            value: through,
            text,
          };
        });
      const others = this.visibleHistory
        .filter((s) => s.id === 'wan' || this.isUplink(s))
        .map((s) => {
          const value = valueAt(s);
          return {
            id: s.id,
            name: this.seriesName(s),
            color: this.seriesColor(s),
            selected: false,
            wan: true,
            value,
            text: value === null ? 'No measurement' : this.ms(value),
          };
        });
      return tunnels.concat(others).sort((a, b) => {
        if (a.value === null || b.value === null) return (a.value === null) - (b.value === null);
        return b.value - a.value || a.name.localeCompare(b.name);
      });
    },
  },
  watch: {
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
    if (this.updateActive) this.startUpdatePoll();
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
    this.stopUpdatePoll();
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
        next.config.autoTunnels = Array.isArray(next.config.autoTunnels)
          ? next.config.autoTunnels
          : [];
        next.capabilities = Object.assign(
          { configure: false, select: false, control: false },
          next.capabilities,
        );
        next.config = Object.assign(defaults(), next.config);
        if (!Array.isArray(next.config.targets) || !next.config.targets.length)
          next.config.targets = defaults().targets;
        next.auto = next.auto || { enabled: false, last: null };
        next.endpoints = Array.isArray(next.endpoints) ? next.endpoints : [];
        next.uplinks = Array.isArray(next.uplinks) ? next.uplinks : [];
        next.events = Array.isArray(next.events) ? next.events : [];
        next.resolved = next.resolved || {};
        next.wan = Object.assign({ available: true, interface: null, reason: '' }, next.wan);
        this.state = next;
        this.ready = true;
        if (this.updateActive && !this.updateTimer) this.startUpdatePoll();
        if (!this.destinations.includes(this.selectedTarget)) {
          this.selectedTarget = this.primary;
          this.historyAt = 0;
        }
        if (!next.tunnels.some((t) => t.id === this.selectedTunnel))
          this.selectedTunnel =
            (next.tunnels.find((t) => t.preferred) || next.tunnels[0] || {}).id || '';
        this.readError = '';
        if (Date.now() - this.historyAt >= historyInterval) await this.loadHistory();
      } catch (e) {
        this.readError = 'Wayline is unavailable. ' + this.describe(e, 'Try again shortly.');
      } finally {
        this.loading = false;
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
      this.draft = settingsFrom(this.state.config);
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
      const known = target && Object.prototype.hasOwnProperty.call(map, target) && map[target];
      return known ? 'Resolves to ' + map[target] : '';
    },
    makePrimary(index) {
      const [target] = this.draft.targets.splice(index, 1);
      this.draft.targets.unshift(target);
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
    changeFor(subject) {
      const cutoff = Date.now() - changeWindow;
      return this.state.events.find((e) => e.subject === subject && e.at >= cutoff) || null;
    },
    uplinkName(u) {
      if (!u.active) return u.interface || u.id;
      return u.interface ? 'Direct WAN (' + u.interface + ')' : 'Direct WAN';
    },
    unmeasurable(u) {
      return u.state === 'route_mismatch' || u.state === 'route_unavailable';
    },
    tunnelTone(t) {
      if (!t.enabled) return 'stopped';
      return t.connected ? 'connected' : 'offline';
    },
    selectableTunnel(t) {
      return t.selectable === undefined ? t.eligible : t.selectable;
    },
    rowNote(t) {
      if (t.preferred || !t.enabled || !this.can.select) return '';
      if (!this.selectableTunnel(t)) return 'Cannot be the exit: ' + (t.selectReason || t.reason);
      const pool = this.state.config.autoTunnels || [];
      if (this.state.auto && this.state.auto.enabled && pool.includes(t.monitorId) && !t.eligible)
        return 'Automatic switching skips it: ' + t.reason;
      return '';
    },
    exitIdentity(id) {
      const e = this.egressFor(id);
      if (!e || !e.ip || e.error) return null;
      return { ip: e.ip, place: this.place(e.location) };
    },
    isExit(t) {
      return !!this.exitTunnel && this.exitTunnel.id === t.id;
    },
    place(code) {
      if (!validCountry(code)) return { flag: '', name: '' };
      return { flag: flagOf(code), name: countryName(code) };
    },
    diagnosticValue(id) {
      const d = (this.state.diagnostics || {})[this.selectedTarget] || {};
      return isNumber(d[id]) ? d[id] : null;
    },
    uplinkValue(u) {
      return this.diagnosticValue(u.active ? 'wan' : u.id);
    },
    serverFor(t) {
      return this.endpoints.find((e) => e.tunnel === t.id) || null;
    },
    serverTitle(t) {
      const e = this.serverFor(t);
      return e && !e.address && e.reason ? e.reason : null;
    },
    serverMs(e) {
      const d = (this.state.diagnostics || {})[e.address] || {};
      return isNumber(d.wan) ? this.ms(d.wan) : '—';
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
    latencyNote(t) {
      if (this.lossShown(t)) {
        return { text: this.lossText(t) + ' loss', cls: 'vph-note-loss', title: this.lossTitle(t) };
      }
      const active = this.activeMedian;
      if (this.comparable && !t.preferred && active !== null && isNumber(t.median)) {
        const diff = active - t.median;
        if (Math.abs(diff) <= 3) return { text: 'About the same as the active exit', cls: '' };
        if (diff > 0) {
          return {
            text: diff.toFixed(2) + ' ms faster than the active exit',
            cls: 'vph-note-faster',
          };
        }
        return { text: (-diff).toFixed(2) + ' ms slower than the active exit', cls: '' };
      }
      return null;
    },
    useDisabled(t) {
      return this.busy || !this.selectableTunnel(t) || !this.can.select;
    },
    useReason(t) {
      if (!this.can.select) return this.selectReason;
      if (!this.selectableTunnel(t)) return t.selectReason || t.reason;
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
    pruneAutomaticTunnels() {
      this.draft.autoTunnels = this.draft.autoTunnels.filter((id) =>
        this.draft.tunnels.includes(id),
      );
    },
    async saveSettings() {
      if (this.busy) return;
      this.error = '';
      const targets = this.draft.targets;
      if (
        !targets.length ||
        targets.some((t) => !validTarget(t)) ||
        new Set(targets.map((t) => t.toLowerCase())).size !== targets.length
      ) {
        this.error =
          'Use an IPv4 address or a host name for each destination. Each destination must be unique.';
        return;
      }
      if (this.draft.enabled && !this.draft.tunnels.length) {
        this.error = 'Choose at least one tunnel to monitor.';
        return;
      }
      this.pruneAutomaticTunnels();
      if (this.draft.autoExit && this.draft.autoTunnels.length < 2) {
        this.error = 'Choose at least two monitored tunnels for automatic switching.';
        return;
      }
      this.busy = true;
      let saved = false;
      try {
        await this.call('set_config', {
          config: settingsFrom(this.draft),
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
      if (!saved && this.settingsOpen) this.draft = settingsFrom(this.state.config);
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
    async selectExit(t) {
      if (this.busy || t.preferred || !this.selectableTunnel(t) || !this.can.select) return;
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
      this.updateError = '';
      try {
        const r = await this.call('install_update');
        this.setUpdate(r.update);
      } catch (e) {
        this.updateError = this.describe(e, 'Update unavailable.');
      }
    },
    async checkUpdates() {
      this.updateError = '';
      try {
        const r = await this.call('check_update');
        this.setUpdate(r.update);
      } catch (e) {
        this.updateError = this.describe(e, 'Update check unavailable.');
      }
    },
    setUpdate(update) {
      if (!update || typeof update !== 'object') return;
      this.$set(this.state, 'update', update);
      if (this.updateActive) this.startUpdatePoll();
      else this.stopUpdatePoll();
    },
    startUpdatePoll() {
      clearInterval(this.updateTimer);
      this.updateTimer = setInterval(this.pollUpdate, 1000);
    },
    stopUpdatePoll() {
      clearInterval(this.updateTimer);
      this.updateTimer = null;
      this.updateWaiting = false;
      this.updateLostSince = 0;
    },
    async pollUpdate() {
      if (this.updatePolling || this.stopped) return;
      this.updatePolling = true;
      try {
        const next = await this.call('get_state');
        if (this.stopped || !next || !next.update) return;
        this.$set(this.state, 'update', next.update);
        this.updateWaiting = false;
        this.updateLostSince = 0;
        if (!this.updateActive) this.stopUpdatePoll();
      } catch {
        if (!this.updateLostSince) this.updateLostSince = Date.now();
        this.updateWaiting = true;
        if (Date.now() - this.updateLostSince >= 60000) {
          this.stopUpdatePoll();
          this.updateError =
            'Lost contact with the router. Refresh the page to see the update status.';
        }
      } finally {
        this.updatePolling = false;
      }
    },
    chooseTarget(target) {
      this.selectedTarget = target;
      storage.set(targetKey, target);
      this.loadHistory();
    },
    async loadHistory() {
      if (this.offset === 0) this.anchor = Math.floor(Date.now() / 60000) * 60000;
      const request = ++this.historyRequest;
      this.historyAt = Date.now();
      if (!this.selectedTunnel) return;
      try {
        const endpoints = this.endpointHistory;
        const r = await this.call('get_history', {
          tunnel: this.selectedTunnel,
          target: this.selectedTarget,
          all: true,
          uplinks: true,
          start: this.windowStart,
          end: this.windowStart + this.range * 60000,
          step: this.range > 360 ? 5 : 1,
          include_wan: this.wanGraph,
          ...(endpoints ? { endpoints: true } : {}),
        });
        if (request === this.historyRequest) {
          const series = Array.isArray(r.series) ? r.series : [];
          series.forEach((item) => {
            item.points = Array.isArray(item.points) ? item.points : [];
          });
          this.history = series.filter(
            (item) =>
              !this.isServer(item) ||
              (endpoints &&
                series.some((owner) => !this.isServer(owner) && owner.id === this.ownerOf(item)) &&
                item.points.some((p) => isNumber(p.median))),
          );
          this.historyMarkers = Array.isArray(r.events) ? r.events : [];
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
          this.historyMarkers = [];
          this.historyError = e.message || 'History unavailable.';
        }
      }
    },
    hasMs(n) {
      return isNumber(n);
    },
    ms(n) {
      return n.toFixed(2) + ' ms';
    },
    latency(n) {
      return isNumber(n) ? n.toFixed(2) : '—';
    },
    clock(t) {
      return new Date(t).toLocaleTimeString([], {
        hour: '2-digit',
        minute: '2-digit',
        hour12: false,
      });
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
    keyNav(e) {
      if (e.key === 'Escape') {
        this.goNow();
        return;
      }
      if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
      e.preventDefault();
      this.shift(e.key === 'ArrowLeft' ? 1 : -1);
    },
    shift(direction) {
      this.setOffset(this.offset + (direction * this.range) / 2);
    },
    setOffset(minutes) {
      this.offset = Math.round(Math.min(1440 - this.range, Math.max(0, minutes)));
      this.hover = null;
      this.loadHistory();
    },
    goNow() {
      this.setOffset(0);
    },
    choosePreset(minutes) {
      this.preset = minutes;
      this.range = minutes;
      this.setOffset(0);
    },
    svgX(e) {
      const rect = e.currentTarget.getBoundingClientRect();
      const x = ((e.clientX - rect.left) * this.chartWidth) / rect.width;
      return Math.max(45, Math.min(this.chartWidth - 18, x));
    },
    timeAt(x) {
      return this.windowStart + ((x - 45) / (this.chartWidth - 63)) * this.range * 60000;
    },
    hoverX(x) {
      const marker = this.markerList.find((m) => Math.abs(this.chartX(m.at) - x) < 5);
      this.hover = { x, at: this.timeAt(x), marker: marker ? marker.text : '' };
    },
    pointerDown(e) {
      if (e.button !== 0) return;
      const x = this.svgX(e);
      e.currentTarget.setPointerCapture(e.pointerId);
      this.drag = { from: x, to: x };
      this.hoverX(x);
    },
    pointerMove(e) {
      const x = this.svgX(e);
      if (!this.drag) {
        this.hoverX(x);
        return;
      }
      this.drag.to = x;
      this.hover = null;
    },
    pointerUp() {
      if (!this.drag) return;
      const { from, to } = this.drag;
      this.drag = null;
      const start = this.timeAt(Math.min(from, to));
      const end = this.timeAt(Math.max(from, to));
      const minutes = Math.round((end - start) / 60000);
      if (minutes < 5) {
        this.hoverX(to);
        return;
      }
      this.range = Math.min(1440, minutes);
      this.setOffset(Math.round((this.anchor - end) / 60000));
    },
    showMarker(m) {
      this.hover = { x: this.chartX(m.at), at: m.at, marker: m.text };
    },
    highlight(id) {
      this.selectedTunnel = id;
    },
    isolate(item, event) {
      const id = item.owner;
      if (event && (event.ctrlKey || event.metaKey || event.shiftKey)) {
        this.isolated = this.isolated.includes(id)
          ? this.isolated.filter((other) => other !== id)
          : this.isolated.concat(id);
      } else {
        this.isolated = this.isolated.length === 1 && this.isolated[0] === id ? [] : [id];
      }
      if (item.tunnel) this.highlight(id);
    },
    isServer(series) {
      return series.id.startsWith(endpointPrefix);
    },
    isUplink(series) {
      return series.id.startsWith(uplinkPrefix);
    },
    seriesName(series) {
      if (series.id === 'wan') return 'Direct WAN (no VPN)';
      if (this.isUplink(series)) return series.id.slice(uplinkPrefix.length) + ' uplink';
      return series.name;
    },
    ownerOf(series) {
      return this.isServer(series) ? series.id.slice(endpointPrefix.length) : series.id;
    },
    isSelected(series) {
      return series.id !== 'wan' && this.ownerOf(series) === this.selectedTunnel;
    },
    isDimmed(series) {
      return (
        series.id !== 'wan' &&
        !this.isUplink(series) &&
        !this.isSelected(series) &&
        this.history.some((s) => this.isSelected(s))
      );
    },
    hasBand(series) {
      return !this.isServer(series) && this.isSelected(series);
    },
    seriesColor(series) {
      const owner = this.ownerOf(series);
      if (owner === 'wan' || this.isUplink(series)) return 'var(--vph-muted)';
      const index = this.state.tunnels.findIndex((t) => t.id === owner);
      if (index >= 0 && this.state.tunnels[index].preferred) return 'var(--vph-brand)';
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
.vph-col-through {
  width: 170px;
}
.vph-col-server {
  width: 150px;
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
.vph-exits td {
  height: 64px;
}
.vph-exits .vph-tunnel {
  width: 100%;
  max-width: 0;
}
.vph-uplink-row td {
  color: var(--vph-muted);
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
  flex-wrap: wrap;
  align-items: center;
  gap: 4px 8px;
  min-width: 0;
}
.vph-name-line .vph-name {
  min-width: 0;
}
.vph-server-ms,
.vph-server-address {
  display: block;
}
.vph-wan-name {
  font-weight: 400;
  color: var(--vph-muted);
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
.vph-tag {
  flex: none;
  display: inline-block;
  border-radius: 4px;
  font-size: 11px;
  line-height: 16px;
  white-space: nowrap;
  color: var(--vph-warn-text);
  background: color-mix(in srgb, #e0a800 18%, var(--vph-card));
  padding: 1px 8px;
}
.vph-status {
  font-size: 12px;
  margin-top: 4px;
}
.vph-metric .vph-number {
  display: block;
}
.vph .vph-latency {
  font-size: 22px;
  font-weight: 500;
  line-height: 28px;
}
.vph .vph-latency small {
  font-size: 12px;
  font-weight: 400;
  color: var(--vph-muted);
}
.vph-latency-text {
  font-size: 13px;
  color: var(--vph-muted);
}
.vph-latency-note {
  display: block;
  margin-top: 2px;
  font-size: 12px;
  line-height: 18px;
  font-variant-numeric: tabular-nums;
  color: var(--vph-muted);
}
.vph-note-loss {
  color: var(--vph-danger-text);
}
.vph-note-faster {
  color: var(--vph-brand-text);
}
.vph-action {
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
.vph-dot {
  fill: var(--series, var(--vph-brand));
}
.vph-wan-series .vph-dot {
  fill: var(--vph-muted);
}
.vph-hero {
  padding: 22px;
  background: color-mix(in srgb, var(--vph-brand) 6%, var(--vph-card));
}
.vph-hero p {
  margin: 0;
}
.vph-hero-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(min(100%, 320px), 1fr));
  align-items: start;
  gap: 20px 32px;
}
.vph-hero .vph-hero-label {
  font-size: 13px;
}
.vph-hero .vph-hero-main {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-top: 8px;
}
.vph-hero-flag {
  font-size: 28px;
}
.vph-hero-ip {
  font-size: 30px;
  font-weight: 500;
  font-variant-numeric: tabular-nums;
  overflow-wrap: anywhere;
}
.vph-hero .vph-hero-pending {
  font-size: 16px;
}
.vph-hero .vph-hero-route,
.vph-hero .vph-hero-latency,
.vph-hero-sentence {
  font-size: 15px;
}
.vph-hero .vph-hero-route,
.vph-hero .vph-hero-latency {
  margin-top: 8px;
}
.vph-hero-secondary .vph-hero-flag,
.vph-hero-secondary .vph-hero-ip {
  font-size: 22px;
}
.vph-hero-compare {
  margin-top: 20px;
}
.vph-bars {
  display: grid;
  gap: 8px;
  margin-top: 10px;
}
.vph-bar-row {
  display: grid;
  grid-template-columns: 56px minmax(0, 1fr) 72px;
  align-items: center;
  gap: 10px;
}
.vph-bar-name,
.vph-bar-value {
  font-size: 12px;
}
.vph-bar-track {
  height: 8px;
  border-radius: 4px;
  overflow: hidden;
  background: color-mix(in srgb, var(--vph-muted) 14%, var(--vph-card));
}
.vph-bar-fill {
  display: block;
  height: 100%;
  background: var(--vph-muted);
}
.vph-bar-vpn {
  background: var(--vph-brand);
}
.vph-bar-value {
  text-align: right;
}
.vph-hero .vph-hero-updated {
  margin-top: 16px;
  font-size: 12px;
  text-align: right;
}
.vph-flag {
  font-family: 'Apple Color Emoji', 'Segoe UI Emoji', 'Noto Color Emoji', sans-serif;
}
.vph-table-head {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 10px 16px;
  margin-top: 18px;
}
.vph-table-head p {
  margin: 0;
  font-size: 13px;
}
.vph-key {
  display: flex;
  flex-wrap: wrap;
  gap: 4px 16px;
  margin: 0;
  padding: 0 8px;
  list-style: none;
  font-size: 12px;
  color: var(--vph-muted);
}
.vph-key li {
  display: inline-flex;
  align-items: center;
  gap: 8px;
}
.vph-key-line {
  display: block;
  width: 24px;
  height: 8px;
  overflow: visible;
}
.vph-key-line line {
  stroke: var(--vph-muted);
  stroke-width: 1.8;
}
.vph-key-line .vph-key-dashed {
  stroke-dasharray: 4 3;
}
.vph-legend-item.dimmed {
  opacity: 0.45;
}
.vph-legend-hint {
  margin: 6px 0 0;
  font-size: 12px;
}
.vph-legend-dot {
  display: inline-block;
  width: 9px;
  height: 9px;
  margin-right: 6px;
  border-radius: 50%;
  background: var(--series, var(--vph-brand));
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
.vph-chart-controls {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px 16px;
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
.vph-ranges .vph-step {
  font-size: 18px;
  line-height: 1;
  padding: 5px 9px;
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
  touch-action: pan-y;
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
  stroke-width: 1.5;
}
.vph-wan-series .vph-band {
  fill: var(--vph-muted);
  opacity: 0.05;
}
.vph-uplink-series .vph-line {
  stroke-width: 1.2;
  opacity: 0.8;
}
.vph-endpoint-series .vph-line {
  stroke-width: 1.2;
  stroke-dasharray: 4 3;
}
.vph-endpoint-series.vph-series-selected .vph-line {
  stroke-width: 1.8;
}
.vph-marker-line {
  stroke: var(--vph-muted);
  stroke-width: 1;
  stroke-dasharray: 2 3;
}
.vph-marker-dot {
  fill: var(--vph-muted);
}
.vph-marker {
  outline: none;
}
.vph-marker:focus-visible .vph-marker-dot {
  stroke: var(--vph-brand);
  stroke-width: 2;
}
.vph-select {
  fill: var(--vph-brand);
  opacity: 0.14;
  pointer-events: none;
}
.vph-guide {
  stroke: var(--vph-muted);
  stroke-width: 1;
  stroke-dasharray: 3 3;
}
.vph-tooltip {
  position: absolute;
  top: 25px;
  width: max-content;
  max-width: min(380px, calc(100% - 24px));
  white-space: normal;
  overflow-wrap: anywhere;
  line-height: 1.5;
  padding: 10px 12px;
  background: var(--vph-card);
  border: 1px solid var(--vph-border);
  border-radius: 5px;
  font-size: 12px;
  pointer-events: none;
}
.vph-tip-event {
  color: var(--vph-brand-text);
  white-space: normal;
  margin: 2px 0 4px;
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
  border-top: 1.5px solid var(--vph-muted);
}
.vph-legend-block {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 2px;
  min-width: 0;
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
.vph-changes {
  padding: 16px 18px;
}
.vph-changes h2 {
  margin-bottom: 8px;
}
.vph-changes ul {
  list-style: none;
  margin: 0;
  padding: 0;
}
.vph-changes li {
  display: flex;
  align-items: baseline;
  gap: 14px;
  font-size: 13px;
  padding: 7px 0;
  border-top: 1px solid var(--vph-border);
}
.vph-changes time {
  flex: none;
  width: 84px;
  font-size: 12px;
  font-variant-numeric: tabular-nums;
}
.vph-settings {
  padding: 24px;
}
.vph-settings h2 {
  margin-bottom: 4px;
}
.vph-set {
  padding: 18px 0;
  border-top: 1px solid var(--vph-border);
}
.vph-set h3 {
  font-size: 14px;
  font-weight: 600;
  margin: 0;
}
.vph-set p {
  margin: 4px 0 0;
  font-size: 13px;
}
.vph-set .vph-check {
  margin: 10px 0 0;
}
.vph-set select {
  width: auto;
  min-width: 200px;
  margin-top: 8px;
}
.vph-row {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 12px 28px;
  margin-top: 10px;
}
.vph-inline {
  display: flex;
  align-items: center;
  gap: 8px;
}
.vph .vph-inline select {
  min-width: 0;
  margin: 0;
}
.vph-tunnels {
  display: flex;
  flex-wrap: wrap;
  gap: 0 24px;
  margin-top: 4px;
}
.vph-dests {
  display: grid;
  gap: 10px;
  margin: 12px 0 10px;
  padding: 0;
  list-style: none;
}
.vph-dest {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  align-items: start;
  gap: 8px 16px;
}
.vph-dest-field small {
  display: block;
  margin-top: 2px;
}
.vph-dest-actions {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 14px;
  min-height: 41px;
  margin-top: 7px;
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
.vph-footnote a {
  color: var(--vph-muted);
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
  .vph-ranges button {
    min-height: 44px;
  }
  .vph-link {
    min-height: 44px;
  }
  .vph-info {
    padding: 12px;
  }
  .vph-history,
  .vph-settings {
    padding: 17px 12px;
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
  .vph-preview {
    font-size: 12px;
  }
}
@media (max-width: 639px) {
  .vph-hero-primary .vph-hero-ip {
    font-size: 24px;
  }
  .vph-exits thead {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
  }
  .vph-exits tbody tr {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 16px;
    padding: 12px 16px;
    border-bottom: 1px solid var(--vph-border);
  }
  .vph-exits tbody tr:last-child {
    border-bottom: 0;
  }
  .vph-exits td {
    height: auto;
    padding: 0;
    border-bottom: 0;
  }
  .vph-exits .vph-tunnel {
    flex: 1 0 100%;
    width: auto;
    max-width: none;
    min-width: 0;
  }
  .vph-exits td[data-label]::before {
    content: attr(data-label);
    margin-right: 6px;
    font-size: 12px;
    color: var(--vph-muted);
  }
  .vph-exits .vph-action {
    min-width: 0;
    margin-left: auto;
  }
  .vph-exits .vph-action.vph-confirming {
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
