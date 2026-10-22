<template>
  <div class="vph-upd" :class="{ 'vph-upd-compact': compact }">
    <p v-if="compact">{{ compactText }}</p>
    <dl v-else class="vph-upd-facts">
      <div>
        <dt class="vph-muted">Installed</dt>
        <dd class="vph-upd-value vph-number">{{ update.current || 'Unknown' }}</dd>
      </div>
      <div>
        <dt class="vph-muted">Latest</dt>
        <dd class="vph-upd-value">
          <span class="vph-number" :class="{ 'vph-muted': !update.latest }">{{ latestText }}</span>
          <span v-if="chip" class="vph-upd-chip" :class="chip.cls">{{ chip.text }}</span>
        </dd>
      </div>
    </dl>
    <p v-if="!compact" class="vph-muted vph-upd-meta">{{ checkedText }}</p>
    <p class="vph-upd-status" :class="{ 'vph-upd-failed': failed }" aria-live="polite">
      {{ statusText }}
    </p>
    <div class="vph-upd-actions">
      <button type="button" class="vph-button" :disabled="working" @click="$emit('check')">
        Check for update
      </button>
      <button
        v-if="update.available"
        type="button"
        class="vph-button vph-primary"
        :disabled="working"
        @click="$emit('install')"
      >
        Install {{ update.latest }}
      </button>
      <button
        v-if="showReload"
        type="button"
        class="vph-button vph-primary"
        @click="$emit('reload')"
      >
        Reload page
      </button>
      <button v-if="failed" type="button" class="vph-button" :disabled="working" @click="retry">
        Try again
      </button>
      <a v-if="update.latest" class="vph-link" :href="notesUrl" target="_blank" rel="noopener"
        >Release notes</a
      >
    </div>
  </div>
</template>

<script>
const activePhases = ['checking', 'downloading', 'verifying', 'installing'];
const phaseLines = {
  checking: 'Checking for a newer release…',
  downloading: 'Downloading the package…',
  verifying: 'Verifying the signature and hash…',
  installing: 'Installing the update…',
};

export default {
  name: 'WaylineUpdatePanel',
  props: {
    info: { type: Object, default: null },
    compact: Boolean,
    disabled: Boolean,
    waiting: Boolean,
    pageVersion: { type: String, default: '' },
  },
  data() {
    return { now: Date.now() };
  },
  computed: {
    update() {
      const u = this.info || {};
      return {
        current: u.current || '',
        latest: u.latest || null,
        available: !!u.available,
        phase: u.phase || 'idle',
        message: u.message || '',
        failed: !!u.failed || u.phase === 'failed',
        checkedAt: Number(u.checkedAt) || 0,
      };
    },
    active() {
      return activePhases.includes(this.update.phase);
    },
    failed() {
      return this.update.failed;
    },
    working() {
      return this.disabled || (this.active && !this.failed);
    },
    stale() {
      return !!this.pageVersion && this.pageVersion !== this.update.current;
    },
    showReload() {
      return !this.failed && this.update.phase === 'done' && this.stale;
    },
    latestText() {
      return this.update.latest || 'Not checked yet';
    },
    chip() {
      if (this.active)
        return {
          text: this.update.phase === 'checking' ? 'Checking…' : 'Updating…',
          cls: 'vph-muted',
        };
      if (!this.update.latest) return null;
      if (this.update.available) return { text: 'Update available', cls: 'vph-upd-brand' };
      return { text: 'Up to date', cls: '' };
    },
    checkedText() {
      return this.update.checkedAt
        ? 'Last checked ' + this.ago(this.update.checkedAt)
        : 'Never checked';
    },
    compactText() {
      const latest = this.update.latest ? ', latest ' + this.update.latest : ', not checked yet';
      return 'Installed ' + (this.update.current || 'unknown') + latest;
    },
    statusText() {
      if (this.failed) return this.update.message || 'The update failed.';
      if (this.waiting) return 'Waiting for the router to finish…';
      if (this.update.phase === 'done')
        return (
          'Updated to ' +
          (this.update.latest || this.update.current) +
          (this.stale ? '. Reload this page to use it.' : '.')
        );
      return phaseLines[this.update.phase] || '';
    },
    notesUrl() {
      return 'https://github.com/epk/wayline/releases/tag/v' + this.update.latest;
    },
  },
  mounted() {
    this.clock = setInterval(() => (this.now = Date.now()), 30000);
  },
  beforeDestroy() {
    clearInterval(this.clock);
  },
  methods: {
    ago(seconds) {
      const s = Math.max(0, Math.floor(this.now / 1000) - seconds);
      if (s < 60) return 'just now';
      if (s < 3600) return Math.floor(s / 60) + ' min ago';
      if (s < 86400) return Math.floor(s / 3600) + ' h ago';
      return Math.floor(s / 86400) + ' d ago';
    },
    retry() {
      this.$emit(this.update.available ? 'install' : 'check');
    },
  },
};
</script>

<style>
.vph-upd {
  display: flex;
  flex-direction: column;
  gap: 10px;
  margin: 10px 0 4px;
}
.vph-upd p,
.vph-upd dl,
.vph-upd dt,
.vph-upd dd {
  margin: 0;
}
.vph-upd-facts {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 4px 24px;
}
.vph-upd-facts dt {
  font-size: 0.875rem;
}
.vph-upd-value {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 4px 10px;
  font-size: 1rem;
  font-weight: 600;
  overflow-wrap: anywhere;
}
.vph-upd-meta {
  font-size: 0.875rem;
}
.vph-upd-failed {
  color: var(--vph-danger-text);
}
.vph-upd-chip {
  padding: 2px 10px;
  border: 1px solid var(--vph-border);
  border-radius: 999px;
  font-size: 0.8125rem;
  font-weight: 500;
}
.vph-upd-brand {
  background: var(--vph-brand-strong);
  border-color: var(--vph-brand-strong);
  color: var(--vph-inverse);
}
.vph-upd-actions {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px 12px;
}
.vph-upd-compact {
  flex-direction: row;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px 12px;
}
.vph-upd-compact .vph-upd-status {
  flex-basis: 100%;
  order: 3;
}
</style>
