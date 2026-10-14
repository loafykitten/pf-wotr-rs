//! Measure when frames actually reach the display.
//!
//! wgpu-hal keeps the `CAMetalDrawable` private, so we swizzle `-[CAMetalLayer nextDrawable]`
//! process-wide: the hook calls the original implementation and attaches a presented handler
//! to every drawable. Metal calls the handler with `presentedTime` (host time, the
//! `CACurrentMediaTime` clock), or 0 when the drawable was never shown (dropped).

use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use block2::RcBlock;
use objc2::runtime::{AnyClass, AnyObject, Imp, ProtocolObject, Sel};
use objc2::{msg_send, sel};
use objc2_metal::MTLDrawable;

/// One presented-handler callback.
#[derive(Debug, Clone, Copy)]
pub struct Presented {
    pub drawable_id: u64,
    /// Host time in seconds when the drawable was shown; 0 if it was dropped.
    pub presented_time: f64,
    /// Host time in seconds when `nextDrawable` returned it.
    pub acquired_time: f64,
}

static ORIGINAL: OnceLock<Imp> = OnceLock::new();
static RECORDING: AtomicBool = AtomicBool::new(false);
static ACQUIRED: AtomicUsize = AtomicUsize::new(0);
static LOG: Mutex<Vec<Presented>> = Mutex::new(Vec::new());
static LAYER_INFO: Mutex<Option<String>> = Mutex::new(None);

/// Host time now, on the same clock as `presentedTime`.
pub fn host_time() -> f64 {
    objc2_quartz_core::CACurrentMediaTime()
}

type NextDrawableFn = unsafe extern "C-unwind" fn(*mut AnyObject, Sel) -> *mut AnyObject;

unsafe extern "C-unwind" fn next_drawable_hook(this: *mut AnyObject, cmd: Sel) -> *mut AnyObject {
    let original = *ORIGINAL.get().expect("hook installed without original");
    // SAFETY: `original` is the implementation we replaced, with the same signature
    // (`-(id<CAMetalDrawable>)nextDrawable`), called with the same receiver and selector.
    let drawable = unsafe {
        let f: NextDrawableFn = std::mem::transmute(original);
        f(this, cmd)
    };
    let acquired_time = host_time();
    if LAYER_INFO.lock().map(|g| g.is_none()).unwrap_or(false) {
        // SAFETY: `this` is a live CAMetalLayer (we only hook that class's method).
        let (count, sync): (usize, bool) =
            unsafe { (msg_send![this, maximumDrawableCount], msg_send![this, displaySyncEnabled]) };
        if let Ok(mut g) = LAYER_INFO.lock() {
            *g = Some(format!("maximumDrawableCount={count} displaySyncEnabled={sync}"));
        }
    }
    if !drawable.is_null() && RECORDING.load(Ordering::Relaxed) {
        ACQUIRED.fetch_add(1, Ordering::Relaxed);
        // SAFETY: a non-null return from nextDrawable is a CAMetalDrawable, which conforms to
        // MTLDrawable; it is autoreleased but alive for the rest of this call.
        let drawable: &ProtocolObject<dyn MTLDrawable> = unsafe { &*(drawable.cast()) };
        let block = RcBlock::new(move |d: NonNull<ProtocolObject<dyn MTLDrawable>>| {
            // SAFETY: Metal passes the drawable that was presented, alive for the callback.
            let d = unsafe { d.as_ref() };
            let rec = Presented {
                drawable_id: d.drawableID() as u64,
                presented_time: d.presentedTime(),
                acquired_time,
            };
            if let Ok(mut log) = LOG.lock() {
                log.push(rec);
            }
        });
        // SAFETY: the block is copied by Metal (it retains it until the callback fires).
        unsafe { drawable.addPresentedHandler(RcBlock::as_ptr(&block)) };
    }
    drawable
}

/// Install the hook once. Safe to call repeatedly.
pub fn install() -> Result<(), String> {
    // Serialise installers, and publish the original before the hook can run.
    static INSTALL: Mutex<bool> = Mutex::new(false);
    let mut done = INSTALL.lock().map_err(|e| e.to_string())?;
    if *done {
        return Ok(());
    }
    let cls = AnyClass::get(c"CAMetalLayer").ok_or("CAMetalLayer class not found")?;
    let method = cls.instance_method(sel!(nextDrawable)).ok_or("nextDrawable not found")?;
    ORIGINAL.set(method.implementation()).map_err(|_| "hook installed twice".to_string())?;
    let hook: NextDrawableFn = next_drawable_hook;
    // SAFETY: the hook has the exact ABI of `-nextDrawable` and forwards to the original.
    unsafe { method.set_implementation(std::mem::transmute::<NextDrawableFn, Imp>(hook)) };
    *done = true;
    Ok(())
}

/// Start (or stop) recording presented handlers, clearing the previous log when starting.
pub fn set_recording(on: bool) {
    if on {
        if let Ok(mut log) = LOG.lock() {
            log.clear();
        }
        ACQUIRED.store(0, Ordering::Relaxed);
    }
    RECORDING.store(on, Ordering::Relaxed);
}

/// Take the recorded callbacks (sorted by drawable ID) and the number of drawables acquired.
pub fn take() -> (Vec<Presented>, usize) {
    let mut v = LOG.lock().map(|mut g| std::mem::take(&mut *g)).unwrap_or_default();
    v.sort_by_key(|p| p.drawable_id);
    (v, ACQUIRED.load(Ordering::Relaxed))
}

/// The CAMetalLayer configuration seen at the first `nextDrawable`.
pub fn layer_info() -> Option<String> {
    LAYER_INFO.lock().ok().and_then(|g| g.clone())
}
