//! macOS "Open With" integration.
//!
//! AppKit's built-in Apple Event handler for `kAEOpenDocuments` dispatches
//! to the `NSApplicationDelegate` by looking for (in order):
//!   1. `application:openURLs:`
//!   2. `application:openFiles:`
//!   3. `application:openFile:`
//!
//! winit owns the delegate (class `WinitApplicationDelegate`) and panics if
//! we replace it. We instead add the open methods to winit's existing
//! delegate class at runtime via `class_addMethod`.
//!
//! Timing matters: AppKit dispatches the queued `kAEOpenDocuments` event
//! during `-[NSApp finishLaunching]`. `applicationDidFinishLaunching:` (which
//! is when eframe calls our `App::new`) runs AFTER that — too late. We
//! subscribe to `NSApplicationWillFinishLaunchingNotification` from `main()`
//! before `eframe::run_native`; that notification fires at the start of
//! `finishLaunching`, when winit's delegate class is registered but before
//! AE dispatch. Our observer attaches the open methods there.

use std::sync::{mpsc, Mutex, OnceLock};

use objc2::ffi::class_addMethod;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, NSObject, Sel};
use objc2::{define_class, msg_send, sel, MainThreadOnly};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSNotification, NSNotificationCenter, NSNotificationName,
    NSObjectProtocol, NSString, NSURL,
};

// ── Global sender ─────────────────────────────────────────────────────────────

static FILE_TX: OnceLock<Mutex<mpsc::Sender<String>>> = OnceLock::new();

/// Store the channel sender. Call once from main thread.
pub fn set_sender(tx: mpsc::Sender<String>) {
    let _ = FILE_TX.set(Mutex::new(tx));
}

fn send_path(p: String) {
    if let Some(guard) = FILE_TX.get() {
        if let Ok(tx) = guard.lock() {
            let _ = tx.send(p);
        }
    }
}

// ── Method implementations added to winit's delegate class ────────────────────

// Type encodings: `v@:@@` = void(id, SEL, id, id)  ·  `c@:@@` = BOOL(id, SEL, id, id)
const ENC_V_AT_COLON_AT_AT: &[u8] = b"v@:@@\0";
const ENC_B_AT_COLON_AT_AT: &[u8] = b"c@:@@\0";

unsafe extern "C-unwind" fn m_open_urls(
    _this: *mut AnyObject,
    _sel: Sel,
    _app: *mut AnyObject,
    urls: *mut NSArray<NSURL>,
) {
    if urls.is_null() { return; }
    let urls: &NSArray<NSURL> = unsafe { &*urls };
    for url in urls.to_vec() {
        if let Some(p) = url.path() {
            send_path(p.to_string());
        }
    }
}

unsafe extern "C-unwind" fn m_open_files(
    _this: *mut AnyObject,
    _sel: Sel,
    _app: *mut AnyObject,
    files: *mut NSArray<NSString>,
) {
    if files.is_null() { return; }
    let files: &NSArray<NSString> = unsafe { &*files };
    for s in files.to_vec() {
        send_path(s.to_string());
    }
}

unsafe extern "C-unwind" fn m_open_file(
    _this: *mut AnyObject,
    _sel: Sel,
    _app: *mut AnyObject,
    file: *mut NSString,
) -> Bool {
    if file.is_null() { return Bool::NO; }
    let s: &NSString = unsafe { &*file };
    send_path(s.to_string());
    Bool::YES
}

/// Add the three open-document methods to winit's `WinitApplicationDelegate`
/// class. Must run after winit has registered the class but before AppKit
/// dispatches the `kAEOpenDocuments` Apple Event — i.e. from the
/// `applicationWillFinishLaunching:` notification.
fn attach_open_methods_to_winit_delegate() {
    let Some(cls) = AnyClass::get(c"WinitApplicationDelegate") else { return };
    unsafe {
        let cls_ptr = cls as *const AnyClass as *mut AnyClass;
        class_addMethod(
            cls_ptr,
            sel!(application:openURLs:),
            std::mem::transmute::<_, unsafe extern "C-unwind" fn()>(
                m_open_urls as unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut NSArray<NSURL>),
            ),
            ENC_V_AT_COLON_AT_AT.as_ptr() as *const _,
        );
        class_addMethod(
            cls_ptr,
            sel!(application:openFiles:),
            std::mem::transmute::<_, unsafe extern "C-unwind" fn()>(
                m_open_files as unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut NSArray<NSString>),
            ),
            ENC_V_AT_COLON_AT_AT.as_ptr() as *const _,
        );
        class_addMethod(
            cls_ptr,
            sel!(application:openFile:),
            std::mem::transmute::<_, unsafe extern "C-unwind" fn()>(
                m_open_file as unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut NSString) -> Bool,
            ),
            ENC_B_AT_COLON_AT_AT.as_ptr() as *const _,
        );
    }
}

// ── Bootstrap observer ────────────────────────────────────────────────────────
//
// A tiny NSObject subclass whose `-willFinishLaunching:` method is invoked by
// NSNotificationCenter when the app is about to finish launching. At that
// point winit's delegate class is registered but AppKit hasn't yet dispatched
// any queued Apple Events.

#[derive(Default)]
struct BootstrapIvars;

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "ColominAEBootstrap"]
    #[ivars = BootstrapIvars]
    struct Bootstrap;

    unsafe impl NSObjectProtocol for Bootstrap {}

    impl Bootstrap {
        #[unsafe(method(willFinishLaunching:))]
        fn will_finish_launching(&self, _note: &NSNotification) {
            attach_open_methods_to_winit_delegate();
        }
    }
);

impl Bootstrap {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(BootstrapIvars);
        unsafe { msg_send![super(this), init] }
    }
}

static BOOTSTRAP: OnceLock<usize> = OnceLock::new(); // store pointer to keep alive

/// Register a notification observer so our open methods get attached to
/// winit's delegate class at the right moment. Call from `main()` BEFORE
/// `eframe::run_native`.
pub fn install_bootstrap() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let bootstrap = Bootstrap::new(mtm);

    // Notification name: "NSApplicationWillFinishLaunchingNotification"
    let name = NSString::from_str("NSApplicationWillFinishLaunchingNotification");
    let notif_name: &NSNotificationName = &*name;

    let nc = NSNotificationCenter::defaultCenter();
    unsafe {
        nc.addObserver_selector_name_object(
            &*bootstrap,
            sel!(willFinishLaunching:),
            Some(notif_name),
            None,
        );
    }

    // Retain the bootstrap object for the lifetime of the process.
    let raw = Retained::into_raw(bootstrap) as usize;
    let _ = BOOTSTRAP.set(raw);
}
