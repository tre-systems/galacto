# Diagrams

Graphviz / DOT sources plus rendered PNGs. The `.dot` files are the source of truth; the PNGs are committed for in-browser viewing on GitHub. Graphviz is for standalone architecture and flow diagrams; Mermaid is used only for small inline diagrams inside Markdown.

## Files

| Diagram                              | Source                | Rendered              |
| ------------------------------------ | --------------------- | --------------------- |
| System overview                      | `system-overview.dot` | `system-overview.png` |
| Frame loop (update → render)         | `frame-loop.dot`      | `frame-loop.png`      |
| GPU buffers (data layout)            | `gpu-buffers.dot`     | `gpu-buffers.png`     |

## Reading Order

1. **System overview** for the whole shape: Cloudflare Pages → the browser page → the Rust/WASM app core → the GPU (where particle state lives and both physics and drawing happen) → the canvas.
2. **Frame loop** for what one `requestAnimationFrame` does: `update` (input, pause, params) then `render` (the compute dispatch, the instanced draw, present).
3. **GPU buffers** for the on-GPU data model: the Particle/Accel storage buffers and Params/Camera uniforms, their exact field packing (the Rust↔WGSL contract), and which passes read and write each.

## Conventions

Color coding by domain:

- Blue — the browser / client surface (bootstrap, render loop, input).
- Green — the Rust WASM app core (`AppState`, `Simulation`, `Camera`).
- Teal — the host (Cloudflare Pages).
- Purple — the GPU rendering boundary (render pipeline, particle buffer).
- Amber — the per-frame GPU **compute** dispatch (the parallel physics pass).
- Green bold outline — the terminal on-screen output (`<canvas>`).
- Diamonds — decisions (white fill, dark border).

The **GPU buffers** diagram is an ER-style record layout: teal headers are storage buffers, blue headers are uniforms, and the amber/purple pass nodes reuse the compute/render colours above.

Fonts: Avenir. Rendered at 220 DPI.

## Render

```
npm run diagrams          # render all .dot files to PNG next to the source
npm run check:diagrams    # verify each .dot renders cleanly and the PNG exists
```

Both scripts assume Graphviz is on PATH (`brew install graphviz`). CI installs Graphviz before running `check:diagrams` (see `.github/workflows/diagrams.yml`). On a machine without `dot`, `check:diagrams` skips with a clear message; refresh the PNGs with `npm run diagrams` before committing diagram changes.

To render one manually:

```
dot -Tpng:cairo docs/diagrams/<name>.dot -Gdpi=220 -o docs/diagrams/<name>.png
```
