# AGENTS.md

Single-crate egui/eframe app: a simple test-pattern noise generator for ASTM 1695. It creates a CT single-slice image with given background noise and a circular object; the image can be saved to TIFF or DICONDE (DICOM flavor used in NDT/industrial imaging). All app logic lives in `src/main.rs`; no tests, no CI, no task runner.

## Status

Implemented: `src/main.rs` realizes the spec below for both native and wasm targets, and `index.html` (the trunk host page) exists. See "Editing src/main.rs" for the per-target structure.

## Generated Image

- Square, side = pixels (default 1000, editable as **Image Size**, range 1–4096).
- Background noise: Mean (default 20000 grays) and StdDev (default 500).
- Center: filled circle of Diameter in pixels (default 700), Mean (default 40000), StdDev (default 1000).
- Cupping (%): dips the circle center darker by this percent of the mean difference between circle and background — e.g. circle 40000, background 10000, 10 % → center ≈ 37000 (dip 3000); the dip fades to 0 at the circle edge (`1 − (d/r)²` falloff, confirmed with the user); 0 % = flat circle.
- Circle → background transition is Gaussian with Sigma (default 3.0).
- Resolution in mm/pix is a user parameter; store it in Pixel Spacing for DICONDE or the appropriate TIFF tag.

## GUI

```
+-----------------------------------------------------------+
| Title bar                                                 |
+-----------------------------------------------------------+
| File Help                                                 |
+-----------------+--------------------+--------------------+
|  = Tools =      |                    |  = Parameters =    |
| Zoom In         |                    | Background Mean    |
| Zoom Out        |                    | Background StdDev  |
| Fit to Screen   |                    | Circle Mean        |
|                 |       IMAGE        | Circle StdDev      |
|                 |                    | Circle Diameter    |
|                 |                    | Transition Sigma   |
|                 |                    | Cupping %          |
|                 |                    | Resolution         |
|                 |                    | [Generate]         |
|                 |                    | Component Name     |
|                 |                    +--------------------+
|                 |                    | Histogram plot     |
+-----------------+--------------------+--------------------+
```

Interface is panels: menu bar on top, then three panels in the middle (Tools | Image | Parameters).

- The image area supports wheel zoom (anchored at the mouse pointer — `zoom_step` keeps the anchor's texel fraction fixed) and drag-to-pan; zoom = 1.0 is fit-to-screen.
- Tools panel holds only three 36×36 icon buttons (zoom_in, zoom_out, fit_to_screen SVGs in `assets/icons`, embedded via `egui::include_image!` — paths relative to `src/`), no caption; the panel is `exact_size(52.0)` so the gaps left/right of each button match.
- Parameters panel has the 7 spec fields plus two extra rows: **Image Size** (square side is adjustable) and **Component Name**.
- The numeric rows live in a 2-column `egui::Grid` (`id "parameters"`) so labels stay left-aligned and all editor boxes start on one vertical line (user request); a full-width **Generate** button sits directly under the grid; `Component Name` remains below a separator outside the grid. The grid row is the only GUI access to Generate — there are no quick buttons in the menu bar (removed on purpose: Generate under the controls, Save TIFF only in the File menu).
- Shortcuts (constants near the top of `main.rs`): Generate = Ctrl+G, Save TIFF = Ctrl+S, Save DICONDE = Ctrl+Shift+S (`Modifiers::COMMAND.plus(Modifiers::SHIFT)` — the `|` operator is not `const fn`, so it can't be used in the constant), Quit = Ctrl+Q (native only).
- Histogram starts right below the parameter content (bold title + `ui.available_height().max(80.0)`-tall plot), filling the whole bottom area of the Parameters panel — it is NOT a bottom sub-panel (a `Panel::bottom` pins to the panel bottom and leaves a gap after short parameter content). The plot is an `egui_plot` `BarChart` — `egui_plot = "0.37"` (depends on egui ^0.36 — compatible with eframe 0.36). Data: `App.histogram`, 256 `u64` bins over the 0..=65535 gray range, recomputed in `set_image` (bin = value >> 8); `BarChart::new(name, bars)` takes a name as arg #1. The x axis formatter maps bin → gray value (bin × 256).
- Open TIFF was removed on purpose — the app builds its own image; don't reintroduce it without a request.

## Build & run

- Desktop (Windows MSVC): `cargo run`.
- Web: `cargo build --target wasm32-unknown-unknown` (target installed via rustup). The web entry panics unless `index.html` contains `<canvas id="the_canvas_id">` (renaming it breaks the wasm build), plus `<div id="loading_text">` which the bootstrap removes on startup.
- Web build check: `trunk build --release --public-url . --no-default-features`.
- `rand` pulls getrandom 0.3, which refuses bare wasm32 unless the `wasm_js` feature is enabled (`[target.'cfg(target_arch = "wasm32")'.dependencies]` in `Cargo.toml`) **and** `--cfg getrandom_backend="wasm_js"` is set (`.cargo/config.toml`); remove neither or the wasm build fails to compile.
- TIFF resolution tags are written with the `tiff` crate directly (`write_tiff` in `main.rs`) because `image`'s `TiffEncoder` cannot write X/YResolution; there is no millimeter ResolutionUnit, so mm/pix is stored as pixels per centimeter.
- No clippy config; effective verification is `cargo build` on **both** targets + `cargo fmt` (wasm build needs the getrandom config above).

## Formatting

- `rustfmt.toml` is non-default: **hard tabs** (not 4 spaces), `max_width = 100`, `use_small_heuristics = "Max"`. Run `cargo fmt` instead of hand-formatting.

## Editing src/main.rs

Layout is **duplicated per target**; each copy only compiles on its own target:

- `save_tiff`, `save_dicom`, and `main` each exist in two versions guarded by `#[cfg(not(target_arch = "wasm32"))]` / `#[cfg(target_arch = "wasm32")]`. The dialogs, the DICOM element construction in `save_dicom`, and the download/write plumbing are duplicated in both copies — change both (`write_tiff` tag encoding itself is shared).
- Any tag/VR/metadata change hits two output paths: wasm serializes to bytes and triggers a browser download; native writes the file with `write_to_file`.
- There are no cfg-gated `App` fields at the moment (the old wasm-only `pending_image` open-queue was removed with Open TIFF); if target-specific fields reappear, don't "clean up" the cfg attributes.

eframe/egui 0.36 gotchas (verified against docs.rs 0.36.2):

- `eframe::App` has no `update` method in 0.36 — the required method is `fn ui(&mut self, ui: &mut Ui, frame: &mut Frame)`, and that `Ui` is the bare root (no margin/background). Layout = `egui::Panel`s (top/bottom, shown in call order) + `egui::CentralPanel::default()` for the image; painting widgets directly on the root `Ui` leaves the rest of the window the default dark viewport background.
- The app forces a light look regardless of OS theme: both `main()`s call `set_theme(Light)` + `set_visuals_of` (both themes, `Visuals::light()`), and panels use an explicit white `light_panel_frame()` helper — menu bars drawn straight on the root `Ui` have no background at all.
- `egui::include_image!` embeds assets, but its paths are relative to **`src/`** (e.g. `"../assets/icons/open.svg"`).
- SVG assets require the `svg` feature on `egui_extras` (already enabled in `Cargo.toml`) **and** `egui_extras::install_image_loaders(&cc.egui_ctx)` in both `main()`s (native and wasm) — without the install call the texture never loads and buttons render empty.

## DICOM output

- Image → 16-bit grayscale (`to_luma16`), single frame, 1 sample, `MONOCHROME2`, uncompressed Explicit VR Little Endian (`transfer_syntax "1.2.840.10008.1.2.1"`), pixel data in VR `OW`.
- Pixel Spacing (0028,0030) is written as one value repeated for both row/column components.
- UI label "Component Name (0010,0010)" is the DICOM *Patient Name* tag; the code uses standard DICOM tags, not separate DICONDE tags.
- Errors are surfaced in the bottom status bar (`App.status`), set by the results of generate/save calls.
- Noise generator: `gen_noise` (public in `main.rs`) samples per-pixel Gaussian noise with `rand_distr::StandardNormal`; the circle→background blend weight is a Gaussian CDF (`erf` approximation) of the distance from the circle edge, driven by `transition_sigma`; `cupping` (percent) applies a `1 − (d/r)²` mean dip inside the circle, zero at the edge.
