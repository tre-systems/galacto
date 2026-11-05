// A view of the physical surface. The app adapter is the only parameter owner.
export function controllerPanel(
  root,
  {
    read,
    change,
    action,
    connect,
    disconnect,
    layout = { buttons: [] },
    onClose,
  },
) {
  const document = root.ownerDocument;
  const heading = document.createElement("header");
  heading.innerHTML =
    '<div><strong>Launch Control XL 3</strong><span id="xl3-connection">Not connected</span></div><button type="button" data-connect>Connect</button><button type="button" data-close aria-label="Close controller">×</button>';
  root.append(heading);
  const switcher = root.querySelector(":scope > .interface-switch");
  if (switcher)
    heading.insertBefore(switcher, heading.querySelector("[data-connect]"));
  heading.querySelector("[data-connect]").addEventListener("click", connect);
  heading
    .querySelector("[data-close]")
    .addEventListener("click", () => (onClose ? onClose() : show(false)));
  const frame = document.createElement("div");
  frame.className = "xl3-scroll";
  const board = document.createElement("div");
  board.className = "xl3-board";
  frame.append(board);
  root.append(frame);
  const left = document.createElement("aside");
  left.className = "xl3-left";
  board.append(left);
  const display = document.createElement("output");
  display.className = "xl3-readout";
  display.textContent = "Ready";
  left.append(display);
  const keys = new Map();
  function button(parent, cc, label, title = label) {
    const b = document.createElement("button");
    b.type = "button";
    b.textContent = label;
    b.title = title;
    b.setAttribute("aria-label", "Controller " + title);
    b.dataset.cc = cc;
    if (layout.momentary?.includes(cc)) {
      let held = false;
      const down = () => {
        if (!held) {
          held = true;
          action(cc, false, true);
          press(cc, true);
        }
      };
      const up = () => {
        if (held) {
          held = false;
          action(cc, false, false);
          press(cc, false);
        }
      };
      b.addEventListener("pointerdown", (event) => {
        event.preventDefault();
        b.focus();
        b.setPointerCapture(event.pointerId);
        down();
      });
      for (const name of [
        "pointerup",
        "pointercancel",
        "lostpointercapture",
        "blur",
      ])
        b.addEventListener(name, up);
      b.addEventListener("keydown", (event) => {
        if ([" ", "Enter"].includes(event.key)) {
          event.preventDefault();
          down();
        }
      });
      b.addEventListener("keyup", (event) => {
        if ([" ", "Enter"].includes(event.key)) {
          event.preventDefault();
          up();
        }
      });
      root.ownerDocument.defaultView.addEventListener("blur", up);
      root.ownerDocument.addEventListener("visibilitychange", () => {
        if (root.ownerDocument.hidden) up();
      });
    } else
      b.addEventListener("click", (event) => action(cc, event.shiftKey, true));
    parent.append(b);
    keys.set(cc, b);
    return b;
  }
  const pages = document.createElement("div");
  pages.className = "xl3-nav";
  left.append(pages);
  const tracks = document.createElement("div");
  tracks.className = "xl3-nav";
  left.append(tracks);
  const grid = document.createElement("div");
  grid.className = "xl3-grid";
  board.append(grid);
  const cells = [];
  for (let slot = 0; slot < 32; slot++) {
    const spec = read(slot),
      cell = document.createElement("label");
    cell.className = "xl3-cell " + (slot < 24 ? "xl3-knob" : "xl3-fader");
    cell.dataset.row = Math.floor(slot / 8);
    cell.style.setProperty(
      "--row-colour",
      slot < 8
        ? "#d7b281"
        : slot < 16
          ? "#a9b8df"
          : slot < 24
            ? "#80b9ae"
            : "#c3cbc6",
    );
    const name = document.createElement("span");
    name.className = "xl3-label";
    name.textContent = spec.label;
    const output = document.createElement("output");
    output.textContent = spec.text;
    const input = document.createElement("input");
    input.type = "range";
    input.min = 0;
    input.max = 1;
    input.step = 0.001;
    input.value = spec.position;
    input.setAttribute("aria-label", "Controller " + spec.label);
    const dial = document.createElement("span");
    dial.className = "xl3-dial";
    dial.setAttribute("aria-hidden", "true");
    cell.append(name, dial, input, output);
    grid.append(cell);
    input.addEventListener("input", () => {
      change(slot, Number(input.value));
      focus(slot);
    });
    input.addEventListener("focus", () => focus(slot));
    input.addEventListener("pointerenter", () => focus(slot));
    let drag;
    dial.addEventListener("pointerdown", (event) => {
      if (input.disabled) return;
      event.preventDefault();
      input.focus();
      focus(slot);
      drag = {
        x: event.clientX,
        y: event.clientY,
        value: read(slot).position,
        id: event.pointerId,
      };
      dial.setPointerCapture(event.pointerId);
    });
    dial.addEventListener("pointermove", (event) => {
      if (!drag) return;
      const position =
        drag.value +
        (drag.y - event.clientY + event.clientX - drag.x) /
          (event.shiftKey ? 1600 : 160);
      change(slot, Math.max(0, Math.min(1, position)));
      focus(slot);
    });
    const endDrag = () => {
      drag = null;
    };
    dial.addEventListener("pointerup", endDrag);
    dial.addEventListener("pointercancel", endDrag);
    dial.addEventListener("lostpointercapture", endDrag);
    cells.push({ cell, input, output, name });
  }
  const groups = { pages, tracks, left, grid };
  for (const [group, cc, label, title] of layout.buttons)
    button(groups[group], cc, label, title);
  const fine = document.createElement("small");
  fine.className = "xl3-shift";
  fine.dataset.cc = 63;
  fine.textContent = "Shift · fine adjustment";
  left.append(fine);
  keys.set(63, fine);
  if (layout.summary) {
    const note = document.createElement("p");
    note.className = "xl3-note";
    note.textContent = layout.summary;
    root.append(note);
  }
  let selected = layout.initialSlot ?? 0,
    paintedSelection = -1,
    readout = "Ready";
  const keyStates = new Map(),
    pressedKeys = new Set(),
    releaseTimers = new Map();
  function paintPress(cc) {
    if (!root.hidden)
      keys.get(cc)?.classList.toggle("xl3-key-down", pressedKeys.has(cc));
  }
  function press(cc, down) {
    if (!keys.has(cc)) return;
    clearTimeout(releaseTimers.get(cc));
    releaseTimers.delete(cc);
    if (down) {
      pressedKeys.add(cc);
      paintPress(cc);
    }
    // Keep a quick press/release visible even if both arrive before one paint.
    else if (pressedKeys.has(cc))
      releaseTimers.set(
        cc,
        setTimeout(() => {
          releaseTimers.delete(cc);
          pressedKeys.delete(cc);
          paintPress(cc);
        }, 160),
      );
  }
  function resetPresses() {
    for (const timer of releaseTimers.values()) clearTimeout(timer);
    releaseTimers.clear();
    pressedKeys.clear();
    for (const cc of keys.keys()) paintPress(cc);
  }
  function paintFocus() {
    if (root.hidden) return;
    const s = read(selected);
    display.textContent = readout ?? s.label + " · " + (s.disabled || s.text);
    if (paintedSelection === selected) return;
    cells[paintedSelection]?.cell.classList.remove("xl3-selected");
    cells[selected]?.cell.classList.add("xl3-selected");
    paintedSelection = selected;
  }
  function focus(slot, message) {
    selected = slot;
    readout = message ?? null;
    paintFocus();
  }
  function setKey(cc, state) {
    keyStates.set(cc, state);
  }
  function refresh() {
    if (root.hidden) return;
    paintFocus();
    for (const cc of keys.keys()) paintPress(cc);
    for (const [cc, state] of keyStates) {
      const key = keys.get(cc);
      if (!key) continue;
      for (const [name, value] of Object.entries(state)) {
        if (name === "pressed") {
          if (key.getAttribute("aria-pressed") !== String(value))
            key.setAttribute("aria-pressed", String(value));
        } else if (name === "label") {
          if (key.textContent !== value) key.textContent = value;
        } else if (name === "disabled" && key.disabled !== value)
          key.disabled = value;
      }
    }
    for (let slot = 0; slot < 32; slot++) {
      const s = read(slot),
        entry = cells[slot],
        { cell, input, output } = entry,
        key = [s.position, s.text, s.disabled].join("|");
      if (entry.key === key) continue;
      entry.key = key;
      input.value = s.position;
      input.disabled = !!s.disabled;
      input.setAttribute("aria-valuetext", s.text);
      cell.title = s.disabled || s.hint || s.label;
      output.textContent = s.disabled ? "—" : s.text;
      cell.style.setProperty("--turn", -135 + s.position * 270 + "deg");
      cell.classList.toggle("unavailable", !!s.disabled);
    }
  }
  function show(value = true) {
    if (!value && root.contains(document.activeElement))
      document.activeElement.blur();
    root.hidden = !value;
    root.inert = !value;
    if (value) {
      refresh();
      heading.querySelector("[data-close]").focus();
    }
  }
  root.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.stopPropagation();
      if (onClose) onClose();
      else show(false);
    }
  });
  const disconnectButton = document.createElement("button");
  disconnectButton.type = "button";
  disconnectButton.className = "xl3-disconnect";
  disconnectButton.textContent = "Disconnect";
  disconnectButton.hidden = true;
  heading.insertBefore(disconnectButton, heading.lastElementChild);
  disconnectButton.addEventListener("click", disconnect);
  return {
    show,
    refresh,
    focus,
    keys,
    setKey,
    press,
    resetPresses,
    connecting(value) {
      heading.querySelector("[data-connect]").disabled = value;
    },
    connected(name) {
      heading.querySelector("[data-connect]").textContent = name
        ? "Reconnect"
        : "Connect";
      disconnectButton.hidden = !name;
      heading.querySelector("#xl3-connection").textContent =
        name || "Not connected";
    },
    message(text) {
      readout = text;
      paintFocus();
    },
    row(delta) {
      const row = (Math.floor(selected / 8) + delta + 4) % 4;
      focus(row * 8);
      cells[row * 8].cell.scrollIntoView({
        block: "nearest",
        inline: "nearest",
      });
    },
    get open() {
      return !root.hidden;
    },
  };
}
