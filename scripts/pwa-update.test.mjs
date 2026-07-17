import assert from 'node:assert/strict';
import test from 'node:test';
import {
  activateWaitingServiceWorker,
  checkForServiceWorkerUpdate,
  installUpdateCheckTriggers,
  shouldCheckForUpdate,
} from '../static/pwa-update.js';

test('update checks use a cooldown', () => {
  assert.equal(shouldCheckForUpdate(1, 0), true);
  assert.equal(shouldCheckForUpdate(60_000, 1), false);
  assert.equal(shouldCheckForUpdate(60_001, 1), true);
});

test('worker discovery bypasses browser caches', async () => {
  let fetchOptions;
  let updateCalls = 0;
  await checkForServiceWorkerUpdate(
    { update: async () => updateCalls++ },
    '/sw.js?v=test',
    async (_url, options) => {
      fetchOptions = options;
      return { ok: true };
    },
  );
  assert.equal(fetchOptions.cache, 'no-store');
  assert.equal(updateCalls, 1);
});

test('foreground triggers are installed and cleaned up', () => {
  const windowTarget = new EventTarget();
  const documentTarget = new EventTarget();
  let checks = 0;
  const cleanup = installUpdateCheckTriggers(() => checks++, {
    windowTarget,
    documentTarget,
    isVisible: () => true,
    isOnline: () => true,
    setIntervalFn: () => 1,
    clearIntervalFn: () => {},
  });
  windowTarget.dispatchEvent(new Event('pageshow'));
  documentTarget.dispatchEvent(new Event('visibilitychange'));
  assert.equal(checks, 2);
  cleanup();
  windowTarget.dispatchEvent(new Event('focus'));
  assert.equal(checks, 2);
});

test('the selected waiting worker activates with a reload fallback', () => {
  const worker = new EventTarget();
  const container = new EventTarget();
  let message;
  let reloads = 0;
  let fallback;
  worker.postMessage = (value) => {
    message = value;
  };
  activateWaitingServiceWorker(worker, {
    serviceWorkerContainer: container,
    reload: () => reloads++,
    setTimeoutFn: (callback) => {
      fallback = callback;
      return 1;
    },
    clearTimeoutFn: () => {},
  });
  assert.deepEqual(message, { type: 'SKIP_WAITING' });
  fallback();
  assert.equal(reloads, 1);
});
