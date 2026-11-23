import { Button } from '@ui-sdk/react/button';
import { Input } from '@ui-sdk/react/input';
import { useId, useState } from 'react';
import { Modal } from './modal.tsx';
import { Check, Pick } from './kit.tsx';
import {
  busy,
  busyAction,
  closeSettings,
  draft,
  error,
  pruneAutomaticTunnels,
  purgeConfirm,
  purgeHistory,
  saveSettings,
  settingsOpen,
  state,
  updateDraft,
} from './store.ts';
import type { Settings } from './types.ts';
import { UpdatePanel } from './update-panel.tsx';

const intervals = [1, 5, 15, 30, 60].map((n) => ({ value: String(n), label: n + ' s' }));
const keep = [
  { value: '1', label: '24 hours' },
  { value: '7', label: '7 days' },
  { value: '30', label: '30 days' },
  { value: '0', label: 'Memory only' },
];

const toggle = (list: string[], value: string, on: boolean): string[] =>
  on ? (list.includes(value) ? list : list.concat(value)) : list.filter((v) => v !== value);

let nextKey = 0;

const settingNames: Record<keyof Settings, string> = {
  enabled: 'latency measurement',
  interval: 'probe interval',
  targets: 'destinations',
  tunnels: 'monitored tunnels',
  directWan: 'direct internet probes',
  endpointDiagnostics: 'VPN server probes',
  upstreamWifi: 'upstream Wi-Fi sampling',
  telemetry: 'router telemetry',
  autoExit: 'automatic exit switching',
  autoTunnels: 'automatic switching pool',
  historyDays: 'history retention',
  autoUpdates: 'automatic updates',
};

const shown = (value: unknown): string =>
  Array.isArray(value)
    ? value.join(', ') || 'none'
    : typeof value === 'boolean'
      ? value
        ? 'on'
        : 'off'
      : String(value);

const changeSummary = (next: Settings, current: Settings): string | undefined => {
  const parts = (Object.keys(settingNames) as (keyof Settings)[])
    .filter((key) => JSON.stringify(next[key]) !== JSON.stringify(current[key]))
    .map((key) => settingNames[key] + ' ' + shown(current[key]) + ' → ' + shown(next[key]));
  return parts.length ? 'Saves router settings: ' + parts.join('; ') : undefined;
};

function Destinations() {
  const d = draft.value;
  const resolved = state.value.resolved || {};
  const [stored, setKeys] = useState(() => d.targets.map(() => nextKey++));
  const base = useId();
  const keys =
    stored.length === d.targets.length ? stored : d.targets.map((_, i) => stored[i] ?? -1 - i);
  const resolvedText = (target: string): string =>
    target && Object.prototype.hasOwnProperty.call(resolved, target) && resolved[target]
      ? 'Resolves to ' + resolved[target]
      : '';
  const setTarget = (i: number, value: string) => {
    const targets = d.targets.slice();
    targets[i] = value.trim();
    updateDraft({ targets });
  };
  const move = (i: number) => {
    const targets = d.targets.slice();
    const [target] = targets.splice(i, 1);
    targets.unshift(target);
    const order = keys.slice();
    const [key] = order.splice(i, 1);
    order.unshift(key);
    setKeys(order);
    updateDraft({ targets });
  };
  const remove = (i: number) => {
    setKeys(keys.filter((_, n) => n !== i));
    updateDraft({ targets: d.targets.filter((_, n) => n !== i) });
  };
  return (
    <>
      <ol className="wl-dests">
        {d.targets.map((target, i) => {
          const name = i ? 'Destination ' + (i + 1) : 'Primary destination';
          const hint = resolvedText(target);
          return (
            <li key={keys[i]} className="wl-dest">
              <div className="wl-dest-field">
                <label htmlFor={base + keys[i]}>{name}</label>
                <Input
                  id={base + keys[i]}
                  aria-describedby={hint ? base + keys[i] + '-hint' : undefined}
                  name={'destination-' + (i + 1)}
                  value={target}
                  required
                  maxLength={253}
                  autoComplete="off"
                  autoCapitalize="off"
                  spellCheck={false}
                  placeholder={i ? 'IP address or host name' : '1.1.1.1'}
                  onChange={(e) => setTarget(i, e.currentTarget.value)}
                />
                {hint && <small id={base + keys[i] + '-hint'}>{hint}</small>}
              </div>
              <div className="wl-dest-actions">
                {!i ? (
                  <span className="wl-tag">Primary</span>
                ) : (
                  <>
                    <Button
                      className="wl-link"
                      aria-label={'Make destination ' + (i + 1) + ' primary'}
                      onClick={() => move(i)}
                    >
                      Make primary
                    </Button>
                    <Button
                      className="wl-link"
                      aria-label={'Remove destination ' + (i + 1)}
                      onClick={() => remove(i)}
                    >
                      Remove
                    </Button>
                  </>
                )}
              </div>
            </li>
          );
        })}
      </ol>
      <Button
        className="wl-link"
        disabled={d.targets.length >= 5}
        onClick={() => {
          setKeys(keys.concat(nextKey++));
          updateDraft({ targets: d.targets.concat('') });
        }}
      >
        Add destination
      </Button>
    </>
  );
}

function Purge() {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button className="wl-button" disabled={busy.value} onClick={() => setOpen(true)}>
        Purge history
      </Button>
      <Modal
        open={open}
        onClose={() => setOpen(false)}
        title="Delete all stored history?"
        className="wl-alert"
        alert
      >
        <h2>Delete all stored history?</h2>
        <p>
          This permanently deletes all stored latency, Wi-Fi and telemetry history and change
          events, including recovery copies. Live measurements continue.
        </p>
        <div className="wl-actions">
          <Button className="wl-button" onClick={() => setOpen(false)}>
            Cancel purge
          </Button>
          <Button
            className="wl-button wl-danger"
            data-agent-confirm="Deletes all stored Wayline history on the router, including recovery copies"
            onClick={() => {
              setOpen(false);
              purgeConfirm.value = true;
              return purgeHistory();
            }}
          >
            {busyAction.value === 'purge' ? 'Purging…' : 'Delete all history'}
          </Button>
        </div>
      </Modal>
    </>
  );
}

export function SettingsDrawer() {
  const open = settingsOpen.value;
  const d = draft.value;
  const s = state.value;
  return (
    <Modal
      open={open}
      onClose={closeSettings}
      title="Settings"
      className="wl-drawer"
      id="wl-settings"
    >
      <form
        className="wl-drawer-form"
        noValidate
        onSubmit={(e) => {
          e.preventDefault();
          void saveSettings();
        }}
      >
        <header className="wl-drawer-head">
          <div>
            <h2 id="wl-settings-title">Settings</h2>
            <p id="wl-settings-description" className="wl-muted">
              Changes apply when you save. Revision {s.revision || '—'}.
            </p>
          </div>
        </header>
        <div className="wl-drawer-body">
          <section className="wl-set" aria-labelledby="wl-set-monitoring">
            <h3 id="wl-set-monitoring">Monitoring</h3>
            <Check
              label="Measure latency"
              checked={d.enabled}
              onChange={(enabled) => updateDraft({ enabled })}
            />
            <Pick
              label="Measure every"
              value={String(d.interval)}
              items={intervals}
              onChange={(v) => updateDraft({ interval: Number(v) })}
            />
          </section>
          <section className="wl-set" aria-labelledby="wl-set-destinations">
            <h3 id="wl-set-destinations">Destinations to ping</h3>
            <p className="wl-muted">
              The primary destination is used to compare tunnels and to switch the exit.
            </p>
            <Destinations />
          </section>
          <section className="wl-set" aria-labelledby="wl-set-tunnels">
            <h3 id="wl-set-tunnels">Tunnels to monitor</h3>
            <div className="wl-checks">
              {s.tunnels.map((t) => (
                <Check
                  key={t.id}
                  label={'Monitor ' + t.name}
                  checked={d.tunnels.includes(t.monitorId)}
                  onChange={(on) => {
                    updateDraft({ tunnels: toggle(draft.value.tunnels, t.monitorId, on) });
                    pruneAutomaticTunnels();
                  }}
                />
              ))}
              {!s.tunnels.length && (
                <p className="wl-muted">No configured VPN tunnels available.</p>
              )}
            </div>
          </section>
          <section className="wl-set" aria-labelledby="wl-set-also">
            <h3 id="wl-set-also">Also measure</h3>
            <Check
              label="Direct internet without VPN (every uplink)"
              checked={d.directWan}
              onChange={(directWan) => updateDraft({ directWan })}
            />
            <p className="wl-muted">
              Probes bypass the VPN. Where a probe cannot, the monitor says so.
            </p>
            <Check
              label="VPN servers, outside the tunnel"
              checked={d.endpointDiagnostics}
              onChange={(endpointDiagnostics) => updateDraft({ endpointDiagnostics })}
            />
            <p className="wl-muted">
              The public address of each monitored server. It shows the route, not latency through
              the tunnel.
            </p>
            <Check
              label="Upstream Wi-Fi link (repeater)"
              checked={d.upstreamWifi && s.wifi.radio !== false}
              disabled={s.wifi.radio === false}
              onChange={(upstreamWifi) => updateDraft({ upstreamWifi })}
            />
            <p className="wl-muted">
              {s.wifi.radio === false
                ? 'This router has no Wi-Fi radio.'
                : 'Signal, noise, link rate, retries, and channel load of the network this router joins. It works without VPN tunnels and keeps history like latency.'}
            </p>
            <Check
              label="Router and interface telemetry"
              checked={d.telemetry}
              onChange={(telemetry) => updateDraft({ telemetry })}
            />
            <p className="wl-muted">
              Processor, memory, temperature, tracked connections, per-interface throughput,
              WireGuard handshakes. Sampled every 5 seconds and kept like latency.
            </p>
          </section>
          <section className="wl-set" aria-labelledby="wl-set-exit">
            <h3 id="wl-set-exit">Exit</h3>
            <Check
              label="Switch the exit automatically"
              checked={d.autoExit}
              onChange={(autoExit) => updateDraft({ autoExit })}
            />
            <p className="wl-muted">
              Moves to a clearly faster healthy tunnel (20% and 10 ms better, at most every 5
              minutes) or away from an unhealthy one after 1 minute.
            </p>
            {d.autoExit && (
              <>
                <fieldset className="wl-checks">
                  <legend>Tunnels allowed for automatic switching</legend>
                  {s.tunnels.map((t) => (
                    <Check
                      key={t.id}
                      label={'Allow ' + t.name}
                      checked={d.autoTunnels.includes(t.monitorId)}
                      disabled={!d.tunnels.includes(t.monitorId)}
                      onChange={(on) =>
                        updateDraft({
                          autoTunnels: toggle(draft.value.autoTunnels, t.monitorId, on),
                        })
                      }
                    />
                  ))}
                </fieldset>
                <p className="wl-muted">
                  Choose at least two monitored tunnels. Only healthy, connected members can become
                  the exit. Automatic switching pauses when the current exit is outside this pool.
                  Stopped tunnels stay stopped.
                </p>
                <p className="wl-warning" role="note">
                  Warning: this changes the native VPN priority by itself. Existing connections do
                  not move.
                </p>
              </>
            )}
          </section>
          <section className="wl-set" aria-labelledby="wl-set-history">
            <h3 id="wl-set-history">History</h3>
            <Pick
              label="Keep history"
              value={String(d.historyDays)}
              items={keep}
              onChange={(v) => updateDraft({ historyDays: Number(v) })}
            />
            <p className="wl-muted">
              Recent minutes keep full detail; older history is summarized. The monitor saves in
              batches to limit flash writes.
            </p>
            <p className="wl-muted">
              Purge stored latency, Wi-Fi and telemetry history and change events from memory and
              flash. Live measurements continue. Settings and VPN tunnels stay unchanged.
            </p>
            <Purge />
          </section>
          <section className="wl-set" aria-labelledby="wl-set-updates">
            <h3 id="wl-set-updates">Updates</h3>
            <Check
              label="Install updates automatically"
              checked={d.autoUpdates}
              onChange={(autoUpdates) => updateDraft({ autoUpdates })}
            />
            <p className="wl-muted">
              Checks once a day and installs the newest signed release, including alpha and beta
              releases.
            </p>
            <UpdatePanel />
          </section>
        </div>
        <footer className="wl-drawer-foot">
          {error.value && (
            <p className="wl-error wl-drawer-error" role="alert">
              {error.value}
            </p>
          )}
          <Button className="wl-button" disabled={busy.value} onClick={closeSettings}>
            Cancel
          </Button>
          <Button
            type="submit"
            className="wl-button wl-primary"
            disabled={busy.value}
            data-agent-confirm={changeSummary(d, s.config)}
          >
            {busy.value && busyAction.value !== 'purge' ? 'Saving…' : 'Save settings'}
          </Button>
        </footer>
      </form>
    </Modal>
  );
}
