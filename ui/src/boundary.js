import Dashboard from './index.vue';

export default {
  name: 'Wayline',
  props: { api: Object, preview: Boolean },
  data() {
    return { failure: '' };
  },
  errorCaptured(err) {
    this.failure = (err && err.message) || String(err) || 'Unknown error.';
    return false;
  },
  render(h) {
    if (this.failure) {
      return h('main', { class: 'vph' }, [
        h('h1', 'Wayline'),
        h('p', 'The Wayline page failed to draw.'),
        h('p', { class: 'vph-error', attrs: { role: 'alert' } }, this.failure),
        h('p', 'Run wayline-ctl doctor on the router and send the output.'),
      ]);
    }
    return h(Dashboard, { props: { api: this.api, preview: this.preview } });
  },
};
