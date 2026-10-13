import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { runInNewContext } from 'node:vm';

const source = readFileSync(new URL('../static/sentry.js', import.meta.url), 'utf8');

function harness({ failCapture = false } = {}) {
  const messages = [];
  let script;
  const window = {
    GALACTO_SENTRY_CONFIG: { dsn: 'https://public@example.invalid/1' },
    location: { origin: 'https://example.invalid' },
    Sentry: {
      init() {},
      captureMessage(message, options) {
        if (failCapture) throw new Error('telemetry unavailable');
        messages.push({ message, options });
      },
    },
  };
  const document = {
    createElement: () => ({}),
    head: { appendChild: (value) => { script = value; } },
  };
  runInNewContext(source, { window, document, navigator: { onLine: true }, URL });
  return { window, messages, load: () => script.onload() };
}

test('an early rejected map is reported once when monitoring loads', () => {
  const { window, messages, load } = harness();
  window.galactoReportGpuReadbackFailure(16384, 1024);
  assert.equal(messages.length, 0);
  load();
  window.galactoReportGpuReadbackFailure(32768, 2048);
  assert.equal(messages.length, 1);
  assert.equal(messages[0].options.level, 'warning');
  assert.deepEqual(JSON.parse(JSON.stringify(messages[0].options.contexts)), {
    gpu_readback: { backend: 'browser_webgpu', phase: 'map_async', particles: 16384, bytes: 1024 },
  });
});

test('only validated numeric dimensions reach GPU diagnostics', () => {
  const { window, messages, load } = harness();
  load();
  for (const [count, bytes] of [['private input', 16], [NaN, 16], [1, 16], [257, 16], [163841, 16], [256, 'private input'], [256, 1024]]) {
    window.galactoReportGpuReadbackFailure(count, bytes);
  }
  assert.equal(messages.length, 0);
  window.galactoReportGpuReadbackFailure(256, 16);
  assert.equal(messages.length, 1);
});

test('unavailable telemetry does not interrupt readback recovery', () => {
  const { window, load } = harness({ failCapture: true });
  load();
  assert.doesNotThrow(() => window.galactoReportGpuReadbackFailure(256, 16));
});
