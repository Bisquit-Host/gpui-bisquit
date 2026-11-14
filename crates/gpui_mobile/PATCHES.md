# Vendored patches

`gpui-mobile` is an archive of commit `5fd54f4554263ae06753e90472c7f2e2ff5d7495`
from https://github.com/itsbalamurali/gpui-mobile. Only its source, manifest, README
and licenses are included. The root dependency still records the git revision;
Cargo's source patch selects this copy.

GPUI and its wgpu renderer use gpui-fast commit
`a427a7d9a6072473ad092b1673cd5235f99284cc`, shared with desktop through the
root git-source patch. `gpui-mobile`'s manifest names the same source and revision. The vendored source is in `crates`.
See [README.md](../../PATCHES.md) for the full renderer patch port inventory.

Patches:

- `crates/gpui_wgpu/src/wgpu_context.rs`, `uses_webgl_instance_data`: use the existing
  texture instance transport for native GLES as well as browser WebGL. GLES on
  this emulator rejects the storage-buffer path's 104-byte path vertex record.
- `crates/gpui_mobile/src/android/jni.rs`: skip platform-view lifecycle JNI calls when
  the registry is empty. A shell that only packages the IME and document picker
  helpers should not attempt to load `GpuiPlatformView` on every resume.
- `crates/gpui_mobile/src/android/window.rs` and its Cargo feature `frame-timing`: record
  time between renderer draw completions and time spent inside `renderer.draw`.
  These are wall times on the render thread, including driver/GPU waits inside
  submission and presentation, not GPU durations or display-present timestamps.
  Remove the feature from the root dependency to turn logging off.

No global registry files or read-only source clones were modified.

Workspace additions:

- Native GLES requests WebGL2/downlevel limits, including zero compute limits.
  The native GL pipelines already use the texture transport from the spike.
- The optional native-gles-only feature creates only a GLES instance. Together
  with emulator -gpu host -feature -Vulkan, this prevents the emulator's Vulkan
  surface from occupying the native window before GLES configures it.
- The IME queue accepts complete editable snapshots, UTF-16 selections and
  composing spans. Hosts using the previous commit/composition events remain
  supported. GpuiInputActivity in bisquit-mobile supplies the editable mirror.
  Apps call `jni::sync_input` with an `InputState`, never the Java method.
  `jni::enable_raw_input` switches the proxy to a `TYPE_NULL` connection that
  forwards each IME call as a `RawInput` (terminals); the next `sync_input`
  leaves raw mode. Vsync stays on Java's Choreographer on purpose: its frame
  callback is ordered after that vsync's batched touches, which an NDK
  `AChoreographer` callback is not.
- The Android AccessKit adapter activates only while an accessibility service
  is enabled or querying, because GPUI builds and diffs the full semantic tree
  on every frame while active. A provider query waits up to 250 ms for GPUI's
  first real tree, so the first synchronous query (for example UIAutomator)
  returns actual nodes instead of the placeholder. Disabling services
  deactivates GPUI's tree and discards pending events and scroll actions.

The emulator feature is opt-in in Cargo. Device builds can retain Vulkan.

Performance patches, 2026-11-01:

- Android retains command buffers, coalesces foreground wake-pipe writes, and
  inserts timers into a descending deadline queue instead of repeatedly sorting
  and shifting the queue. Unchanged system insets no longer force an app redraw.
- `gpui/src/window.rs`: glyph painting asks the platform window for subpixel
  support once per paint pass, not per glyph (Android takes a state mutex there).
  `gpui/src/elements/text.rs`: text measurement checks out a pooled line wrapper
  only when it truncates.
- Scene batching selects the next two primitive orders without sorting all nine
  primitive types for every batch.
- wgpu reuses its instance bind group, uploads full GLES instance-texture rows
  together, and skips unchanged global uniforms. Path textures allocate only for
  scenes containing paths; path vertices, sprites and texture bindings are reused.
- Backdrop uniforms use one upload per frame. Fully covered glass pixels skip
  their zero-contribution outer shadow. Empty shadow integration ranges return
  zero before the sampling loop.
- Quad, image, shadow and underline fragment shaders consume flat vertex outputs
  instead of decoding instance records again for every pixel. Quads use 11 locations,
  below the 16-location GLES/WebGL limit requested by this renderer.
- The Java video-view bridge reuses layout parameters and skips unchanged
  physical bounds instead of requesting layout on every painted frame.

Device renderer pass, 2026-11-01 (Realme RMX3850, Adreno, Vulkan, 120 Hz):

- `gpui_wgpu/src/backdrop.rs`: glass is grouped so one break of the main pass
  serves every backdrop whose source is already final (`plan_groups`). A group's
  copies run together, its downsample and blur passes run once for all of its
  glass in a shared ping-pong atlas, and each glass is drawn inline when the main
  pass resumes. Downsampling is one box-filter pass by 2^n and the gaussian is one
  2D pass of the separable kernel's paired taps, so a group needs at most two
  blur passes. The billing root went from 26 render passes per frame to 7.
- `gpui_wgpu/src/wgpu_renderer.rs`: an opaque, square, borderless quad drawn
  first over the whole target becomes the pass's clear color. Adreno renders
  GPUI's few-draw passes in sysmem ("Direct") mode, where that quad cost a full
  read and write of the frame. Instance data is bound once as a whole buffer, with
  each primitive kind selected by first instance, as the GLES texture transport
  already did, so scrolling no longer creates bind groups every frame.
- `gpui_wgpu/src/presenter.rs`: on Android frames are presented on a
  `gpui-present` thread. `queueBuffer` makes a synchronous binder call to
  SurfaceFlinger; the render thread now only waits for it before touching the
  surface again (acquire, configure, release).
- `gpui_wgpu/src/shaders.wgsl`: quads with one corner radius, one border width,
  a solid border and a solid background (nearly all UI quads) use
  `vs_quad_simple`/`fs_quad_simple`, `fs_quad`'s math for that case with 5
  varyings instead of 11 and no gradient or dash code, so Adreno runs more
  fragments at once. Its 10-vertex strip fans over the quad, or frames the
  interior of a transparent quad (a border drawn over a card) so the
  interior, which `fs_quad` would leave transparent, is never shaded. Batches
  are split into runs by pipeline, keeping the drawing order.
- Paths rasterize into an intermediate sized to the frame's paths instead of
  the whole target; each path batch clears and resolves its 4x multisampled
  intermediate. `PathSprite` carries its texture coordinates.
- `gpui/src/taffy.rs`: a layout root whose subtree matches one laid out in the
  previous frame (taffy styles compared with `PartialEq`, children, measure
  keys and available space) takes that frame's layouts instead of running
  taffy. Text supplies a key over every input that sizes it
  (`Window::request_keyed_measured_layout`); measured leaves are re-measured
  with their last inputs so text keeps its lines, and any different result
  abandons the reuse. Scrolling changes no styles, so scroll frames skip layout.
- `gpui/src/window.rs`, `gestures.rs`, `elements/div.rs`: taps arrive as a
  mouse press and release at lift-off, so a touch that can still become a tap
  is tracked as the window's touch press. After Android's 100 ms tap timeout
  (or briefly after a quick tap) the topmost hitbox with click listeners under
  it applies its `active` style, or 0.85 opacity when it has none, like the
  reference's black ripple. Pans and long presses clear it. Only the view that
  drew the pressed element repaints, and a tap that navigates does not light
  up whatever replaces it. `.active(|s| s)` opts an element out;
  `Window::visible_touch_press` lets custom controls react, as glass buttons
  do by swelling.
- `gpui/src/elements/list.rs`: horizontal padding insets the items, so a
  `list` with `.px(..)` matches a `LazyColumn` with horizontal padding.
- Root `Cargo.toml` adds a `profiling` profile (release with line tables) for
  simpleperf; the manifest declares `<profileable android:shell="true"/>`.

Device pass, 2026-11-06 (same phone):

- `crates/gpui_mobile/src/android/accessibility.rs`, `window.rs` and the app's
  `GpuiAccessibilityBridge.java`: the bridge reports a mode instead of a flag.
  Screen readers and services that follow `TYPE_WINDOW_CONTENT_CHANGED` keep the
  tree eager. Other services, such as the autofill service ColorOS enables by
  default, get it on demand: while one queries the provider (dropped after 10 s
  without a query) or a text field has focus. Before, that service made GPUI
  build and diff the full tree every frame, draw the screen uncached, and the
  main thread spend a fifth of its time in JNI method lookups.
- `gpui_wgpu/src/wgpu_context.rs`: native instances drop
  `VALIDATION_INDIRECT_CALL`. GPUI issues no indirect draws, and the validation
  compiled compute pipelines while the device was created.
- `gpui_wgpu/src/wgpu_atlas.rs`: monochrome atlas textures (glyphs, icons) keep
  a CPU mirror and upload one dirty rectangle per texture per frame. Adreno
  allocates kernel memory for every buffer-to-image copy.
- `crates/gpui_mobile/src/android/window.rs`: Android text uses grayscale antialiasing,
  as the platform does. Subpixel glyphs fringed on the PenTile panel and were
  four-byte color tiles uploaded one by one: the first frame spent 155 ms on 184
  of them.

- `gpui/src/elements/list.rs`: `ListSizingBehavior::Infer` measures rows at the
  list's last content width instead of max-content width, so a sheet sized by a
  list of wrapping rows is no longer shorter than its content.

- `gpui/src/elements/div.rs`, `list.rs`: nested scrolling as in Compose. A
  scroll container clamps the delta to its bounds and stops propagation only
  when its offset moved, so the console log scrolls alone and hands the pan to
  the page at its ends. Touch (precise) deltas no longer cross axes; only wheel
  lines still turn a horizontal-only scroller.
- `gpui/src/stretch.rs`, `scene.rs`, `fast/rounded_clip.rs`,
  `fast/retained.rs`: `Window::with_vertical_stretch` scales what is painted
  inside it about a horizontal line, for Android's stretch overscroll. The
  scene stores primitives, layer bounds and rounded clip shapes already
  stretched and replays them unchanged; a retained subtree is reused only under
  the stretch it was painted with. Hitboxes are not stretched.
- `crates/gpui_mobile/src/android/frame_source.rs`, `host.rs`, `jni.rs`: with
  `host::use_activity_vsync`, frames follow the activity's `Choreographer`
  (`gpuiRequestVsync` posts a frame callback that calls `host::vsync`; it
  repeats every vsync while a finger is down). Its callback runs after that
  vsync's batched touches reached `nativeTouch`, and the loop latches the vsync
  before draining touches. With the separate vsync thread, touch and vsync
  raced: during a steady pan about 7% of frames took no touch and the next two,
  a stall and a jump at a true 120 Hz. A real-finger trace after the change had
  no such pairs in about 1,000 frames.

Hitch pass, 2026-11-10 (emulator, CPU profile of the render thread):

- `gpui_wgpu/src/cosmic_text_system.rs`, `render_glyph_image`: glyphs are
  rasterized unhinted on Android, as the platform draws text on phone-density
  screens. A glyph's first paint rasterizes it on the render thread, and running
  Inter's hinting program was about 70% of that: a fifth of the frame that first
  draws a page. Other platforms keep hinting.

The app vendors the Java half of rustls-platform-verifier 0.7.1 in
`android/app/src/main/java/org/rustls/platformverifier`, with the AAR's network
security config. Its revocation check built `PKIXBuilderParameters` from the
whole `AndroidCAStore`, reparsing every system certificate on each handshake;
the port uses the chain's own trust anchor. Gradle fails if Cargo.lock moves
off 0.7.1. `CertificateVerifier.prewarm` (from `MainActivity`) and a thread in
`start_android` (aws-lc's RNG, the shared client) do one-time TLS setup at
launch instead of during the first request.

See [render-investigation.md](render-investigation.md) for measurements,
verification and their limits.
