import { Button } from '@ui-sdk/react/button';
import { useEffect, useState } from 'react';
import {
  bundleVersion,
  checkUpdates,
  installUpdate,
  reloadPage,
  updatePhases,
  updatesDisabled,
  updateState,
  updateWaiting,
} from './store.ts';

const phaseLines: Record<string, string> = {
  checking: 'Checking for a newer release…',
  downloading: 'Downloading the package…',
  verifying: 'Verifying the signature and hash…',
  installing: 'Installing the update…',
};

const agoSeconds = (now: number, seconds: number): string => {
  const s = Math.max(0, Math.floor(now / 1000) - seconds);
  if (s < 60) return 'just now';
  if (s < 3600) return Math.floor(s / 60) + ' min ago';
  if (s < 86400) return Math.floor(s / 3600) + ' h ago';
  return Math.floor(s / 86400) + ' d ago';
};

export function UpdatePanel(props: { compact?: boolean }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const clock = setInterval(() => setNow(Date.now()), 30000);
    return () => clearInterval(clock);
  }, []);
  const u = updateState.value;
  const failed = u.failed || u.phase === 'failed';
  const active = updatePhases.includes(u.phase);
  const working = updatesDisabled.value || (active && !failed);
  const stale = !!bundleVersion && bundleVersion !== u.current;
  const showReload = !failed && u.phase === 'done' && stale;
  const chip = active
    ? { text: u.phase === 'checking' ? 'Checking…' : 'Updating…', cls: 'wl-muted' }
    : !u.latest
      ? null
      : u.available
        ? { text: 'Update available', cls: 'wl-upd-brand' }
        : { text: 'Up to date', cls: '' };
  let status = phaseLines[u.phase] || '';
  if (failed) status = u.message || 'The update failed.';
  else if (updateWaiting.value) status = 'Waiting for the router to finish…';
  else if (u.phase === 'done')
    status =
      'Updated to ' + (u.latest || u.current) + (stale ? '. Reload this page to use it.' : '.');
  return (
    <div className={'wl-upd' + (props.compact ? ' wl-upd-compact' : '')}>
      {props.compact ? (
        <p>
          Installed {u.current || 'unknown'}
          {u.latest ? ', latest ' + u.latest : ', not checked yet'}
        </p>
      ) : (
        <>
          <dl className="wl-upd-facts">
            <div>
              <dt className="wl-muted">Installed</dt>
              <dd className="wl-num">{u.current || 'Unknown'}</dd>
            </div>
            <div>
              <dt className="wl-muted">Latest</dt>
              <dd>
                <span className={'wl-num' + (u.latest ? '' : ' wl-muted')}>
                  {u.latest || 'Not checked yet'}
                </span>
                {chip && <span className={'wl-upd-chip ' + chip.cls}>{chip.text}</span>}
              </dd>
            </div>
          </dl>
          <p className="wl-muted">
            {u.checkedAt ? 'Last checked ' + agoSeconds(now, u.checkedAt) : 'Never checked'}
          </p>
        </>
      )}
      <p className={'wl-upd-status' + (failed ? ' wl-upd-failed' : '')} aria-live="polite">
        {status}
      </p>
      <div className="wl-actions">
        <Button className="wl-button" disabled={working} onClick={() => void checkUpdates()}>
          Check for update
        </Button>
        {u.available && (
          <Button
            className="wl-button wl-primary"
            disabled={working}
            data-agent-confirm={
              'Installs Wayline ' +
              u.latest +
              ' on the router, replacing ' +
              (u.current || 'the installed version')
            }
            onClick={() => void installUpdate()}
          >
            Install {u.latest}
          </Button>
        )}
        {showReload && (
          <Button className="wl-button wl-primary" onClick={reloadPage}>
            Reload page
          </Button>
        )}
        {failed && (
          <Button
            className="wl-button"
            disabled={working}
            onClick={() => void (u.available ? installUpdate() : checkUpdates())}
          >
            Try again
          </Button>
        )}
        {u.latest && (
          <a
            className="wl-link"
            href={'https://github.com/epk/wayline/releases/tag/v' + u.latest}
            target="_blank"
            rel="noopener noreferrer"
          >
            Release notes
          </a>
        )}
      </div>
    </div>
  );
}
