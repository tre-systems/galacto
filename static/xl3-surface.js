import { XL3, xl3Pairs } from './xl3.js?v=__CACHE_BUST__';
import { controllerPanel } from './controller-panel.js?v=__CACHE_BUST__';
import { controllerBindings, XL3_PARAMETERS, GALACTO_XL3_LAYOUT } from './controller-bindings.js?v=__CACHE_BUST__';
import { ViewFeedback } from './view-feedback.js?v=__CACHE_BUST__';

export function createXL3Surface({ root, mod, close, standard }) {
  let access, connecting = false, generation = 0, managed = false;
  const staged = new Map();
  const $ = id => document.getElementById(id);
  const event = (el, type) => el.dispatchEvent(new Event(type, { bubbles: true }));
  const snapshot = () => mod.controller_snapshot();
  const available = () => !mod.is_ready() ? 'Waiting for the galaxy' :
    mod.arrangement_active() ? 'Composed playback owns the controls' : false;
  const stageIds = Object.values(XL3_PARAMETERS).slice(0, 4).map(item => item[0]);
  const stageText = (id, p) => id === 'temp-slider' ? (0.5 + p * 2.5).toFixed(1) :
    id === 'gas-slider' ? `${Math.round(p * 50)}%` : id === 'bulge-slider' ? `${Math.round(p * 60)}%` :
    `${Math.max(256, Math.round(p * 163840 / 256) * 256).toLocaleString()}`;
  // Fine encoder steps should reach the same DOM handlers without rounding to
  // a whole percent. No initial values or preset data are written here.
  for (const [id] of Object.values(XL3_PARAMETERS)) $(id).step = '0.01';
  const bindings = controllerBindings({
    available,
    camera: snapshot,
    setCamera(axis, value) {
      mod.controller_camera(axis, value);
      $('autopilot-toggle').checked = false;
    },
    parameter(id, readout) {
      const position = staged.get(id) ?? Number($(id).value) / 100;
      return { position, text: staged.has(id) ? stageText(id, position) :
        readout ? $(readout).textContent : id === 'volume-slider' ? `${Math.round(position * 100)}%` : `${(position * 2).toFixed(2)}×` };
    },
    setParameter(id, value) {
      if (stageIds.includes(id)) staged.set(id, value);
      else { $(id).value = value * 100; event($(id), 'input'); }
    },
  });
  const paint = new ViewFeedback(() => panel.refresh());
  const driver = new XL3({
    ...bindings,
    change(slot, value) {
      if (!document.hidden) { bindings.change(slot, value); invalidate(); }
    },
    action(cc, shift) { if (!document.hidden) action(cc, shift); },
    feedback(e) {
      if (e.buttonsReset) panel.resetPresses();
      if (e.button !== undefined) panel.press(e.button, e.down);
      if (e.slot !== undefined) panel.focus(e.slot, e.reason ||
        (e.picked === false ? `Move fader ${e.direction} to pick up` : undefined));
      if (e.mode !== undefined) panel.message(e.active ? 'DAW Control ready' : 'Select DAW Control on the XL3');
      paint.schedule();
    },
  });
  const panel = controllerPanel(root, {
    ...bindings, change(slot, value) { bindings.change(slot, value); invalidate(); },
    action, connect, disconnect, layout: GALACTO_XL3_LAYOUT, onClose: close,
  });
  function toggle(id) { const el = $(id); el.checked = !el.checked; event(el, 'change'); }
  function action(cc, shift = false, down = true) {
    if (!down) return;
    if (cc === 104) return close();
    if (cc === 52) return standard();
    if (cc === 106 || cc === 107) return panel.row(cc === 106 ? -1 : 1);
    if (available()) { panel.message(available()); return; }
    if (cc === 116) {
      managed = true;
      const s = snapshot();
      if (s[6]) return; // repeated presses cannot restart a fade
      if (!mod.controller_arm()) { panel.message('Audio is unavailable'); return; }
      if (s[5] && s[7]) mod.controller_fade();
      else mod.controller_play();
    } else if (cc >= 37 && cc <= 39) toggle(['autopilot-toggle', 'halo-show', 'rc-toggle'][cc - 37]);
    else if (cc === 40) $('mute-btn').click();
    else if (cc === 41) mod.controller_pause(!snapshot()[3]);
    else if (cc === 42) { mod.controller_home(); $('autopilot-toggle').checked = false; }
    else if (cc === 43) {
      if (!staged.size) return;
      for (const [id, value] of staged) { $(id).value = value * 100; event($(id), 'input'); }
      event($('count-slider'), 'change'); // one reseed, even when count is unchanged
      staged.clear();
    } else if (cc === 44) {
      if (shift) mod.restart();
      else panel.message('Hold Shift to restart the galaxy');
    } else if (cc >= 45 && cc <= 51) {
      staged.clear();
      $('scenario-select').value = cc - 45;
      mod.set_scenario(cc - 45); // keep the XL3 panel open
    }
    invalidate();
  }
  async function connect() {
    if (connecting) return;
    if (available()) { panel.message(available()); return; }
    if (!navigator.requestMIDIAccess) { panel.message('Web MIDI needs Chrome or Edge on this computer'); return; }
    connecting = true;
    panel.connecting(true);
    const attempt = ++generation;
    try {
      managed = true;
      if (!mod.controller_arm()) throw new Error('Audio unavailable');
      const next = await navigator.requestMIDIAccess({ sysex: false });
      if (attempt !== generation) return;
      access?.removeEventListener('statechange', connectionChanged);
      access = next;
      access.addEventListener('statechange', connectionChanged);
      const pairs = xl3Pairs(access);
      if (pairs.length !== 1) {
        disconnect();
        panel.message(pairs.length ? 'Connect one XL3 at a time' : 'No XL3 DAW port found. Connect it by USB, then retry.');
        return;
      }
      if (await driver.connect(pairs[0]) && attempt === generation) {
        panel.connected(pairs[0].input.name);
        panel.message('Ready · Play starts; press again to fade');
        invalidate();
      }
    } catch (error) {
      if (attempt !== generation) return;
      disconnect();
      panel.message(error.name === 'NotAllowedError' ? 'MIDI permission not granted' : 'Connection failed. Check audio permission and USB, then retry.');
    } finally { connecting = false; panel.connecting(false); }
  }
  function connectionChanged() {
    if (driver.input && (driver.input.state !== 'connected' || driver.output?.state !== 'connected')) {
      disconnect(); panel.message('XL3 disconnected · reconnect when ready');
    }
  }
  function disconnect() {
    generation++;
    access?.removeEventListener('statechange', connectionChanged);
    access = null;
    driver.disconnect();
    panel.connected(null);
  }
  function invalidate() {
    const s = snapshot(), locked = !!available();
    const states = {
      37: !!s[4], 38: $('halo-show').checked, 39: $('rc-toggle').checked,
      40: $('mute-btn').getAttribute('aria-pressed') === 'true', 41: !!s[3],
      43: staged.size > 0, 116: !!s[5],
    };
    for (let i = 0; i < 7; i++) states[45 + i] = Number($('scenario-select').value) === i;
    for (const [, cc] of GALACTO_XL3_LAYOUT.buttons) {
      panel.setKey(cc, { pressed: !!states[cc], disabled: ![52, 104, 106, 107].includes(cc) &&
        (locked || (cc === 116 && !!s[6]) || (cc === 43 && !staged.size)),
        ...(cc === 116 ? { label: s[6] ? 'Fading…' : s[5] ? '↘' : '▶' } : {}) });
      driver.led(cc, states[cc] ? 21 : 1);
    }
    driver.sync();
    paint.schedule();
  }
  // Keep Space on panel buttons from also toggling the simulation's document shortcut.
  root.addEventListener('keydown', event => event.stopPropagation());
  for (const type of ['input', 'change', 'click']) $('controls').addEventListener(type, invalidate);
  $('scenario-select').addEventListener('change', () => { staged.clear(); invalidate(); });
  document.addEventListener('visibilitychange', () => {
    if (document.hidden) {
      driver.pickup.clear(); driver.down.clear(); driver.shift = false; panel.resetPresses();
    }
  });
  window.addEventListener('pagehide', disconnect);
  // Audio-clock fades and autopilot continue independently of panel rendering.
  const timer = setInterval(() => { if (!document.hidden && (panel.open || driver.input)) invalidate(); }, 150);
  window.addEventListener('pagehide', () => clearInterval(timer), { once: true });
  invalidate();
  return { panel, invalidate, disconnect, get managed() { return managed; } };
}
