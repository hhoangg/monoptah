use super::Forwarding;
use crate::shell::quit_gate::is_quit_hotkey;
use gpui::Window;
use objc2::{MainThreadMarker, msg_send, rc::Retained, runtime::AnyObject};
use objc2_app_kit::{NSEvent, NSEventMask, NSEventModifierFlags, NSResponder, NSView};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    ptr::{self, NonNull},
    rc::Rc,
    time::Instant,
};

/// A window-local monitor reaches GPUI even when WKWebView or AVPlayerView is
/// AppKit's first responder. GPUI still translates the original NSEvent, so
/// keyboard layouts and configured shortcuts use its normal matching rules.
pub(super) struct NativeKeys {
    monitor: Retained<AnyObject>,
}
impl NativeKeys {
    pub(super) fn new(
        window: &Window,
        forwarding: Rc<Forwarding>,
        quit_press: Rc<dyn Fn(bool, Instant) -> bool>,
    ) -> Option<Self> {
        let main = MainThreadMarker::new()?;
        let handle = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return None;
        };
        let view = unsafe { Retained::retain(handle.ns_view.as_ptr().cast::<NSView>()) }?;
        let native_window = view.window()?;
        let block = block2::RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
            let original = event.as_ptr();
            let event = unsafe { event.as_ref() };
            if forwarding.native.get()
                || !event
                    .window(main)
                    .is_some_and(|w| ptr::eq(&*w, &*native_window))
            {
                return original;
            }
            // Cmd+Q is a menu key equivalent: returning the event would let
            // AppKit's menu quit at once. Swallow it, repeats included, and let
            // the quit gate time the hold. Menu clicks send no key event.
            let modifiers = event.modifierFlags();
            if modifiers.contains(NSEventModifierFlags::Command) {
                let characters = event.characters().map(|text| text.to_string());
                if is_quit_hotkey(
                    modifiers.0 as u64,
                    characters.as_deref().unwrap_or_default(),
                ) && quit_press(event.isARepeat(), Instant::now())
                {
                    return ptr::null_mut();
                }
            }
            let in_chord = forwarding
                .chord_deadline
                .get()
                .is_some_and(|until| std::time::Instant::now() < until);
            if !in_chord
                && !event.modifierFlags().intersects(
                    NSEventModifierFlags::Command
                        | NSEventModifierFlags::Control
                        | NSEventModifierFlags::Option
                        | NSEventModifierFlags::Function,
                )
            {
                return original;
            }
            let responder = native_window.firstResponder();
            if responder.as_ref().is_none_or(|r| {
                ptr::eq(
                    (&**r as *const NSResponder).cast::<()>(),
                    (&*view as *const NSView).cast::<()>(),
                )
            }) {
                return original;
            }
            forwarding.handled.set(false);
            forwarding.native.set(true);
            // Keep the native first responder: forwarding does not select or
            // blur the HTML form, text selection, or video controls.
            let _: bool = unsafe { msg_send![&view, performKeyEquivalent: event] };
            forwarding.native.set(false);
            if forwarding.handled.get() {
                ptr::null_mut()
            } else {
                original
            }
        });
        let monitor = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &block)
        }?;
        Some(Self { monitor })
    }
}
impl Drop for NativeKeys {
    fn drop(&mut self) {
        unsafe { NSEvent::removeMonitor(&self.monitor) };
    }
}
