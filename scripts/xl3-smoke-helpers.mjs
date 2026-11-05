// Isolated MIDI ports: browser tests never claim the owner's physical device.
export function xl3Probe() {
  return `(() => {
    const input = { name: 'LCXL3 1 DAW Out', state: 'connected', open: async () => {} };
    const output = { name: 'LCXL3 1 DAW In', state: 'connected', open: async () => {}, send: b => sent.push(Array.from(b)) };
    const sent = [], access = new EventTarget();
    access.inputs = new Map([[1, input]]); access.outputs = new Map([[1, output]]);
    Object.defineProperty(navigator, 'requestMIDIAccess', { configurable: true, value: async options => {
      if (options.sysex) throw Error('Unexpected SysEx');
      return access;
    }});
    window.__xl3 = { input, output, sent, access,
      send: (...data) => input.onmidimessage?.({ data: new Uint8Array(data) }) };
  })();`;
}

export async function exerciseXL3(page) {
  async function check(expression) {
    const result = await page.send('Runtime.evaluate', {
      expression: `(async () => { const assert = (ok, reason) => { if (!ok) throw Error(reason); };
        const sleep = ms => new Promise(r => setTimeout(r, ms));
        const wait = async predicate => { const end = performance.now()+4000; while (!predicate() && performance.now()<end) await sleep(50); };
        const state = () => window.galacto.controllerState();
        const send = (...data) => window.__xl3.send(...data);
        const press = cc => { send(0xb0, cc, 127); send(0xb0, cc, 0); };
        ${expression}
        return 'pass'; })()`, awaitPromise: true, returnByValue: true, userGesture: true,
    });
    if (result.exceptionDetails || result.result?.value !== 'pass')
      throw Error(`XL3 smoke: ${result.result?.description || JSON.stringify(result)}`);
  }
  await check(`
    await window.galacto.whenReady();
    const until = performance.now() + 5000;
    while (!document.querySelector('.xl3-grid') && performance.now() < until) await sleep(50);
    assert(document.querySelectorAll('.xl3-cell').length === 32, 'hardware grid');
    window.__xl3Defaults = Array.from(document.querySelectorAll('#controls input, #controls select')).map(el => [el.id, el.value, el.checked]);
    assert(document.getElementById('scenario-select').value === '6', 'default M51');
    assert(document.getElementById('volume-slider').value === '32', 'default volume');
    assert(document.getElementById('autopilot-toggle').checked, 'default autopilot');
    document.querySelector('[data-connect]').click();
    await sleep(200);
    assert(document.querySelector('#xl3-connection').textContent.includes('DAW'), 'connect');
    assert(state()[5] === 0 && state()[8] === 0, 'connect arms silently');
    const after = Array.from(document.querySelectorAll('#controls input, #controls select')).map(el => [el.id, el.value, el.checked]);
    assert(JSON.stringify(after) === JSON.stringify(window.__xl3Defaults), 'connect preserves settings');
    assert(window.__xl3.sent.every(m => m[0] !== 0xf0), 'no SysEx');
    for (const [i, id] of ['speed-slider','gravity-slider','halo-slider','halo-size-slider','size-slider','glow-slider','autopilot-speed','volume-slider'].entries()) {
      send(0xbf, 5+i, 0); send(0xbf, 5+i, 127);
      assert(Number(document.getElementById(id).value) > 99, 'fader ' + id);
    }
    for (const [id, value] of window.__xl3Defaults) {
      const el = document.getElementById(id);
      if (el?.type === 'range') { el.value = value; el.dispatchEvent(new Event('input', {bubbles:true})); }
    }
    const temp = document.getElementById('temp-slider').value;
    send(0xbf,77,70);
    assert(document.getElementById('temp-slider').value === temp, 'setup knob stages, no live reseed');
    send(0xbf,80,0); // stage minimum body count for a bounded GPU smoke
    press(43);
    assert(document.getElementById('temp-slider').value !== temp, 'Apply commits staged Q');
    assert(Number(document.getElementById('count-slider').value) === 0, 'Apply commits body count');
    for (const [i, cc] of [88,89,90].entries()) {
      const old = state()[i]; send(0xbf,cc,65);
      assert(Math.abs(state()[i]-old) > 0.001, 'camera knob ' + i);
    }
    assert(!document.getElementById('autopilot-toggle').checked, 'manual camera hands off autopilot');
    press(41); assert(state()[3] === 1, 'pause'); press(41);
    assert(state()[3] === 0, 'resume');
    press(38); assert(document.getElementById('halo-show').checked, 'halo button'); press(38);
    send(0xb0,39,127);
    assert(document.querySelector('[data-cc="39"]').classList.contains('xl3-key-down'), 'held highlight');
    send(0xb0,39,0); await sleep(220);
    assert(!document.querySelector('[data-cc="39"]').classList.contains('xl3-key-down'), 'release highlight');
    press(39);
    for (let i=0;i<7;i++) { press(45+i); assert(Number(document.getElementById('scenario-select').value) === i, 'scenario ' + i); }
    press(52);
    assert(document.getElementById('xl3-panel').hidden, 'standard layout switch');
    document.querySelector('#controls [data-interface="xl3"]').click();
    assert(!document.getElementById('xl3-panel').hidden, 'XL3 layout switch');
    press(104); assert(document.getElementById('xl3-panel').hidden, 'Hide');
    const gravity = document.getElementById('gravity-slider').value;
    send(0xbf,6,0); send(0xbf,6,127);
    assert(document.getElementById('gravity-slider').value !== gravity, 'hidden panel still controls');
    document.getElementById('controls-toggle').click();
    press(116); await wait(() => state()[8] > 0);
    assert(state()[5] === 1 && state()[8] > 0, 'Play starts sound: ' + JSON.stringify(state()));
    press(116); assert(state()[6] === 1, 'second Play fades');
    press(116); assert(state()[6] === 1, 'repeat cannot interrupt fade');
    const fadeStart = state()[8];
    send(0xbf,12,0); send(0xbf,12,127); // volume writes must not cancel the fade
    await sleep(1500);
    assert(state()[8] > 0 && state()[8] < fadeStart, 'fade is gradual despite volume writes');
    await sleep(7000);
    assert(state()[5] === 0 && state()[6] === 0 && state()[8] === 0, 'fade reaches exact silence');
    press(116); await wait(() => state()[8] > 0); assert(state()[5] === 1 && state()[8] > 0, 'restart after fade');
    press(116); await sleep(8500);
    const body = document.getElementById('count-slider').value;
    window.galacto.startArrangement(60, 123);
    send(0xbf,80,127); press(43); press(116);
    assert(document.getElementById('count-slider').value === body && state()[5] === 0, 'composed capture locked');
    window.galacto.stopArrangement();
    window.__xl3.input.state = 'disconnected'; window.__xl3.access.dispatchEvent(new Event('statechange'));
    assert(window.__xl3.input.onmidimessage === null, 'hotplug releases input');
    assert(document.querySelector('#xl3-connection').textContent === 'Not connected', 'hotplug status');
  `);
  await page.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 1, mobile: false });
  await check(`
    await sleep(100);
    const panel = document.getElementById('xl3-panel'), r = panel.getBoundingClientRect();
    assert(r.left >= 0 && r.right <= innerWidth && r.bottom <= innerHeight, 'phone panel fits viewport');
    assert(document.documentElement.scrollWidth <= innerWidth, 'no page-wide overflow');
    const scroll = document.querySelector('.xl3-scroll');
    assert(scroll.scrollWidth > scroll.clientWidth, 'hardware grid scrolls within panel');
    scroll.scrollLeft = scroll.scrollWidth;
    assert(scroll.scrollLeft > 0, 'last fader reachable');
    const close = panel.querySelector('[data-close]').getBoundingClientRect();
    assert(close.left >= 0 && close.right <= innerWidth, 'close reachable');
  `);
  console.log('smoke: XL3 live WASM controls, silent arm, fade, presets, capture lock, hotplug and phone layout pass');
}
