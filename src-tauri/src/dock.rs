use objc2::{AllocAnyThread, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSImage};
use objc2_foundation::NSData;
use tauri::AppHandle;

const BARN: &[u8] = include_bytes!("../icons/icon.png");

pub fn show_barn(app: &AppHandle) {
    let dispatched = app.run_on_main_thread(|| {
        let Some(main_thread) = MainThreadMarker::new() else {
            log::warn!("Could not set the Dock icon off the main thread");
            return;
        };
        let Some(barn) = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(BARN)) else {
            log::warn!("Could not read the Dock icon");
            return;
        };
        unsafe {
            NSApplication::sharedApplication(main_thread).setApplicationIconImage(Some(&barn))
        };
    });
    if let Err(error) = dispatched {
        log::warn!("Could not set the Dock icon: {error}");
    }
}
