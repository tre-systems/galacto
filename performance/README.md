# Galacto instrument package

`instrument.json` defines version 1.0.0 of the performance-host API 1 package.
The file list is the source boundary: the host imports a committed version and
checks every file hash before building. Engine and adapter fixes belong here.
Bump the package version before updating the host's pinned dependency.

The adapter exposes `prepare(context, output)`, `start()`, `release()`, `park()`
and `tick(milliseconds)`, plus the controller bindings and layout. The host owns
the shared AudioContext, output fades, MIDI connection and controller panel.
`park()` runs only after the output reaches zero; it never closes the context.
The standalone application keeps its ordinary controls and stored presets.
