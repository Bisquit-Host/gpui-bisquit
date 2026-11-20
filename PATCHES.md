# Bisquit patches

This repository is gpui-fast with the patches below, plus `crates/gpui_mobile`
(see [its patch notes](crates/gpui_mobile/PATCHES.md)). The notes were written
while these crates were vendored inside the Bisquit desktop app, so "vendored"
and "root workspace" refer to that earlier layout.


See [MOBILE.md](crates/gpui_mobile/PATCHES.md) for renderer, frame pacing, font and input patches
and the pinned upstream revisions.

## Shared GPUI source

`crates` holds `gpui`, `gpui_wgpu`, `gpui_apple` and
`gpui_macos` from [gpui-fast](https://github.com/longbridge/gpui-fast) commit
`b533574510496ab3619204b57f8b8e0487238d9b`, which imports Zed's GPUI at
`78f5d7446c55f6c1aa9b66e8111f6b3f4625ebb0` and adds retained views, retained
layout and scroll layers. Examples, external tests and benches are omitted;
their manifest targets and dev dependencies are removed. The nested workspace
resolves inherited dependencies against gpui-fast's root manifest, replacing
its path crates with git dependencies pinned to the same commit. The root
workspace patches the gpui-fast source with these four crates. No crate is
taken from Zed directly; the desktop app owns its Tokio runtime.

Retained views and scroll layers can be turned off at runtime with
`GPUI_VIEW_RETENTION=0` and `GPUI_SCROLL_LAYERS=0`, to tell a stale view
from a renderer problem. Scroll layers are compiled for macOS, Linux and
Windows only; Android draws retained views without them.

### Merge of gpui-fast b533574 (2026-11-20)

Merged from `80be501` with `git merge`: 16 gpui-fast changes and its sync with
Zed `78f5d74`. Eight files conflicted.

- `gpui_macos/src/window.rs`: gpui-fast now pauses a Metal display link while a
  window is occluded instead of detaching it
  (`fast::display_link_lifecycle`). Our delayed GPU release drops the layer
  that link is bound to, so the release detaches the link first, and a window
  whose layer was recreated starts a new link instead of resuming the old one.
- `gpui_wgpu/src/wgpu_context.rs`: our WebGL2 limits for Android GLES are
  dropped for `fast::native_gl::required_limits`, which requests the adapter's
  own limits when it lacks compute. Android GLES still always uses texture
  instance data; other native GL adapters do when they lack vertex storage.
- `gpui_wgpu/src/cosmic_text_system.rs`: our rule stays (any face with a glyph
  loads); a known emoji font skips the charmap walk through
  `fast::emoji_fonts`.
- `gpui_wgpu/src/wgpu_renderer.rs`: our surface creation with the Android GL
  fallback notes the context for `fast::wgpu_context`, and a frame clears
  dynamic texture uploads before `fast::path_targets::prepare`.
- `gpui/src/window.rs`: the frame's hit test goes through `fast::pointer`, next
  to our touch-pressed hitbox.
- `fast/misc/mod.rs`, `gpui_wgpu.rs` and `script/upstream-allowlist` list both
  sides.

### Merge of gpui-fast 80be501 (2026-11-15)

Merged from `a427a7d` with `git merge`; nothing conflicted. Both sides changed
`gpui/src/elements/list.rs` (their deferred `measure_all` after a width change,
our nested scrolling and `Infer` row width), `fast/tests/layers.rs`, the Wayland
client and `gpui_macos/src/window.rs`, in separate hunks.

### Rebase onto gpui-fast a427a7d (2026-11-09)

Merged from `61dccec` by a three-way merge. Upstream now passes the list's
horizontal padding to list layers itself (`end_list` takes `padding`), so our
`row_width` patch was dropped for its version. `fast/frame.rs` keeps our
simple-quad and background-clear helpers next to upstream's new tests module.

### Rebase onto gpui-fast 61dccec (2026-11-07)

Our patches were carried from Zed `145bcfc1bc` by a three-way merge against
gpui-fast. The 58 Zed commits between `decd2399b1` and `145bcfc1bc` are not in
this base; none of them are patches we rely on.

Dropped, because gpui-fast does the same work:

- The taffy layout reuse across frames and keyed text measurement
  (`request_keyed_measured_layout`). gpui-fast retains layout nodes and text
  measurements per view.
- The per-paint subpixel rendering cache. gpui-fast asks once per glyph run.
- In `gpui_wgpu`: the globals write cache, the single instance bind group and
  per-batch path intermediate regions. gpui-fast's `fast/frame.rs` records
  each frame with one upload, cached bind groups and grouped path passes; the
  path intermediate is frame-sized again.
- `Interactivity::scroll_max`, identical to `fast::layers::wheel::scroll_max`.

Carried onto gpui-fast's hooks:

- Touch press feedback, nested scrolling, the click bounds, and list horizontal
  padding, which is also passed to list layers as their row width.
- `Backdrop` primitives: sorted by `fast::scene::sort_in_drawing_order` and
  drawn in `fast::frame::draw_scene`, which ends the main pass per glass
  group as the old loop did. Scroll-layer content with a backdrop is drawn into
  the frame instead of tiles (`fast::layers::paint::repaint`).
- The macOS occlusion release also clears the Metal tile cache.
- Lazy pipelines with the disk pipeline cache, now with gpui-fast's rounded
  clip bind group; the simple-quad pipeline (clipped like `fs_quad`) and the
  opaque background clear, both in `draw_scene`; the GL path vertex base passed
  through the instance index; the Android presenter thread, Vulkan-to-GL
  fallback, present mode validation, copyable surfaces and 2D instance texture
  uploads.

### Rebase onto 145bcfc1bc (2026-11-06)

The patches were carried from `36682547be` by a three-way merge against both
upstream revisions. `Cargo.toml`'s `[workspace.dependencies]` is regenerated
from Zed's root manifest for every `workspace = true` dependency of the four
crates; Zed-internal path crates become git dependencies at the same revision.
Upstream moved wgpu to a git revision and taffy to 0.15; `gpui-mobile` names
the same wgpu revision so both resolve one copy.

Changed while rebasing:

- Upstream removed the hashed line-layout cache, so its borrowed-key patch and
  tests were dropped with it.
- `presenter.rs` presents through `Queue::present`, which replaced
  `SurfaceTexture::present`. The lazy `paths` pipeline uses premultiplied
  source-over blending, matching upstream's composite fix.
- The renderer keeps the instance reuse and Android Vulkan-to-GL fallback;
  upstream's configure-once surface setup is honored through
  `surface_configured`.
- Upstream ported `gpui_apple` and `gpui_macos` from `cocoa`/`metal` to
  `objc2`. The occlusion patch was rewritten on that code: the delayed release
  runs on the window's GPUI executors because window state is now `Rc`, and the
  `CAMetalDisplayLink` frame path consumes the forced redraw after a restore.
  A restored layer is installed before the display link starts on it.
- `gpui-mobile` reports `Platform::platform_info` as native Android and sends
  `PlatformInput::Scroll`. Applications use the renamed `ScrollEvent` and
  `on_scroll`.

### Renderer patch port from gpui-pre-wgpu 0.3.6

The old registry copy's complete `src/` diff touched three files. These changes
now live in `crates/gpui_wgpu`:

- `wgpu_context.rs`: native Android GLES uses texture instance transport and
  WebGL2 limits with zero compute requirements. `native-gles-only` restricts
  instance creation to GL; default builds keep Vulkan and GL selection.
- `wgpu_renderer.rs`: compile each pipeline lazily, preserving shader choice,
  blending, MSAA and the dual-source feature guard. Remove the unused surfaces
  pipeline and layout. Validate requested present mode on replacement surfaces,
  falling back to Fifo when Android GLES cannot provide Mailbox.
- `cosmic_text_system.rs`: retain private `Bspc` tracking in font cache keys,
  apply cluster spacing to proportional fonts and fallbacks, exclude it from
  OpenType tags, resolve Android generic monospace names, and accept nonempty
  symbol/emoji fonts without Latin `m` coverage.

Dropped after checking the new upstream implementation:

- The optional-surface lifecycle patch is superseded by `RendererState`.
  `unconfigure_surface` explicitly drops the surface and retains the core;
  drawing, resizing and recovery handle the unconfigured state.
- Removing rejected font faces from fontdb is superseded by upstream's decision
  to retain fallback faces. The local nonempty-primary-font check remains.
- Incidental import ordering has no runtime purpose. The upstream dual-source
  guard and its diagnostic comment are retained.

The newer upstream device recovery, atlas-generation tracking and rendering
error handling are retained. Font memory mapping, frame telemetry, Activity
ownership, accessibility activation and render investigation tooling belong to
`gpui-mobile` or the application and are retained there.

`bisquit-mobile` inherits the workspace GPUI dependency. The excluded
`gpui-mobile` vendor crate names the same git source and exact revision directly,
including its iOS renderer and test-support dependency. Its iOS font-kit source
also matches Zed, removing the former registry/git duplicate.

The host adapts the new display hooks for its existing single-display model,
reports no measured refresh interval, and labels frame requests as native
Choreographer callbacks or local clock fallback. GPU specs now flatten the
renderer core's optional result. AccessKit, text-system and HTTP-client APIs
remain compatible. No edits to GPUI's `scene.rs` or `window.rs` were needed.

## Android accessibility

`gpui-mobile` uses `accesskit_android` 0.7.5, matching the AccessKit 0.24.1
schema resolved by the pinned Zed revision. `src/android/accessibility.rs` connects the
`PlatformWindow::a11y_init` and `a11y_tree_update` hooks to the low-level Android
adapter. The host supplies `dev.gpui.mobile.GpuiAccessibilityBridge` and the
`gpuiInitAccessibility(long)` / `gpuiDispatchAccessibility(long)` activity methods.

The adapter's published 0.7.5 archive is retained under
`gpui-mobile/vendor/accesskit_android`. Its `src/event.rs` text-change calculation
is patched to handle empty strings without unsigned underflow. The upstream
expression `(old_u16.len() - 1) as jint` aborts the app through JNI when an empty
field first receives text in a build with overflow checks. Focused tests cover
insertion/deletion at empty boundaries and UTF-16 prefix/suffix counts.
The archive SHA-256 is
`8e27dedd5e1932e1f52fedcde681bc2ae4213df53bad5b295103aa36b1d0ffb6`.

The provider belongs to NativeActivity's content view. Queries, hover and event
delivery run on Android's UI thread. GPUI action callbacks enqueue work on its
native executor. JNI references never cross between the adapter's JNI 0.21 and
the platform's JNI 0.22 wrappers. Weak numeric handles reject callbacks after
the GPUI window has gone away.

GPUI already emits physical-pixel node bounds and accounts for layout insets.
AccessKit adds the content view's screen origin. Scroll actions convert those
bounds back to logical pixels once and dispatch page-sized wheel input through
GPUI. The root exposes vertical scrolling because this GPUI snapshot lacks a
built-in accessibility scroll handler. Android set-text requests map to GPUI
`SetValue`; editable components must register that action. The invisible IME
mirror is excluded from accessibility to avoid duplicate text fields.
The Java bridge supplies the clickable flag omitted by AccessKit 0.7.5 and
clips screen bounds/visibility against the host View's visible rectangle.

Accessibility activates when Android queries the provider and deactivates when
Android accessibility is disabled. Equal complete GPUI snapshots skip adapter
updates and JNI event posts. With the `frame-timing` feature, `a11y_update` logs
the mean native tree processing time in microseconds every 60 changed snapshots.
This timing excludes GPUI's tree construction and Java event delivery.

On the shared emulator, 60 changed snapshots averaged 64 microseconds of native
tree processing, with 59 bounded nodes in the last snapshot. JNI posting and
Java event delivery were not included. Equal snapshots still require comparison
but skip adapter processing and event posting; disabled accessibility does not
build GPUI accessibility trees.

### Modal isolation

`crates/gpui_mobile/src/accessibility_tree.rs` filters GPUI's complete snapshots before
initial Android accessibility activation and each active update. It retains the
window root and the last `Role::Dialog` subtree in child order, including nested
dialogs and editable `TextRun` children. It moves reported focus into that
subtree if the old focus is behind the modal. A later complete snapshot restores
the background after dismissal. The window root does not expose synthetic page
scroll actions while a modal is open. Shell keyboard focus trapping remains the
host's responsibility. Three focused tests cover stacked dialogs, nested text
input, focus and dismissal. This filter relies on the pinned GPUI sending full trees;
it must not be applied to incremental AccessKit updates.

## SH02 rendering investigation

The supplied audit PNGs do not contain the reported disappearing UI. Their
only pixel changes are 254 pixels in the Android clock, with a bounding box of
`[144, 63, 164, 94)`. All application pixels are identical. Separate crops retain
the header, allocation text and footer. Fresh GLES idle/tap captures are also
byte-identical. No renderer or frame-pacing change was made.

`render-stress.sh SERVER_ID [cycles] [output-directory]` uses the installed APK
under the shared emulator lock. It captures idle/tapped Network frames, visits
fourteen GET/list server pages, scrolls read-only lists, and backgrounds/resumes
the app. It does not press server mutation controls. `verify-render-captures.py
OUTPUT_DIRECTORY` compares static Network pairs outside Android's system bars
and rejects blank frames. It requires Pillow. Renderer timings use the existing
native `frame-timing` logs, not Android Java frame counters.

See [render-investigation.md](render-investigation.md) for evidence, device
constraints, checks and measured submission times.

## Liquid glass backdrops (gpui, gpui_wgpu, gpui_apple)

- `gpui/src/scene.rs`, `window.rs`: a `Backdrop` primitive (`Capture` or `Glass`) with `BackdropEffect`, and `Window::paint_backdrop`. Parameters mirror Kyant's backdrop 2.0.1 (vibrancy, blur, lens, dispersion, highlight, shadows, touch glow, radial glow) so mobile glass matches the Kotlin reference.
- `gpui_wgpu/src/backdrop.rs`, `shaders_backdrop.wgsl`: frames with backdrops draw into a copyable surface, or offscreen and blitted when the surface cannot be copied. Each glass copies the pixels under it (or a saved slot), blurs them with Skia's radius-to-sigma gaussian and draws the lens shader. Captures save regions into numbered slots for later glass to read or restore. `fast/frame.rs` holds the renderer in `FrameState` and draws backdrops of the frame's own scene only; scroll-layer tiles never contain them.
- `gpui_apple/src/metal_renderer.rs`: desktop skips backdrop batches.
