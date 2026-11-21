import { useSyncExternalStore } from 'react';

let version = 0;
let queued = false;
const listeners = new Set<() => void>();

const notify = (): void => {
  version++;
  if (queued) return;
  queued = true;
  queueMicrotask(() => {
    queued = false;
    listeners.forEach((listener) => listener());
  });
};

export interface Signal<T> {
  value: T;
}

export function signal<T>(initial: T): Signal<T> {
  let current = initial;
  return {
    get value() {
      return current;
    },
    set value(next: T) {
      if (Object.is(next, current)) return;
      current = next;
      notify();
    },
  };
}

export function computed<T>(read: () => T): { readonly value: T } {
  let at = -1;
  let cached: T;
  return {
    get value() {
      if (at !== version) {
        cached = read();
        at = version;
      }
      return cached;
    },
  };
}

const subscribe = (listener: () => void): (() => void) => {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
};

const snapshot = (): number => version;

export const useStore = (): number => useSyncExternalStore(subscribe, snapshot, snapshot);
