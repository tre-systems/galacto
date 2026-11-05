// Normalized hardware positions use the same ranges and handlers as Controls.
export const XL3_PARAMETERS = Object.freeze({
  0: ['temp-slider', 'Next Q', 'temp-readout', 'Disk stability · Apply rebuilds the galaxy'],
  1: ['gas-slider', 'Next gas', 'gas-readout', 'Gas fraction · Apply rebuilds the galaxy'],
  2: ['bulge-slider', 'Next bulge', 'bulge-readout', 'Bulge fraction · Apply rebuilds the galaxy'],
  3: ['count-slider', 'Next bodies', 'count-readout', 'Body count · Apply rebuilds the galaxy; high counts cost more GPU time'],
  24: ['speed-slider', 'Speed', 'speed-readout', 'Simulation speed · capped at high body counts'],
  25: ['gravity-slider', 'Gravity', 'gravity-readout', 'Live gravitational strength'],
  26: ['halo-slider', 'Halo', 'halo-readout', 'Live dark-matter halo strength'],
  27: ['halo-size-slider', 'Halo size', 'halo-size-readout', 'Live dark-matter scale radius'],
  28: ['size-slider', 'Star size', 'size-readout', 'Star size and soundscape colour'],
  29: ['glow-slider', 'Glow', 'glow-readout', 'Star halos and soundscape brightness / echo'],
  30: ['autopilot-speed', 'Camera speed', null, 'Autopilot drift rate · enable Auto to move the camera'],
  31: ['volume-slider', 'Level', null, 'Sound output level'],
});

export function controllerBindings({ parameter, setParameter, camera, setCamera, available }) {
  function read(slot) {
    let spec;
    if (slot >= 11 && slot <= 13) {
      const value = camera()[slot - 11] ?? 0;
      spec = {
        label: ['Orbit', 'Tilt', 'Zoom'][slot - 11], position: value,
        text: slot === 11 ? `${Math.round(value * 360)}°` : slot === 12
          ? `${Math.round((value * 3 - 1.5) * 180 / Math.PI)}°`
          : `${(0.001 * 5000 ** value).toFixed(3)}×`,
        hint: 'Manual camera · switches Auto off',
      };
    } else if (XL3_PARAMETERS[slot]) {
      const [id, label, readout, hint] = XL3_PARAMETERS[slot];
      spec = { label, hint, ...parameter(id, readout) };
    } else return { label: '—', position: 0, text: '—', disabled: 'Unassigned' };
    return { ...spec, disabled: available() || false };
  }
  function change(slot, position) {
    if (!Number.isInteger(slot) || !Number.isFinite(position) || read(slot).disabled) return false;
    position = Math.max(0, Math.min(1, position));
    if (slot >= 11 && slot <= 13) setCamera(slot - 11, position);
    else setParameter(XL3_PARAMETERS[slot][0], position);
    return true;
  }
  return { read, change };
}

export const GALACTO_XL3_LAYOUT = {
  initialSlot: 24,
  summary: 'Cross a fader’s value to pick up. Shift: fine adjustment. Top knobs stage a new galaxy; Apply rebuilds it.',
  buttons: [
    ['pages', 106, '↑', 'Previous row'], ['pages', 107, '↓', 'Next row'],
    ['left', 116, '▶', 'Play / fade'], ['left', 104, 'Hide', 'Hide controls'],
    ['grid', 37, 'Auto', 'Camera autopilot'], ['grid', 38, 'Halo', 'Show dark matter'],
    ['grid', 39, 'Curve', 'Show rotation curve'], ['grid', 40, 'Mute', 'Mute sound'],
    ['grid', 41, 'Pause', 'Pause simulation'], ['grid', 42, 'Home', 'Reset camera'],
    ['grid', 43, 'Apply', 'Rebuild galaxy with staged knobs'], ['grid', 44, 'Restart', 'Shift + Restart galaxy'],
    ['grid', 45, 'Disk', 'Isolated disk'], ['grid', 46, 'Merger', 'Prograde merger'],
    ['grid', 47, 'Head-on', 'Head-on collision'], ['grid', 48, 'Retro', 'Retrograde encounter'],
    ['grid', 49, 'Minor', 'Minor merger'], ['grid', 50, 'Group', 'Compact group'],
    ['grid', 51, 'M51', 'M51 flyby'], ['grid', 52, 'Controls', 'Standard controls'],
  ],
};
