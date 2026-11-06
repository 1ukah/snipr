use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};

fn app(pid: u32) -> Option<objc2::rc::Retained<NSRunningApplication>> {
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid as i32)
}

pub fn hide(pid: u32) -> bool {
    app(pid).is_some_and(|a| a.hide())
}

pub fn show(pid: u32) -> bool {
    app(pid).is_some_and(|a| {
        a.unhide();
        #[allow(deprecated)]
        a.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps)
    })
}

pub fn is_hidden(pid: u32) -> Option<bool> {
    app(pid).map(|a| a.isHidden())
}
