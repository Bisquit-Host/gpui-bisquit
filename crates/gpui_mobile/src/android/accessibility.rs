//! AccessKit's Android provider runs on the Java UI thread. GPUI callbacks
//! enqueue work on its native executor; never run application code under JNI.

use accesskit_android::{jni as ajni, Adapter, PlatformAction, QueuedEvents};
use ajni::{
    objects::{JClass, JObject, JString},
    sys::{jboolean, jfloat, jint, jlong},
    JNIEnv,
};
use gpui::accesskit::{
    self, Action, ActionData, ActionHandler, ActionRequest, ActivationHandler, NodeId, TreeUpdate,
};
use parking_lot::Mutex;
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, OnceLock, Weak,
    },
    time::{Duration, Instant},
};

use super::window::AndroidWindow;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
type Registry = Mutex<HashMap<u64, Weak<Mutex<State>>>>;
static REGISTRY: OnceLock<Registry> = OnceLock::new();

fn registry() -> &'static Registry {
    REGISTRY.get_or_init(Mutex::default)
}
fn state(id: jlong) -> Option<Arc<Mutex<State>>> {
    registry().lock().get(&(id as u64)).and_then(Weak::upgrade)
}

struct Activation(Box<dyn Fn() -> Option<TreeUpdate> + Send>);
impl ActivationHandler for Activation {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        let tree = (self.0)().map(|mut tree| {
            crate::accessibility_tree::isolate_modal(&mut tree);
            tree
        });
        super::frame_source::schedule_frame();
        tree
    }
}

struct Actions {
    callback: Box<dyn Fn(ActionRequest) + Send>,
    bounds: HashMap<NodeId, accesskit::Rect>,
    editable: HashSet<NodeId>,
    scrolls: Vec<gpui::ScrollEvent>,
    scale: f32,
}
impl ActionHandler for Actions {
    fn do_action(&mut self, request: ActionRequest) {
        let direction = match request.action {
            Action::ScrollUp => Some((0., 1.)),
            Action::ScrollDown => Some((0., -1.)),
            Action::ScrollLeft => Some((1., 0.)),
            Action::ScrollRight => Some((-1., 0.)),
            _ => None,
        };
        if let Some((x, y)) = direction {
            if let Some(bounds) = self.bounds.get(&request.target_node) {
                let scale = f64::from(self.scale);
                self.scrolls.push(gpui::ScrollEvent {
                    position: gpui::point(
                        gpui::px(((bounds.x0 + bounds.x1) / 2. / scale) as f32),
                        gpui::px(((bounds.y0 + bounds.y1) / 2. / scale) as f32),
                    ),
                    delta: gpui::ScrollDelta::Pixels(gpui::point(
                        gpui::px(x * ((bounds.x1 - bounds.x0) / scale) as f32 * 0.8),
                        gpui::px(y * ((bounds.y1 - bounds.y0) / scale) as f32 * 0.8),
                    )),
                    touch_phase: gpui::TouchPhase::Moved,
                    ..Default::default()
                });
            }
        } else {
            (self.callback)(request);
        }
        // Scrolls wait in our input queue rather than GPUI's executor. Waking
        // the looper alone does not request a frame on an otherwise idle app.
        super::frame_source::schedule_frame();
    }
}

/// How much of the tree the enabled accessibility services need, as reported
/// by `GpuiAccessibilityBridge.mode()`.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Off,
    /// Only services that do not follow content changes, like OEM autofill
    /// helpers enabled by default on ColorOS: the tree is built while they
    /// query it or a text field is focused.
    OnDemand,
    /// A screen reader or other service that follows content changes.
    Eager,
}

/// An on-demand tree is dropped once no service has queried it for this long.
const ON_DEMAND_IDLE: Duration = Duration::from_secs(10);

struct State {
    adapter: Adapter,
    activation: Activation,
    deactivation: Box<dyn Fn() + Send>,
    actions: Actions,
    /// GPUI builds and diffs the whole semantic tree on every frame while
    /// active, and draws every view uncached, so it is active only while
    /// `wanted`.
    active: bool,
    mode: Mode,
    text_focused: bool,
    last_query: Instant,
    events: Vec<QueuedEvents>,
    last_tree: Option<TreeUpdate>,
    updates: u64,
    update_micros: u128,
}

impl State {
    fn wanted(&self) -> bool {
        match self.mode {
            Mode::Off => false,
            Mode::OnDemand => self.text_focused || self.last_query.elapsed() < ON_DEMAND_IDLE,
            Mode::Eager => true,
        }
    }
    fn query(&mut self) {
        self.last_query = Instant::now();
        self.activate();
    }
    fn activate(&mut self) {
        if !self.active {
            self.active = true;
            self.adapter.activate(&mut self.activation);
        }
    }
    fn deactivate(&mut self) {
        if self.active {
            self.active = false;
            self.adapter = Adapter::default();
            self.last_tree = None;
            self.actions.bounds.clear();
            self.actions.editable.clear();
            self.actions.scrolls.clear();
            self.events.clear();
            (self.deactivation)();
        }
    }
}

/// Android queries the provider synchronously, while GPUI builds its first
/// real tree on the next frame. Without this bounded wait, the first query
/// after activation (for example from UIAutomator) sees only the placeholder.
fn await_tree(state: &Mutex<State>) {
    let deadline = Instant::now() + Duration::from_millis(250);
    loop {
        {
            let mut state = state.lock();
            state.query();
            if state.last_tree.is_some() {
                return;
            }
        }
        if Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(Duration::from_millis(4));
    }
}

pub(super) struct Bridge {
    id: u64,
    state: Arc<Mutex<State>>,
}
impl Bridge {
    pub(super) fn new(callbacks: gpui::A11yCallbacks) -> Result<Self, String> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let state = Arc::new(Mutex::new(State {
            adapter: Adapter::default(),
            activation: Activation(callbacks.activation),
            deactivation: callbacks.deactivation,
            active: false,
            actions: Actions {
                callback: callbacks.action,
                bounds: HashMap::new(),
                editable: HashSet::new(),
                scrolls: Vec::new(),
                scale: 1.,
            },
            mode: Mode::Off,
            text_focused: false,
            last_query: Instant::now(),
            events: Vec::new(),
            last_tree: None,
            updates: 0,
            update_micros: 0,
        }));
        let bridge = Self { id, state };
        registry().lock().insert(id, Arc::downgrade(&bridge.state));
        super::jni::with_env(|env| {
            let activity = super::jni::activity(env)?;
            env.call_method(
                &activity,
                jni::jni_str!("gpuiInitAccessibility"),
                jni::jni_sig!("(J)V"),
                &[jni::objects::JValue::Long(id as i64)],
            )
            .map_err(|error| {
                env.exception_clear();
                error.to_string()
            })?;
            Ok(())
        })?;
        Ok(bridge)
    }

    pub(super) fn update(&self, mut tree: TreeUpdate, window: &AndroidWindow) {
        let start = Instant::now();
        let mut state = self.state.lock();
        // A frame already in flight may still deliver a tree after deactivation.
        if !state.active {
            return;
        }
        if !state.wanted() {
            state.deactivate();
            return;
        }
        // GPUI 0.3.6 already emits physical-pixel bounds, including its layout
        // padding for system insets. AccessKit adds host.getLocationOnScreen().
        // Scaling or adding status-bar insets here would apply them twice.
        state.actions.scale = window.scale_factor();
        let modal = crate::accessibility_tree::isolate_modal(&mut tree);
        if let Some(root) = tree.tree.as_ref().map(|tree| tree.root) {
            if let Some((_, node)) = tree.nodes.iter_mut().find(|(id, _)| *id == root) {
                let size = window.physical_size();
                node.set_bounds(accesskit::Rect::new(
                    0.,
                    0.,
                    f64::from(size.width.0),
                    f64::from(size.height.0),
                ));
                // GPUI has no built-in scroll accessibility action. Dispatch a
                // page-sized wheel event through its existing hit-tested path.
                if !modal {
                    node.add_action(Action::ScrollUp);
                    node.add_action(Action::ScrollDown);
                }
            }
        }
        if state.last_tree.as_ref() == Some(&tree) {
            return;
        }
        state.actions.bounds = tree
            .nodes
            .iter()
            .filter_map(|(id, node)| node.bounds().map(|bounds| (*id, bounds)))
            .collect();
        state.actions.editable = tree
            .nodes
            .iter()
            .filter(|(_, node)| node.supports_action(Action::SetValue))
            .map(|(id, _)| *id)
            .collect();
        state.last_tree = Some(tree.clone());
        if let Some(events) = state.adapter.update_if_active(|| tree) {
            if state.mode != Mode::Off {
                state.events.push(events);
            }
        }
        state.updates += 1;
        state.update_micros += start.elapsed().as_micros();
        if cfg!(feature = "frame-timing") && state.updates % 60 == 0 {
            log::info!(
                "a11y_update count={} mean_us={} nodes={}",
                state.updates,
                state.update_micros / u128::from(state.updates),
                state.actions.bounds.len()
            );
        }
        let enabled = state.mode != Mode::Off;
        drop(state);
        if !enabled {
            return;
        }
        if let Err(error) = super::jni::with_env(|env| {
            let activity = super::jni::activity(env)?;
            env.call_method(
                &activity,
                jni::jni_str!("gpuiDispatchAccessibility"),
                jni::jni_sig!("(J)V"),
                &[jni::objects::JValue::Long(self.id as i64)],
            )
            .map_err(|error| {
                env.exception_clear();
                error.to_string()
            })?;
            Ok(())
        }) {
            log::warn!("Could not dispatch accessibility events: {error}");
        }
    }

    /// Text entry is where on-demand services (autofill) act, so their tree is
    /// kept while a text field has focus.
    pub(super) fn set_text_focused(&self, focused: bool) {
        let mut state = self.state.lock();
        state.text_focused = focused;
        if focused && state.mode == Mode::OnDemand {
            state.activate();
        }
    }

    pub(super) fn drain_scrolls(&self) -> Vec<gpui::ScrollEvent> {
        std::mem::take(&mut self.state.lock().actions.scrolls)
    }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        registry().lock().remove(&self.id);
    }
}

// JNI entry points use AccessKit's re-exported JNI 0.21 types. The rest of
// gpui-mobile uses JNI 0.22; no references cross between those two wrappers.
#[allow(non_snake_case, reason = "JNI symbol names match the Java host class")]
mod exports {
    use super::*;
    #[no_mangle]
    pub extern "system" fn Java_dev_gpui_mobile_GpuiAccessibilityBridge_nativeNode<'local>(
        mut env: JNIEnv<'local>,
        _: JClass<'local>,
        id: jlong,
        host: JObject<'local>,
        node: jint,
    ) -> JObject<'local> {
        let Some(state) = state(id) else {
            return JObject::null();
        };
        await_tree(&state);
        let mut state = state.lock();
        let State {
            adapter,
            activation,
            ..
        } = &mut *state;
        adapter.create_accessibility_node_info(activation, &mut env, &host, node)
    }
    #[no_mangle]
    pub extern "system" fn Java_dev_gpui_mobile_GpuiAccessibilityBridge_nativeFocus<'local>(
        mut env: JNIEnv<'local>,
        _: JClass<'local>,
        id: jlong,
        host: JObject<'local>,
        focus: jint,
    ) -> JObject<'local> {
        let Some(state) = state(id) else {
            return JObject::null();
        };
        await_tree(&state);
        let mut state = state.lock();
        let State {
            adapter,
            activation,
            ..
        } = &mut *state;
        adapter.find_focus(activation, &mut env, &host, focus)
    }
    #[no_mangle]
    pub extern "system" fn Java_dev_gpui_mobile_GpuiAccessibilityBridge_nativeAction(
        mut env: JNIEnv,
        _: JClass,
        id: jlong,
        host: JObject,
        node: jint,
        action: jint,
        arguments: JObject,
    ) -> jboolean {
        let Some(action) = PlatformAction::from_java(&mut env, action, &arguments) else {
            return 0;
        };
        let Some(state) = state(id) else {
            return 0;
        };
        let events = {
            let mut state = state.lock();
            state.last_query = Instant::now();
            let State {
                adapter, actions, ..
            } = &mut *state;
            adapter.perform_action(actions, node, &action)
        };
        if let Some(events) = events {
            events.raise(&mut env, &host);
            1
        } else {
            0
        }
    }
    #[no_mangle]
    pub extern "system" fn Java_dev_gpui_mobile_GpuiAccessibilityBridge_nativeHover(
        mut env: JNIEnv,
        _: JClass,
        id: jlong,
        host: JObject,
        action: jint,
        x: jfloat,
        y: jfloat,
    ) -> jboolean {
        let Some(state) = state(id) else {
            return 0;
        };
        let events = {
            let mut state = state.lock();
            state.query();
            let State {
                adapter,
                activation,
                ..
            } = &mut *state;
            adapter.on_hover_event(activation, action, x, y)
        };
        if let Some(events) = events {
            events.raise(&mut env, &host);
            1
        } else {
            0
        }
    }
    #[no_mangle]
    pub extern "system" fn Java_dev_gpui_mobile_GpuiAccessibilityBridge_nativeEvents(
        mut env: JNIEnv,
        _: JClass,
        id: jlong,
        host: JObject,
    ) {
        let Some(state) = state(id) else {
            return;
        };
        let events = std::mem::take(&mut state.lock().events);
        // Android may reenter the provider while events are raised. No state
        // lock may span this call.
        for events in events {
            events.raise(&mut env, &host);
        }
    }
    #[no_mangle]
    pub extern "system" fn Java_dev_gpui_mobile_GpuiAccessibilityBridge_nativeMode(
        _: JNIEnv,
        _: JClass,
        id: jlong,
        mode: jint,
    ) {
        let Some(state) = state(id) else {
            return;
        };
        let mut state = state.lock();
        state.mode = match mode {
            0 => Mode::Off,
            1 => Mode::OnDemand,
            _ => Mode::Eager,
        };
        if state.wanted() {
            state.activate();
        } else {
            state.deactivate();
        }
    }
    struct SetText<'a> {
        actions: &'a mut Actions,
        text: String,
        accepted: bool,
    }
    impl ActionHandler for SetText<'_> {
        fn do_action(&mut self, mut request: ActionRequest) {
            if !self.actions.editable.contains(&request.target_node) {
                return;
            }
            request.action = Action::SetValue;
            request.data = Some(ActionData::Value(self.text.clone().into()));
            self.actions.do_action(request);
            self.accepted = true;
        }
    }
    #[no_mangle]
    pub extern "system" fn Java_dev_gpui_mobile_GpuiAccessibilityBridge_nativeSetText(
        mut env: JNIEnv,
        _: JClass,
        id: jlong,
        host: JObject,
        node: jint,
        text: JString,
    ) -> jboolean {
        let Ok(text) = env.get_string(&text).map(String::from) else {
            return 0;
        };
        let Some(action) = PlatformAction::from_java(&mut env, 1, &JObject::null()) else {
            return 0;
        };
        let Some(state) = state(id) else {
            return 0;
        };
        let (events, accepted) = {
            let mut state = state.lock();
            let State {
                adapter, actions, ..
            } = &mut *state;
            let mut handler = SetText {
                actions,
                text,
                accepted: false,
            };
            let events = adapter.perform_action(&mut handler, node, &action);
            (events, handler.accepted)
        };
        if let Some(events) = events {
            events.raise(&mut env, &host);
        }
        u8::from(accepted)
    }
}
