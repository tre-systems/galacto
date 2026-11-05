import { test } from "node:test";
import assert from "node:assert/strict";
import { XL3, xl3Pairs, transportIntent } from "../static/xl3.js";
import {
  controllerBindings,
  XL3_PARAMETERS,
  GALACTO_XL3_LAYOUT,
} from "../static/controller-bindings.js";

function rig() {
  const values = Array(32).fill(0.5),
    changes = [],
    actions = [],
    feedback = [],
    sent = [];
  const input = { state: "connected", open: async () => {} },
    output = {
      state: "connected",
      open: async () => {},
      send: (b) => sent.push(b),
    };
  const c = new XL3({
    read: (slot) => ({ position: values[slot] }),
    change: (slot, v) => {
      values[slot] = v;
      changes.push([slot, v]);
    },
    action: (...a) => actions.push(a),
    feedback: (e) => feedback.push(e),
  });
  return { c, values, changes, actions, feedback, sent, input, output };
}
test("XL3 claims only one matching named DAW input/output pair", () => {
  const port = (name, state = "connected") => ({ name, state });
  const access = {
    inputs: new Map([
      [1, port("LCXL3 1 MIDI Out")],
      [2, port("LCXL3 1 DAW Out")],
      [3, port("Other DAW Out")],
    ]),
    outputs: new Map([
      [1, port("LCXL3 1 DAW In")],
      [2, port("Other DAW In")],
    ]),
  };
  assert.equal(xl3Pairs(access).length, 1);
  assert.equal(xl3Pairs(access)[0].input.name, "LCXL3 1 DAW Out");
  access.outputs.set(3, port("LCXL3 1 DAW Out"));
  assert.equal(xl3Pairs(access).length, 0);
});
test("DAW setup is volatile, all 24 encoders respond, wrong channels and echoes do not", async () => {
  const r = rig();
  await r.c.connect(r);
  assert.ok(r.sent.some((m) => String(m) === "159,12,127"));
  for (let slot = 0; slot < 24; slot++) r.c.receive([0xbf, 77 + slot, 65]);
  assert.equal(r.changes.length, 24);
  assert.ok(r.values.slice(0, 24).every((v) => v > 0.5));
  for (const msg of [
    [0xbe, 77, 65],
    [0xbf, 13, 127],
    [0xbf, 77, 64],
    [0xbf, 77, 128],
    [0xbf, 77, -1],
    [0xbf, 77, 2.5],
    [0xbf, 77, 65, 1],
  ])
    r.c.receive(msg);
  assert.equal(r.changes.length, 24);
  assert.ok(r.sent.every((m) => m.length === 3 && m[0] !== 0xf0));
  r.c.disconnect();
  assert.deepEqual(r.sent.at(-1), [0x9f, 12, 0]);
  assert.equal(r.input.onmidimessage, null);
});
test("physical faders keep pickup after relative knob movement or scene changes", async () => {
  const r = rig();
  await r.c.connect(r);
  r.c.receive([0xbf, 5, 0]);
  assert.equal(r.changes.length, 0);
  r.c.receive([0xbf, 5, 70]);
  assert.equal(r.changes.length, 1);
  r.values[24] = 0.2;
  r.c.receive([0xbf, 5, 71]);
  assert.equal(r.changes.length, 1);
  assert.equal(r.feedback.at(-1).picked, false);
  r.c.receive([0xbf, 5, 20]);
  assert.equal(r.changes.length, 2);
  r.c.receive([0xb6, 30, 29]);
  r.c.receive([0xbf, 5, 60]);
  assert.equal(r.changes.length, 2);
  r.c.receive([0xb6, 30, 2]);
  r.c.receive([0xbf, 5, 61]);
  assert.equal(r.changes.length, 2);
});
test("button release and held repeats never double transport; Shift gives fine steps", async () => {
  const r = rig();
  await r.c.connect(r);
  r.c.receive([0xb0, 116, 127]);
  r.c.receive([0xb0, 116, 127]);
  r.c.receive([0xb0, 116, 0]);
  r.c.receive([0xb0, 116, 127]);
  assert.deepEqual(r.actions, [
    [116, false],
    [116, false],
  ]);
  r.c.receive([0xb6, 63, 127]);
  r.c.receive([0xbf, 77, 65]);
  assert.ok(Math.abs(r.values[0] - 0.5 - 1 / 1270) < 1e-10);
  r.c.receive([0xbf, 77, 63]);
  assert.equal(r.values[0], 0.5);
  for (let n = 0; n < 1000; n++) r.c.receive([0xbf, 77, 0]);
  assert.equal(r.values[0], 0);
});
test("every mapped button reports press and release without repeating its action; Shift is feedback only", async () => {
  const r = rig();
  await r.c.connect(r);
  r.feedback.length = 0;
  const buttons = GALACTO_XL3_LAYOUT.buttons.map(([, cc]) => cc);
  for (const cc of buttons) {
    r.c.receive([0xb0, cc, 127]);
    r.c.receive([0xb0, cc, 127]);
    r.c.receive([0xb0, cc, 0]);
    r.c.receive([0xb0, cc, 0]);
  }
  assert.deepEqual(
    r.actions,
    buttons.map((cc) => [cc, false]),
  );
  assert.deepEqual(
    r.feedback,
    buttons.flatMap((button) => [
      { button, down: true },
      { button, down: false },
    ]),
  );
  r.feedback.length = 0;
  for (const value of [127, 127, 0, 0]) r.c.receive([0xb6, 63, value]);
  assert.deepEqual(r.feedback, [
    { button: 63, down: true },
    { button: 63, down: false },
  ]);
  assert.equal(r.actions.length, buttons.length);
  r.c.disconnect();
});
test("mode changes and disconnect reset held button feedback without dispatching extra actions", async () => {
  const r = rig();
  await r.c.connect(r);
  r.c.receive([0xb0, 37, 127]);
  r.c.receive([0xb6, 63, 127]);
  r.feedback.length = 0;
  r.c.receive([0xb6, 30, 29]);
  assert.deepEqual(r.feedback, [
    { buttonsReset: true },
    { mode: 29, active: false },
  ]);
  assert.equal(r.c.shift, false);
  r.feedback.length = 0;
  r.c.receive([0xb0, 37, 0]);
  r.c.receive([0xb6, 63, 127]);
  assert.deepEqual(r.feedback, []);
  r.c.receive([0xb6, 30, 2]);
  r.c.receive([0xb0, 37, 127]);
  assert.deepEqual(r.actions, [
    [37, false],
    [37, false],
  ]);
  r.c.disconnect();
  assert.deepEqual(r.feedback.at(-1), { buttonsReset: true });
  assert.equal(r.c.down.size, 0);
});
test("transport covers idle, running, fade, export, double start and browser interruption", () => {
  assert.equal(transportIntent({}), "start");
  assert.equal(transportIntent({ running: true }), "fade");
  assert.equal(transportIntent({ running: true, suspended: true }), "resume");
  for (const state of [
    { ending: true },
    { starting: true },
    { exporting: true },
    { running: true, ending: true, suspended: true },
  ])
    assert.equal(transportIntent(state), "none");
});


test('Galacto maps eight distinct live faders and keeps camera on knobs', () => {
  const writes = [], positions = [0.3, 0.5, 0.7];
  let locked = false;
  const b = controllerBindings({
    parameter: () => ({ position: 0.4, text: 'value' }),
    setParameter: (...args) => writes.push(args), camera: () => positions,
    setCamera: (...args) => writes.push(['camera', ...args]), available: () => locked,
  });
  const ids = Object.keys(XL3_PARAMETERS).map(Number).filter(slot => slot >= 24).map(slot => XL3_PARAMETERS[slot][0]);
  assert.equal(ids.length, 8);
  assert.equal(new Set(ids).size, 8);
  assert.equal(writes.length, 0);
  for (let slot = 24; slot < 32; slot++) assert.equal(b.change(slot, 0.8), true);
  assert.deepEqual(writes.map(write => write[0]), ids);
  for (let slot = 11; slot <= 13; slot++) {
    assert.equal(b.read(slot).position, positions[slot - 11]);
    b.change(slot, 0.9);
    assert.deepEqual(writes.at(-1), ['camera', slot - 11, 0.9]);
  }
  for (const slot of [4, 8, 10, 14, 23, 32, -1, 1.5]) assert.equal(b.change(slot, 0.5), false);
  assert.equal(b.change(24, NaN), false);
  const count = writes.length;
  locked = 'Composed playback';
  assert.equal(b.change(24, 0.2), false);
  assert.equal(b.change(11, 0.2), false);
  assert.equal(writes.length, count);
  assert.equal(b.read(24).disabled, locked);
});
