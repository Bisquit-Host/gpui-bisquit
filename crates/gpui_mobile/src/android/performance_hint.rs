//! An ADPF hint session for the render thread, as HWUI keeps its own clocks up.
//! Each drawn frame reports its duration against a 120 Hz budget, and the
//! first frame after idle announces a workload increase so CPU and GPU clocks
//! rise before the frame rather than ~100ms into an animation.
use std::{
    cell::{Cell, OnceCell},
    ffi::c_void,
    time::{Duration, Instant},
};

const TARGET: Duration = Duration::from_nanos(1_000_000_000 / 120);
/// Clocks drop after this long without a drawn frame.
const IDLE: Duration = Duration::from_millis(100);

type GetManagerFn = unsafe extern "C" fn() -> *mut c_void;
type CreateSessionFn = unsafe extern "C" fn(*mut c_void, *const i32, usize, i64) -> *mut c_void;
type ReportFn = unsafe extern "C" fn(*mut c_void, i64) -> i32;
/// `APerformanceHint_notifyWorkloadIncrease`, API 36.
type IncreaseFn = unsafe extern "C" fn(*mut c_void, bool, bool, *const libc::c_char) -> i32;

struct Session {
    session: *mut c_void,
    report: ReportFn,
    increase: Option<IncreaseFn>,
}

thread_local! {
    static SESSION: OnceCell<Option<Session>> = const { OnceCell::new() };
    static LAST_FRAME: Cell<Option<Instant>> = const { Cell::new(None) };
    static DREW: Cell<bool> = const { Cell::new(false) };
}

fn symbol<T>(name: &[u8]) -> Option<T> {
    debug_assert_eq!(name.last().copied(), Some(0));
    let pointer = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr().cast()) };
    // SAFETY: callers name a libandroid symbol whose C signature is `T`.
    (!pointer.is_null()).then(|| unsafe { std::mem::transmute_copy::<*mut c_void, T>(&pointer) })
}

fn open() -> Option<Session> {
    let get_manager: GetManagerFn = symbol(b"APerformanceHint_getManager\0")?;
    let create: CreateSessionFn = symbol(b"APerformanceHint_createSession\0")?;
    let report: ReportFn = symbol(b"APerformanceHint_reportActualWorkDuration\0")?;
    let increase = symbol::<IncreaseFn>(b"APerformanceHint_notifyWorkloadIncrease\0");
    let manager = unsafe { get_manager() };
    if manager.is_null() {
        return None;
    }
    let thread = unsafe { libc::gettid() };
    let session = unsafe { create(manager, &thread, 1, TARGET.as_nanos() as i64) };
    if session.is_null() {
        return None;
    }
    log::info!(
        "performance hint session open, workload increase hints: {}",
        increase.is_some()
    );
    Some(Session {
        session,
        report,
        increase,
    })
}

fn with_session(f: impl FnOnce(&Session)) {
    SESSION.with(|cell| {
        if let Some(session) = cell.get_or_init(open) {
            f(session);
        }
    });
}

/// Call on the render thread before a frame that may draw.
pub(crate) fn frame_starting(now: Instant) {
    let resumed = LAST_FRAME
        .replace(Some(now))
        .is_none_or(|last| now.duration_since(last) > IDLE);
    DREW.set(false);
    if resumed {
        with_session(|session| {
            if let Some(increase) = session.increase {
                unsafe {
                    increase(
                        session.session,
                        true,
                        true,
                        c"gpui frame after idle".as_ptr(),
                    )
                };
            }
        });
    }
}

/// Call on the render thread when the renderer presents.
pub(crate) fn mark_drawn() {
    DREW.set(true);
}

/// Call on the render thread after a frame callback; reports it if it drew.
pub(crate) fn frame_finished(started: Instant) {
    if !DREW.get() {
        return;
    }
    let now = Instant::now();
    LAST_FRAME.set(Some(now));
    with_session(|session| unsafe {
        (session.report)(
            session.session,
            now.duration_since(started).as_nanos() as i64,
        );
    });
}
