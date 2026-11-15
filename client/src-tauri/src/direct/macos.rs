//! End the trial session across sleep/wake instead of reviving stale input.
use objc2::{class, msg_send, rc::Retained, runtime::AnyObject};
use objc2_foundation::NSString;
use tauri::{AppHandle, Manager};

pub fn observe_sleep(app: &AppHandle) {
    thread_local! {static TOKENS:std::cell::RefCell<Vec<Retained<AnyObject>>> = const {std::cell::RefCell::new(Vec::new())};}
    unsafe {
        let workspace: Retained<AnyObject> = msg_send![class!(NSWorkspace), sharedWorkspace];
        let center: Retained<AnyObject> = msg_send![&*workspace, notificationCenter];
        for name in [
            "NSWorkspaceWillSleepNotification",
            "NSWorkspaceDidWakeNotification",
        ] {
            let app = app.clone();
            let callback = block2::RcBlock::new(move |_: *mut AnyObject| {
                app.state::<crate::AppState>().direct.stop(&app);
            });
            let name = NSString::from_str(name);
            let token: Retained<AnyObject> = msg_send![&*center,addObserverForName:&*name object:std::ptr::null::<AnyObject>() queue:std::ptr::null::<AnyObject>() usingBlock:&*callback];
            TOKENS.with(|tokens| tokens.borrow_mut().push(token));
        }
    }
}
