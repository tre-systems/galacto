// Novation XL3 DAW interface. All messages are volatile MIDI CC/note messages;
// custom modes, firmware, global settings and DIN routing are never written.
export const XL3_ROWS = [69, 72, 73];
export function xl3Pairs(access) {
  const prefix = (name) =>
    name?.match(
      /^(.*(?:LCXL3|Launch Control XL 3).*?)\s+DAW\s+(?:In|Out)$/i,
    )?.[1];
  return [...access.inputs.values()]
    .filter((p) => p.state === "connected" && prefix(p.name))
    .flatMap((input) => {
      const outputs = [...access.outputs.values()].filter(
        (p) => p.state === "connected" && prefix(p.name) === prefix(input.name),
      );
      return outputs.length === 1 ? [{ input, output: outputs[0] }] : [];
    });
}
export class XL3 {
  constructor({
    read,
    change,
    action,
    feedback = () => {},
    schedule = (callback) => setTimeout(callback, 16),
    cancel = (id) => clearTimeout(id),
  }) {
    Object.assign(this, { read, change, action, feedback, schedule, cancel });
    this.pending = null;
    this.forceFeedback = false;
    this.lights = new Map();
    this.generation = 0;
    this.pickup = new Map();
    this.down = new Set();
    this.sent = new Map();
    this.shift = false;
    this.active = false;
  }
  send(bytes) {
    if (this.output?.state === "connected") this.output.send(bytes);
  }
  async connect(pair) {
    this.disconnect();
    const generation = this.generation;
    await pair.input.open();
    if (generation !== this.generation) return false;
    await pair.output.open();
    if (generation !== this.generation) return false;
    this.input = pair.input;
    this.output = pair.output;
    this.active = true;
    this.input.onmidimessage = (event) => this.receive(event.data);
    this.send([0x9f, 12, 127]); // Enable DAW ownership; no SysEx permission needed.
    this.send([0xb6, 30, 2]); // DAW Control surface.
    for (const row of XL3_ROWS) this.send([0xb6, row, 127]);
    this.send([0xb6, 70, 0]); // App owns fader pickup, including scenes and touch edits.
    this.sync(true);
    return true;
  }
  disconnect() {
    this.generation++;
    if (this.pending !== null) this.cancel(this.pending);
    this.pending = null;
    this.forceFeedback = false;
    this.lights.clear();
    if (this.input) this.input.onmidimessage = null;
    if (this.output) {
      for (const row of XL3_ROWS) this.send([0xb6, row, 0]);
      this.send([0x9f, 12, 0]);
    }
    this.input = null;
    this.output = null;
    this.active = false;
    this.shift = false;
    this.pickup.clear();
    this.down.clear();
    this.sent.clear();
    this.feedback({ buttonsReset: true });
  }
  receive(bytes) {
    if (!this.input || !this.output) return;
    const [status, cc, value] = bytes;
    if (
      bytes.length !== 3 ||
      ![status, cc, value].every(Number.isInteger) ||
      cc < 0 ||
      cc > 127 ||
      value < 0 ||
      value > 127
    )
      return;
    if (status === 0xb6 && cc === 30) {
      this.active = value === 1 || value === 2;
      this.pickup.clear();
      this.down.clear();
      this.shift = false;
      this.feedback({ buttonsReset: true });
      if (!this.active && this.pending !== null) {
        this.cancel(this.pending);
        this.pending = null;
      }
      if (this.active) {
        for (const row of XL3_ROWS) this.send([0xb6, row, 127]);
        this.sync(true);
      }
      this.feedback({ mode: value, active: this.active });
      return;
    }
    if (status === 0xb6 && cc === 63) {
      const down = value !== 0;
      if (this.active && this.shift !== down) {
        this.shift = down;
        this.feedback({ button: cc, down });
      }
      return;
    }
    if (!this.active) return;
    if (status === 0xbf && cc >= 77 && cc <= 100) {
      const slot = cc - 77,
        spec = this.read(slot),
        delta = value - 64;
      if (!spec || spec.disabled || !delta) {
        if (spec?.disabled) this.feedback({ slot, reason: spec.disabled });
        return;
      }
      const next = Math.max(
        0,
        Math.min(1, spec.position + delta / (this.shift ? 1270 : 127)),
      );
      this.change(slot, next);
      this.feedback({ slot, picked: true });
      this.sync();
      return;
    }
    if (status === 0xbf && cc >= 5 && cc <= 12) {
      const slot = 24 + cc - 5,
        spec = this.read(slot);
      if (!spec || spec.disabled) return;
      const next = value / 127,
        state = this.pickup.get(slot) || {
          previous: next,
          target: spec.position,
          picked: false,
        };
      // Another surface or a scene can move the target after this fader picked up.
      if (Math.abs(state.target - spec.position) > 1e-7) state.picked = false;
      state.picked ||=
        Math.abs(next - spec.position) <= 1.5 / 127 ||
        (state.previous - spec.position) * (next - spec.position) <= 0;
      state.previous = next;
      if (state.picked) this.change(slot, next);
      state.target = this.read(slot).position;
      this.pickup.set(slot, state);
      this.feedback({
        slot,
        picked: state.picked,
        direction: next < spec.position ? "up" : "down",
      });
      this.sync();
      return;
    }
    if (status === 0xb0) {
      if (value && !this.down.has(cc)) {
        this.down.add(cc);
        this.action(cc, this.shift);
        if (this.active && this.down.has(cc))
          this.feedback({ button: cc, down: true });
      }
      if (!value && this.down.delete(cc))
        this.feedback({ button: cc, down: false });
    }
  }
  sync(force = false) {
    if (!this.active || !this.output) return;
    this.forceFeedback ||= force;
    if (this.pending !== null) return;
    this.pending = this.schedule(() => {
      this.pending = null;
      this.flushFeedback();
    });
  }
  flushFeedback() {
    if (!this.active || !this.output) return;
    const force = this.forceFeedback;
    this.forceFeedback = false;
    for (let slot = 0; slot < 32; slot++) {
      const spec = this.read(slot);
      if (!spec) continue;
      const cc = slot < 24 ? 13 + slot : 5 + slot - 24,
        position = Math.round(spec.position * 127),
        colour = spec.disabled
          ? 0
          : slot < 8
            ? 9
            : slot < 16
              ? 45
              : slot < 24
                ? 21
                : 3;
      // LEDs and encoder targets are state feedback, not performance events.
      const key = cc + ":position";
      if (force || this.sent.get(key) !== position) {
        this.send([0xbf, cc, position]);
        this.sent.set(key, position);
      }
      if (force || this.sent.get(cc) !== colour) {
        this.send([0xb0, cc, colour]);
        this.sent.set(cc, colour);
      }
    }
    for (const [cc, colour] of this.lights)
      if (force || this.sent.get("led:" + cc) !== colour) {
        this.send([0xb0, cc, colour]);
        this.sent.set("led:" + cc, colour);
      }
  }
  led(cc, colour) {
    if (this.lights.get(cc) === colour) return;
    this.lights.set(cc, colour);
    this.sync();
  }
}

// One physical press is one transport request. Loading, export and the full
// fade/tail are protected from repeated presses and delayed MIDI release events.
export function transportIntent({
  running,
  ending,
  starting,
  exporting,
  suspended,
}) {
  if (ending || starting || exporting) return "none";
  return running ? (suspended ? "resume" : "fade") : "start";
}
