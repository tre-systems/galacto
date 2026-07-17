// Browser bootstrap and DOM wiring for Galacto's WebGPU/WASM app.
const MAX_PARTICLE_COUNT = 163840;
const PARTICLE_COUNT_STEP = 256;
let activeParticleCount = MAX_PARTICLE_COUNT / 10;

const assetVersion = new URL(import.meta.url).searchParams.get("v");
const assetVersionSuffix = assetVersion ? `?v=${encodeURIComponent(assetVersion)}` : "";

function warmAppShellCache() {
  if (!assetVersion || !("caches" in window)) return;
  caches
    .open(`galacto-${assetVersion}`)
    .then((cache) => cache.add(`./app.js${assetVersionSuffix}`).catch(() => {}))
    .catch(() => {});
}

function countForSlider(slider) {
  return Math.max(
    PARTICLE_COUNT_STEP,
    Math.round(((slider.value / 100) * MAX_PARTICLE_COUNT) / PARTICLE_COUNT_STEP) *
      PARTICLE_COUNT_STEP,
  );
}

async function checkWebGPUSupport() {
  const ua = navigator.userAgent || "";
  if (/\b(FxiOS|CriOS|EdgiOS)\b/i.test(ua)) {
    throw new Error(
      "Galacto needs Safari's current WebGPU implementation on iPhone and iPad. Firefox, Chrome, and Edge on iOS still expose WebGPU unreliably for this app.",
    );
  }
  if (!navigator.gpu) {
    throw new Error("WebGPU not supported in this browser");
  }
  const adapter = await navigator.gpu.requestAdapter().catch(() => null);
  if (!adapter) {
    throw new Error("No WebGPU adapter found");
  }
  return true;
}

function showRuntimeNotice(title, message) {
  const notice = document.getElementById("runtime-notice");
  const titleEl = document.getElementById("runtime-notice-title");
  const messageEl = document.getElementById("runtime-notice-message");
  if (!notice || notice.dataset.dismissed === "1") return;

  titleEl.textContent = title;
  messageEl.textContent = message;
  notice.hidden = false;
}

function setupRuntimeNotice() {
  document.getElementById("runtime-notice-dismiss")?.addEventListener("click", () => {
    const notice = document.getElementById("runtime-notice");
    notice.dataset.dismissed = "1";
    notice.hidden = true;
  });
}

function safeWasmCall(label, fn, options = {}) {
  try {
    return fn();
  } catch (error) {
    console.warn(`${label} is unavailable in this browser context`, error);
    showRuntimeNotice(
      options.title || `${label} is unavailable`,
      options.message ||
        "Galacto can keep showing the simulation, but this control is not supported by this browser or device.",
    );
    if (typeof options.onError === "function") options.onError(error);
    return options.fallback;
  }
}

// Star glow halo extent - reshapes the billboard falloff and colours audio,
// but never touches the simulation.
function setupGlowControl(mod) {
  const slider = document.getElementById("glow-slider");
  const readout = document.getElementById("glow-readout");
  const apply = () => {
    const g = slider.value / 100;
    mod.set_glow(g);
    readout.textContent = (1 + 2 * g).toFixed(1) + "×";
  };
  slider.addEventListener("input", apply);
  apply();
}

// Cinematic autopilot: the engine drives the camera by default; an actual
// camera gesture (drag, pinch, or scroll) hands control back and flips the
// toggle. A plain tap on the canvas is also how the mobile panel is tucked
// away, so don't treat pointerdown alone as manual camera control.
function setupAutopilot(mod) {
  const toggle = document.getElementById("autopilot-toggle");
  const speed = document.getElementById("autopilot-speed");
  const canvas = document.getElementById("gpu-canvas");
  const disableAutopilot = () => {
    toggle.checked = false;
    toggle.disabled = true;
    speed.disabled = true;
  };
  const applySpeed = () =>
    safeWasmCall(
      "Autopilot",
      () => mod.set_autopilot_speed((speed.value / 100) * 2.0),
      { onError: disableAutopilot },
    );
  safeWasmCall("Autopilot", () => mod.set_autopilot(toggle.checked), {
    onError: disableAutopilot,
  });
  applySpeed();
  toggle.addEventListener("change", () =>
    safeWasmCall("Autopilot", () => mod.set_autopilot(toggle.checked), {
      onError: disableAutopilot,
    }),
  );
  speed.addEventListener("input", applySpeed);
  const DRAG_RELEASE_PX = 8;
  let pointerStart = null;
  let touchStart = null;
  const release = () => {
    if (!toggle.checked) return;
    toggle.checked = false;
    safeWasmCall("Autopilot", () => mod.set_autopilot(false), {
      onError: disableAutopilot,
    });
  };
  const movedFarEnough = (start, x, y) =>
    Math.hypot(x - start.x, y - start.y) >= DRAG_RELEASE_PX;

  canvas.addEventListener("pointerdown", (e) => {
    pointerStart = { id: e.pointerId, x: e.clientX, y: e.clientY };
  });
  canvas.addEventListener("pointermove", (e) => {
    if (!pointerStart || e.pointerId !== pointerStart.id) return;
    if (movedFarEnough(pointerStart, e.clientX, e.clientY)) {
      pointerStart = null;
      release();
    }
  });
  const endPointer = (e) => {
    if (pointerStart && e.pointerId === pointerStart.id) pointerStart = null;
  };
  canvas.addEventListener("pointerup", endPointer);
  canvas.addEventListener("pointercancel", endPointer);

  canvas.addEventListener("touchstart", (e) => {
    if (e.touches.length >= 2) {
      touchStart = null;
      release();
      return;
    }
    const t = e.touches[0];
    touchStart = t ? { x: t.clientX, y: t.clientY } : null;
  });
  canvas.addEventListener("touchmove", (e) => {
    if (e.touches.length >= 2) {
      touchStart = null;
      release();
      return;
    }
    const t = e.touches[0];
    if (touchStart && t && movedFarEnough(touchStart, t.clientX, t.clientY)) {
      touchStart = null;
      release();
    }
  });
  canvas.addEventListener("touchend", () => {
    touchStart = null;
  });
  canvas.addEventListener("touchcancel", () => {
    touchStart = null;
  });
  canvas.addEventListener("wheel", release, { passive: true });
}

// Fade the whole control panel away after a spell of no interaction, for a
// clean, movie-like view; any pointer move, key, or scroll brings it back.
function setupIdleHide() {
  const controls = document.getElementById("controls");
  const IDLE_MS = 20000;
  let last = performance.now();
  const wake = () => {
    last = performance.now();
    controls.classList.remove("idle-hidden");
  };
  ["pointermove", "pointerdown", "keydown", "wheel"].forEach((ev) =>
    window.addEventListener(ev, wake, { passive: true }),
  );
  setInterval(() => {
    if (performance.now() - last > IDLE_MS)
      controls.classList.add("idle-hidden");
  }, 1000);
}

async function init() {
  const loadingEl = document.getElementById("loading");
  const errorEl = document.getElementById("error");
  const errorDetailsEl = document.getElementById("error-details");

  try {
    await checkWebGPUSupport();
    const mod = await import(`./galacto.js${assetVersionSuffix}`);
    // Load the wasm with the same per-deploy ?v= as the glue JS. Without
    // this the wasm URL is unversioned, so a service worker can serve a
    // stale cached wasm against fresh glue after a deploy (a version
    // mismatch that breaks init); versioning keeps the pair in lockstep.
    await mod.default({ module_or_path: `./galacto_bg.wasm${assetVersionSuffix}` });
    loadingEl.style.display = "none";
    const panel = setupControlPanel();
    setupScenarioControl(mod, panel.collapse);
    setupParticleCountControl(mod);
    setupSpeedControl(mod);
    setupTempControl(mod);
    setupGasFractionControl(mod);
    setupBulgeControl(mod);
    setupGravityControl(mod);
    setupHaloControl(mod);
    setupHaloConcentrationControl(mod);
    setupHaloProfileControl(mod);
    setupHaloVisibility(mod);
    setupRotationCurve(mod);
    setupStarSizeControl(mod);
    setupGlowControl(mod);
    setupVolumeControl(mod);
    setupMuteButton(mod);
    setupRestartButton(mod);
    setupRuntimeNotice();
    setupAutoSound(mod);
    setupInfoButtons();
    setupAutopilot(mod);
    setupIdleHide();
    // ?compose=SEED&dur=SECONDS auto-plays the deterministic cinematic
    // arrangement (drives camera + galaxy through the arc). The video-capture
    // script loads this so the captured picture matches the audio rendered from
    // generate_piece with the same seed/duration.
    // The engine inits asynchronously (WebGPU adapter/device), so the setters
    // no-op until it's live. Wait for is_ready() before driving the engine.
    const whenReady = async (timeoutMs = 20000) => {
      const start = performance.now();
      while (!mod.is_ready() && performance.now() - start < timeoutMs) {
        await new Promise((r) => requestAnimationFrame(r));
      }
      return mod.is_ready();
    };
    const composeParams = new URLSearchParams(location.search);
    if (composeParams.has("compose")) {
      const seed = parseInt(composeParams.get("compose"), 10) || 1;
      const dur = parseFloat(composeParams.get("dur")) || 240;
      // ?particles=N renders the piece at a higher body count for a denser,
      // more detailed galaxy (the sim self-throttles its step rate to hold the
      // frame rate, so the motion just evolves more slowly).
      const particles = parseInt(composeParams.get("particles"), 10);
      whenReady().then(() => {
        if (Number.isFinite(particles) && particles > 0) {
          activeParticleCount = particles;
          mod.set_particle_count(particles);
        }
        mod.start_arrangement(dur, seed);
      });
    }
    // Headless control surface for the automated production pipeline
    // (scripts/produce.mjs) - render a piece's audio and drive the arrangement
    // without the UI. Harmless to expose; it's just the offline engine.
    window.galacto = {
      isReady: () => mod.is_ready(),
      whenReady,
      startArrangement: (dur, seed) => mod.start_arrangement(dur, seed),
      stopArrangement: () => mod.stop_arrangement(),
      arrangementActive: () => mod.arrangement_active(),
      setParticleCount: (n) => mod.set_particle_count(n),
      fps: () => mod.fps(),
      // Render a composed piece's audio and POST the WAV bytes to `postUrl`,
      // so the headless capture run can save it to disk. Returns the report.
      renderPieceTo: async (postUrl, dur, seed, lufs) => {
        const res = await mod.generate_piece(dur, seed, lufs);
        await fetch(postUrl, { method: "POST", body: new Blob([res.wav]) });
        return res.report;
      },
    };
  } catch (error) {
    console.error("Failed to initialize application:", error);
    loadingEl.style.display = "none";
    errorEl.style.display = "block";
    errorDetailsEl.replaceChildren();
    const detail = document.createElement("p");
    const label = document.createElement("strong");
    label.textContent = "Error:";
    detail.append(label, " ", error.message || String(error));
    errorDetailsEl.append(detail);
  }
}

// Switch the initial-condition scenario; the sim re-seeds immediately.
// Picking a new simulation also closes the panel so the fresh galaxy is
// unobstructed.
function setupScenarioControl(mod, collapse) {
  const sel = document.getElementById("scenario-select");
  sel.addEventListener("change", () => {
    mod.set_scenario(parseInt(sel.value, 10));
    collapse();
  });
}

// Body-count slider (0–100) maps linearly to the number of bodies, so the
// default (16,384) sits at 10% and the top is 10× (163,840). Changing it
// re-seeds the sim (like a restart) - heavy, and at the top end the all-pairs
// gravity is ~100× the work - so the sim applies on release ('change'), not
// while dragging; the readout and audio preview update live.
function setupParticleCountControl(mod) {
  const slider = document.getElementById("count-slider");
  const readout = document.getElementById("count-readout");
  const fmt = (n) => (n >= 1000 ? +(n / 1000).toFixed(1) + "k" : String(n));
  const show = () => {
    const count = countForSlider(slider);
    mod.stage_particle_count(count);
    readout.textContent = fmt(count);
  };
  slider.addEventListener("input", show);
  slider.addEventListener("change", () => {
    activeParticleCount = countForSlider(slider);
    mod.set_particle_count(activeParticleCount);
  });
  show();
}

// Switch the dark-matter halo profile; the sim re-seeds so the disk starts
// balanced against it (logarithmic = a flat rotation curve that confines the
// system; NFW = a rising-then-falling curve whose fast debris can escape).
function setupHaloProfileControl(mod) {
  const sel = document.getElementById("halo-select");
  sel.addEventListener("change", () => {
    mod.set_halo_profile(parseInt(sel.value, 10));
  });
}

// Toggle the dark-matter halo overlay - a soft violet cloud, sized to the
// active profile's scale radius, drawn behind the stars. Off by default.
function setupHaloVisibility(mod) {
  const toggle = document.getElementById("halo-show");
  const apply = () => mod.set_halo_visible(toggle.checked);
  toggle.addEventListener("change", apply);
  apply();
}

// Rotation-curve overlay: a small chart of circular speed v(r) decomposed into
// disk + bulge + dark-matter halo (km/s vs kpc), from the WASM in physical
// units. The flat outer curve held up by the halo is the classic observational
// clue behind dark matter - drag the Halo/Gravity sliders and watch it respond.
// Off by default (the toggle lives in the Dark matter halo group); a clock
// shows the elapsed simulated time. All physics comes from the sim - JS only
// draws.
function setupRotationCurve(mod) {
  const wrap = document.getElementById("rotcurve");
  const toggle = document.getElementById("rc-toggle");
  const canvas = document.getElementById("rc-canvas");
  const timeEl = document.getElementById("rc-time");
  const ctx = canvas.getContext("2d");
  const W = 240, H = 150;
  const dpr = window.devicePixelRatio || 1;
  canvas.style.width = W + "px";
  canvas.style.height = H + "px";
  canvas.width = Math.round(W * dpr);
  canvas.height = Math.round(H * dpr);
  ctx.scale(dpr, dpr);
  const COL = {
    total: "#ffffff",
    disk: "#7aa2f7",
    bulge: "#ffd479",
    halo: "#b18cff",
    axis: "rgba(199,210,254,0.4)",
    grid: "rgba(255,255,255,0.07)",
  };

  const draw = () => {
    const flat = mod.rotation_curve(56);
    if (!flat || flat.length < 10) return;
    const n = flat.length / 5;
    const R = [], comp = { bulge: [], disk: [], halo: [], total: [] };
    let maxR = 0, maxV = 0;
    for (let i = 0; i < n; i++) {
      const o = i * 5;
      R.push(flat[o]);
      comp.bulge.push(flat[o + 1]);
      comp.disk.push(flat[o + 2]);
      comp.halo.push(flat[o + 3]);
      comp.total.push(flat[o + 4]);
      if (flat[o] > maxR) maxR = flat[o];
      if (flat[o + 4] > maxV) maxV = flat[o + 4];
    }
    const padL = 32, padR = 8, padT = 8, padB = 18;
    const pw = W - padL - padR, ph = H - padT - padB;
    const vStep = maxV > 360 ? 200 : maxV > 180 ? 100 : 50;
    const vMax = Math.max(vStep, Math.ceil((maxV * 1.1) / vStep) * vStep);
    const X = (r) => padL + (r / maxR) * pw;
    const Y = (v) => padT + ph - (v / vMax) * ph;
    ctx.clearRect(0, 0, W, H);

    // y gridlines + km/s labels
    ctx.font = "9px system-ui, sans-serif";
    ctx.textBaseline = "middle";
    ctx.textAlign = "right";
    for (let v = 0; v <= vMax + 0.5; v += vStep) {
      const y = Y(v);
      ctx.strokeStyle = COL.grid;
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(padL, y);
      ctx.lineTo(W - padR, y);
      ctx.stroke();
      ctx.fillStyle = COL.axis;
      ctx.fillText(String(v), padL - 4, y);
    }
    // x label
    ctx.textAlign = "center";
    ctx.textBaseline = "bottom";
    ctx.fillStyle = COL.axis;
    ctx.fillText("radius (kpc)", padL + pw / 2, H);
    // y unit
    ctx.save();
    ctx.translate(8, padT + ph / 2);
    ctx.rotate(-Math.PI / 2);
    ctx.textBaseline = "top";
    ctx.fillText("km/s", 0, 0);
    ctx.restore();

    const line = (arr, color, dash, width) => {
      ctx.beginPath();
      ctx.setLineDash(dash);
      ctx.lineWidth = width;
      ctx.strokeStyle = color;
      for (let i = 0; i < n; i++) {
        const x = X(R[i]), y = Y(arr[i]);
        i ? ctx.lineTo(x, y) : ctx.moveTo(x, y);
      }
      ctx.stroke();
    };
    line(comp.bulge, COL.bulge, [3, 2], 1);
    line(comp.disk, COL.disk, [3, 2], 1);
    line(comp.halo, COL.halo, [3, 2], 1);
    line(comp.total, COL.total, [], 2);
    ctx.setLineDash([]);
  };

  let rafId = null;
  const tick = () => {
    const myr = mod.elapsed_myr();
    timeEl.textContent =
      myr < 1000 ? Math.round(myr) + " Myr" : (myr / 1000).toFixed(2) + " Gyr";
    rafId = requestAnimationFrame(tick);
  };
  const setShown = (on) => {
    wrap.hidden = !on;
    if (on) {
      draw();
      if (rafId === null) tick();
    } else if (rafId !== null) {
      cancelAnimationFrame(rafId);
      rafId = null;
    }
  };
  toggle.addEventListener("change", () => setShown(toggle.checked));

  // The curve depends on the live gravity, halo strength/size/model, and the
  // bulge fraction - redraw when any of those change (only while on screen).
  // (Bulge re-seeds on release, but the staged value drives the curve live.)
  const redraw = () => {
    if (!wrap.hidden) draw();
  };
  document.getElementById("gravity-slider").addEventListener("input", redraw);
  document.getElementById("halo-slider").addEventListener("input", redraw);
  document.getElementById("halo-size-slider").addEventListener("input", redraw);
  document.getElementById("halo-select").addEventListener("change", redraw);
  document.getElementById("bulge-slider").addEventListener("input", redraw);
}

// Map the speed slider (0–100) to a multiplier on a logarithmic scale
// (0.25× … 8×), push it to the sim, and show the current value.
function setupSpeedControl(mod) {
  const slider = document.getElementById("speed-slider");
  const countSlider = document.getElementById("count-slider");
  const readout = document.getElementById("speed-readout");
  const MIN = 0.25;
  const MAX = 8;
  // Megayears of simulated time per real second at the current speed: at ×1
  // the sim advances one time-unit per real second (see the accumulator).
  const myrPerSecAt1 = mod.myr_per_unit_time();
  const apply = () => {
    const t = slider.value / 100;
    const requested = MIN * Math.pow(MAX / MIN, t);
    const capCount = Math.max(activeParticleCount, countForSlider(countSlider));
    const speedCap = mod.max_speed_for_particle_count(capCount);
    const speed = Math.min(requested, speedCap);
    mod.set_speed(speed);
    readout.textContent =
      Math.round(speed * myrPerSecAt1) + " Myr/s" + (speed < requested ? " max" : "");
  };
  slider.addEventListener("input", apply);
  countSlider.addEventListener("input", apply);
  countSlider.addEventListener("change", apply);
  apply();
}

// The control panel: a discreet ⚙ button opens it, and the whole panel is
// draggable by that handle (mouse or touch) so it never has to cover the
// visuals. A press that doesn't move
// toggles it; a press that moves drags it.
function setupControlPanel() {
  const panel = document.getElementById("controls");
  const handle = document.getElementById("controls-toggle");

  const setCollapsed = (collapsed) => {
    panel.classList.toggle("collapsed", collapsed);
    handle.setAttribute("aria-expanded", String(!collapsed));
    handle.title = collapsed
      ? "Show controls (drag to move)"
      : "Hide controls (drag to move)";
  };

  const moveTo = (left, top) => {
    panel.style.left = left + "px";
    panel.style.top = top + "px";
    panel.style.right = "auto";
    panel.style.bottom = "auto";
    panel.style.transform = "none";
  };

  const clampIntoView = () => {
    if (!panel.style.left) return; // still at the CSS default (top-left)
    const r = panel.getBoundingClientRect();
    moveTo(
      Math.max(4, Math.min(parseFloat(panel.style.left), window.innerWidth - r.width - 4)),
      Math.max(4, Math.min(parseFloat(panel.style.top), window.innerHeight - r.height - 4))
    );
  };

  // The cog always starts in the top-left (the CSS default); a drag only
  // repositions it for the current session and is not persisted.
  let startX, startY, baseLeft, baseTop, dragging = false, moved = false;
  handle.addEventListener("pointerdown", (e) => {
    dragging = true;
    moved = false;
    const r = panel.getBoundingClientRect();
    baseLeft = r.left;
    baseTop = r.top;
    startX = e.clientX;
    startY = e.clientY;
    handle.setPointerCapture(e.pointerId);
  });
  handle.addEventListener("pointermove", (e) => {
    if (!dragging) return;
    const dx = e.clientX - startX;
    const dy = e.clientY - startY;
    if (!moved && Math.hypot(dx, dy) > 4) moved = true;
    if (!moved) return;
    moveTo(baseLeft + dx, baseTop + dy);
    clampIntoView();
  });
  const endDrag = (e) => {
    if (!dragging) return;
    dragging = false;
    try {
      handle.releasePointerCapture(e.pointerId);
    } catch {}
  };
  handle.addEventListener("pointerup", endDrag);
  handle.addEventListener("pointercancel", endDrag);

  // A press that didn't move is a click → toggle. (A fresh pointerdown
  // resets `moved`, so a drag never leaves the toggle stuck.)
  handle.addEventListener("click", () => {
    if (moved) {
      moved = false;
      return;
    }
    setCollapsed(!panel.classList.contains("collapsed"));
    clampIntoView();
  });

  window.addEventListener("resize", clampIntoView);

  // A press anywhere outside the open panel tucks it away (clicks on the
  // canvas, etc.). Presses inside - toggle, sliders, links - are ignored.
  document.addEventListener("pointerdown", (e) => {
    if (!panel.classList.contains("collapsed") && !panel.contains(e.target)) {
      setCollapsed(true);
    }
  });

  // Expose a collapse so other controls (e.g. switching simulation) can
  // tuck the panel away.
  return { collapse: () => setCollapsed(true) };
}

// A small "?" after each control's label that reveals a plain-language
// description on click. Generated from one map (rather than hand-marked-up
// rows) so the copy lives in one place; one shared popover is reused.
function setupInfoButtons() {
  const INFO = {
    "scenario-select":
      "The initial setup. A lone spiral disk, or two-or-more galaxies on a collision course - the merger variants and an M51-style flyby. Switching it re-seeds the simulation.",
    "autopilot-toggle":
      "Lets the camera fly itself - a slow orbit and a gentle glide in and out, so the simulation plays like a movie with its soundtrack. The slider beside it sets how fast it drifts. Drag, pinch, or scroll the view and it switches off; turn it back on any time.",
    "count-slider":
      "How many stars are simulated (16k–164k). Per-body mass scales as 1/N, so more bodies refine the same galaxy rather than adding mass. Gravity is all-pairs (O(N²)), so high counts cap speed to keep the browser responsive. Re-seeds.",
    "speed-slider":
      "How fast the simulation runs (0.25×–8×), shown as millions of years of simulated time per real second. It doesn't change the physics - a fixed timestep keeps it frame-rate independent - and high body counts cap it automatically.",
    "gas-slider":
      "How much of the disk is cold, star-forming gas (0–50%) - the blue component that dissipates its random motion and gathers into the spiral arms. More gas means brighter, better-defined blue arms and a brighter, airier sound. Re-seeds on release.",
    "bulge-slider":
      "The central bulge's share of the galaxy's mass (0–60%). A small bulge is a disk-dominated late-type spiral with a gently-rising rotation curve; a large one is a bulge-dominated early type with a sharp inner peak and fuller sound. Re-seeds on release.",
    "temp-slider":
      "Disk stability, the Toomre Q parameter. Below ~1 the disk fragments into clumps; ~1–2 it swing-amplifies into spiral arms; above ~2 it stays a smooth disk. Applied on Restart.",
    "gravity-slider":
      "Scales the strength of gravity (0.25×–4×) on the running sim. Turn it up and the galaxy collapses inward; down and it loosens and disperses.",
    "halo-select":
      "Dark-matter halo profile. Logarithmic gives a flat outer rotation curve and keeps everything bound; NFW (the cold-dark-matter shape) rises then falls and lets fast debris escape. Re-seeds.",
    "halo-slider":
      "How much dark matter - the characteristic circular speed in km/s (0–2× the default 220). Turn on the Curve and watch the flat outer part rise as you raise this. It holds the disk together and shapes the tidal tails.",
    "halo-size-slider":
      "The halo's scale radius (in kpc) - how spread-out the dark matter is. Smaller = a concentrated halo with a steeply-rising inner curve; larger = a diffuse halo with a gentler rise. Reshapes the rotation curve live, no re-seed.",
    "halo-show":
      "Overlays the otherwise-invisible dark-matter halo as a soft violet cloud, sized to the active profile's scale radius.",
    "rc-toggle":
      "Plots the rotation curve - circular speed vs radius (km/s vs kpc) - split into disk, bulge, and dark-matter halo. The flat outer part is the classic observational clue behind dark matter.",
    "size-slider":
      "On-screen size of each star. Larger looks brighter and more diffuse, smaller is sharper. It never touches the physics, but it lightly colours the soundscape.",
    "glow-slider":
      "How far each star's soft halo reaches around its bright central point. The core stays crisp; turning this up spreads a fainter glow further out and opens the soundscape brightness/echo. It never touches the simulation or its speed.",
    "volume-slider":
      "Loudness of the generative soundscape, which is synthesized live and driven by the galaxy's own core dynamics.",
  };

  const pop = document.createElement("div");
  pop.id = "info-popover";
  pop.setAttribute("role", "tooltip");
  pop.hidden = true;
  document.getElementById("controls").appendChild(pop);

  let openBtn = null;
  const close = () => {
    if (openBtn) openBtn.setAttribute("aria-expanded", "false");
    openBtn = null;
    pop.hidden = true;
  };
  const open = (btn) => {
    if (openBtn && openBtn !== btn)
      openBtn.setAttribute("aria-expanded", "false");
    pop.textContent = btn.dataset.info;
    pop.hidden = false;
    btn.setAttribute("aria-expanded", "true");
    openBtn = btn;
    // Anchor under the icon, flip above if it would overflow, clamp to view.
    pop.style.left = "0px";
    pop.style.top = "0px";
    const r = btn.getBoundingClientRect();
    const pw = pop.offsetWidth;
    const ph = pop.offsetHeight;
    const left = Math.max(8, Math.min(r.left, window.innerWidth - pw - 8));
    let top = r.bottom + 6;
    if (top + ph > window.innerHeight - 8) top = r.top - ph - 6;
    pop.style.left = left + "px";
    pop.style.top = top + "px";
  };

  for (const [id, text] of Object.entries(INFO)) {
    const ctrl = document.getElementById(id);
    const row = ctrl && ctrl.closest(".control");
    const label = row && row.querySelector("label");
    if (!label) continue;
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "info";
    btn.textContent = "?";
    btn.dataset.info = text;
    btn.setAttribute("aria-expanded", "false");
    btn.setAttribute("aria-label", "About " + label.textContent.trim());
    label.insertAdjacentElement("afterend", btn);
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      openBtn === btn ? close() : open(btn);
    });
  }

  // Dismiss on any press that isn't another info button, on Escape, and on
  // resize (the anchor would otherwise go stale).
  document.addEventListener("pointerdown", (e) => {
    const onInfo = e.target.closest && e.target.closest(".info");
    if (!pop.hidden && !onInfo) close();
  });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") close();
  });
  window.addEventListener("resize", close);
}

// Disk stability slider (0–100) → Toomre Q (0.5 … 3.0): ≲1 fragments into
// clumps, ~1–2 swing-amplifies into spiral arms, ≫2 stays smooth. A seed-time
// property, so it's staged and applied on the next Restart or scenario switch.
function setupTempControl(mod) {
  const slider = document.getElementById("temp-slider");
  const readout = document.getElementById("temp-readout");
  const MIN = 0.5;
  const MAX = 3.0;
  const apply = () => {
    const q = MIN + (slider.value / 100) * (MAX - MIN);
    mod.set_disk_temperature(q);
    readout.textContent = q.toFixed(2);
  };
  slider.addEventListener("input", apply);
  apply();
}

// Gas-fraction slider (0–100) → 0 … 50% of the disk seeded as cold gas. A
// seed-time property like the body count: audio/readout track the drag, and
// releasing re-seeds (so a drag doesn't re-seed on every tick).
function setupGasFractionControl(mod) {
  const slider = document.getElementById("gas-slider");
  const readout = document.getElementById("gas-readout");
  const MAX = 0.5; // up to 50% gas
  const fracFor = () => (slider.value / 100) * MAX;
  const apply = () => {
    const frac = fracFor();
    mod.stage_gas_fraction(frac);
    readout.textContent = Math.round(frac * 100) + "%";
  };
  slider.addEventListener("input", apply);
  slider.addEventListener("change", () => mod.set_gas_fraction(fracFor()));
  apply();
}

// Bulge slider (0–100) → 0 … 60% bulge mass fraction. Seed-time like the gas
// and body-count sliders: audio/readout track the drag, releasing re-seeds.
function setupBulgeControl(mod) {
  const slider = document.getElementById("bulge-slider");
  const readout = document.getElementById("bulge-readout");
  const MAX = 0.6;
  const fracFor = () => (slider.value / 100) * MAX;
  const apply = () => {
    const frac = fracFor();
    mod.stage_bulge_fraction(frac);
    readout.textContent = Math.round(frac * 100) + "%";
  };
  slider.addEventListener("input", apply);
  slider.addEventListener("change", () => mod.set_bulge_fraction(fracFor()));
  apply();
}

// Gravity slider (0–100) → log scale (0.25× … 4×). Live: rewrites the params
// uniform, so the running galaxy collapses or disperses immediately.
function setupGravityControl(mod) {
  const slider = document.getElementById("gravity-slider");
  const readout = document.getElementById("gravity-readout");
  const MIN = 0.25;
  const MAX = 4.0;
  const apply = () => {
    const g = MIN * Math.pow(MAX / MIN, slider.value / 100);
    mod.set_gravity(g);
    readout.textContent = g.toFixed(2) + "×";
  };
  slider.addEventListener("input", apply);
  apply();
}

// Dark-matter halo slider (0–100) → 0 … 2× the default confining pull. Live:
// turn it down and debris drifts off; turn it up and the system compresses.
function setupHaloControl(mod) {
  const slider = document.getElementById("halo-slider");
  const readout = document.getElementById("halo-readout");
  const DEFAULT = 75; // matches the Rust HALO_V0 default (value 50 → 1×)
  const apply = () => {
    const halo = (slider.value / 50) * DEFAULT;
    mod.set_halo(halo);
    // Characteristic circular speed in km/s (220 km/s at ×1 - the units.rs
    // anchor): the flat outer rotation speed the halo holds up.
    readout.textContent = Math.round(halo * (220 / DEFAULT)) + " km/s";
  };
  slider.addEventListener("input", apply);
  apply();
}

// Dark-matter halo concentration: the scale-radius multiplier (0.4 … 2.5×,
// log). Live - reshapes the halo force and the rotation curve without
// re-seeding. The readout is the scale radius in kpc, which differs by profile
// (≈15 kpc for the logarithmic core, ≈7 kpc for NFW), so it also refreshes
// when the model changes.
function setupHaloConcentrationControl(mod) {
  const slider = document.getElementById("halo-size-slider");
  const readout = document.getElementById("halo-size-readout");
  const select = document.getElementById("halo-select");
  const MIN = 0.4;
  const MAX = 2.5;
  const scaleFor = () => MIN * Math.pow(MAX / MIN, slider.value / 100);
  const baseKpc = () => (select.value === "1" ? 7 : 15);
  const apply = () => {
    const scale = scaleFor();
    mod.set_halo_concentration(scale);
    readout.textContent = (baseKpc() * scale).toFixed(1) + " kpc";
  };
  slider.addEventListener("input", apply);
  select.addEventListener("change", apply);
  apply();
}

// Star-size slider (0–100) → 0.006 … 0.026 NDC half-extent (value 50 → 1×).
// Live: how big/glowy each star appears, with a small audio brightness cue.
function setupStarSizeControl(mod) {
  const slider = document.getElementById("size-slider");
  const readout = document.getElementById("size-readout");
  const MIN = 0.006;
  const MAX = 0.026;
  const DEFAULT = 0.016;
  const apply = () => {
    const size = MIN + (slider.value / 100) * (MAX - MIN);
    mod.set_particle_size(size);
    readout.textContent = (size / DEFAULT).toFixed(2) + "×";
  };
  slider.addEventListener("input", apply);
  apply();
}

// Volume slider (0–100) → 0…1 on a squared (perceptual) taper, so the
// slider's lower half does the fine, audible work. Defaults below full so
// the soundscape starts gently; the readout shows the slider position.
function setupVolumeControl(mod) {
  const slider = document.getElementById("volume-slider");
  const apply = () => {
    const t = slider.value / 100;
    mod.set_volume(t * t);
  };
  slider.addEventListener("input", apply);
  apply();
}

// Mute button: toggles the whole soundscape silent without touching the
// volume setting, so unmuting restores the chosen level. The icon and ARIA
// state track the toggle.
function setupMuteButton(mod) {
  const btn = document.getElementById("mute-btn");
  let muted = false;
  const apply = () => {
    btn.textContent = muted ? "🔇" : "🔊";
    btn.setAttribute("aria-pressed", String(muted));
    const label = muted ? "Unmute" : "Mute";
    btn.setAttribute("aria-label", label);
    btn.title = label;
    mod.set_muted(muted);
  };
  btn.addEventListener("click", () => {
    muted = !muted;
    apply();
  });
  apply();
}

// Restart: re-seed the current scenario with the staged disk temperature.
function setupRestartButton(mod) {
  document
    .getElementById("restart-btn")
    .addEventListener("click", () => mod.restart());
}

// Start the generative soundscape automatically. It's synthesized in the
// WASM (no audio files) and driven by the visuals - scenario, zoom,
// rotation, and the sim knobs. Browsers block audio until a user gesture,
// so we arm a one-shot listener for the visitor's first interaction
// (pointer / key / touch / wheel) - which, with a galaxy there to be
// dragged and zoomed, happens within moments - and boot the audio engine
// inside that gesture so the AudioContext is allowed to start.
function setupAutoSound(mod) {
  // iOS is fussy: the AudioContext often stays *suspended* after the first
  // touch (Safari tends to unlock audio on touchend/click, not touchstart). So
  // rather than a one-shot, re-assert the start on every interaction and only
  // stop once the context is genuinely running.
  const events = [
    "pointerdown",
    "pointerup",
    "touchend",
    "mousedown",
    "click",
    "keydown",
    "wheel",
  ];
  // iOS routes Web Audio to the silent-switch-respecting "ambient" session by
  // default, so an installed PWA stays silent whenever the ring/silent switch
  // is on. Ask for the "playback" session so the soundscape plays like a media
  // app - through the silent switch. (Safari/WebKit only; harmless elsewhere.)
  const claimPlayback = () => {
    try {
      if (navigator.audioSession) navigator.audioSession.type = "playback";
    } catch (_) {}
  };
  let soundUnavailable = false;
  const markSoundUnavailable = () => {
    soundUnavailable = true;
    const muteButton = document.getElementById("mute-btn");
    if (muteButton) {
      muteButton.disabled = true;
      muteButton.title = "Sound is unavailable in this browser";
      muteButton.setAttribute("aria-label", "Sound unavailable");
    }
  };
  const tryStart = () => {
    if (soundUnavailable) return;
    claimPlayback();
    const running = safeWasmCall(
      "Sound",
      () => {
        mod.set_sound_enabled(true); // builds the engine (first time), then resumes
        return mod.audio_running();
      },
      {
        fallback: false,
        title: "Sound is unavailable",
        message:
          "This browser or device could not start Galacto's Web Audio engine. The visual simulation can still run.",
        onError: markSoundUnavailable,
      },
    );
    if (running) {
      events.forEach((e) =>
        window.removeEventListener(e, tryStart, { capture: true })
      );
    }
  };
  events.forEach((e) =>
    window.addEventListener(e, tryStart, { capture: true, passive: true })
  );
  // iOS suspends the AudioContext when the PWA is backgrounded; re-assert the
  // session and resume it when we return to the foreground.
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden && !soundUnavailable) {
      claimPlayback();
      safeWasmCall("Sound", () => mod.resume_audio(), {
        title: "Sound is unavailable",
        message:
          "This browser or device could not resume Galacto's Web Audio engine. The visual simulation can still run.",
        onError: markSoundUnavailable,
      });
    }
  });
}

// Reveal the "update available" toast and wire its Reload button to take
// the waiting service worker live (which triggers the reload below).
function showUpdateToast(reg) {
  const toast = document.getElementById("update-toast");
  if (!toast || toast.dataset.shown === "1") return; // show at most once
  toast.dataset.shown = "1";
  toast.hidden = false;
  // Force a reflow so the transition runs, then slide it in.
  void toast.offsetWidth;
  toast.classList.add("show");
  document.getElementById("update-reload").addEventListener(
    "click",
    (e) => {
      e.currentTarget.disabled = true;
      e.currentTarget.textContent = "Updating…";
      if (reg.waiting) window.galactoPwa.activateWaitingServiceWorker(reg.waiting);
    },
    { once: true },
  );
  const dismiss = document.getElementById("update-dismiss");
  dismiss.addEventListener("click", () => {
    const deferred = toast.classList.toggle("deferred");
    dismiss.textContent = deferred ? "Update ready" : "✕";
    dismiss.setAttribute(
      "aria-label",
      deferred ? "Update ready. Show update options" : "Show update options later",
    );
    dismiss.title = deferred ? "Show update options" : "Later";
  });
}

// Register the service worker so the app installs and launches offline, and
// surface an update prompt when a new version is deployed. Best-effort - the
// sim runs fine without it (and it's a no-op where unsupported). The ?v=
// keeps the worker fresh per deploy, matching the cache-busted shell.
if ("serviceWorker" in navigator) {
  window.addEventListener("load", async () => {
    try {
      const pwa = await import(`./pwa-update.js${assetVersionSuffix}`);
      window.galactoPwa = pwa;
      warmAppShellCache();
      const swUrl = `./sw.js${assetVersionSuffix}`;
      const reg = await navigator.serviceWorker.register(swUrl);
      let checking = false;
      let lastCheckAt = 0;
      const checkForUpdate = async (force = false) => {
        if (
          checking ||
          !navigator.onLine ||
          document.visibilityState !== "visible" ||
          (!force && !pwa.shouldCheckForUpdate(Date.now(), lastCheckAt))
        ) {
          return;
        }
        checking = true;
        lastCheckAt = Date.now();
        try {
          await pwa.checkForServiceWorkerUpdate(reg, swUrl);
          if (reg.waiting && navigator.serviceWorker.controller) showUpdateToast(reg);
        } catch {
          // Update checks are best-effort; the simulation remains available offline.
        } finally {
          checking = false;
        }
      };
      pwa.installUpdateCheckTriggers(() => void checkForUpdate());

      // A new version already finished installing and is waiting.
      if (reg.waiting && navigator.serviceWorker.controller) showUpdateToast(reg);

      // A new version arrives while the page is open.
      reg.addEventListener("updatefound", () => {
        const installing = reg.installing;
        if (!installing) return;
        installing.addEventListener("statechange", () => {
          if (installing.state === "installed" && navigator.serviceWorker.controller) {
            showUpdateToast(reg);
          }
        });
      });

      await checkForUpdate(true);
    } catch (e) {
      console.warn("Service worker registration failed:", e);
    }
  });
}

init().catch(console.error);
