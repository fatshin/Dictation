// Read the focused text field's value via the macOS Accessibility API.
//
// Returns None when:
//   - no focused UI element exists
//   - the focused element has no AXValue / value is not a string
//     (typical for Electron apps like Slack / Notion / VSCode / Cursor /
//     Discord — falls back to ASR-only)
//   - Accessibility permission has not been granted
//
// Long fields are truncated to MAX_CONTEXT_CHARS to avoid blowing the LLM
// context window. The privacy posture (README) says nothing leaves the device,
// but we still cap to keep the prompt small and avoid sending whole documents
// to the local LLM.

#[cfg(target_os = "macos")]
pub use mac::{ensure_ax_trusted, get_focused_field_context, is_ax_trusted};

#[cfg(not(target_os = "macos"))]
pub fn get_focused_field_context() -> Option<FocusedFieldContext> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn ensure_ax_trusted() -> bool {
    true
}

#[cfg(not(target_os = "macos"))]
pub fn is_ax_trusted() -> bool {
    true
}

#[derive(Debug, Clone, Default)]
pub struct FocusedFieldContext {
    pub text: String,
    pub truncated: bool,
}

const MAX_CONTEXT_CHARS: usize = 4096;

#[cfg(target_os = "macos")]
mod mac {
    use super::{FocusedFieldContext, MAX_CONTEXT_CHARS};
    use accessibility_sys::{
        kAXErrorSuccess, kAXFocusedUIElementAttribute, kAXTrustedCheckOptionPrompt,
        kAXValueAttribute, AXIsProcessTrusted, AXIsProcessTrustedWithOptions,
        AXUIElementCopyAttributeValue, AXUIElementCreateSystemWide, AXUIElementRef,
    };
    use core_foundation::base::{CFGetTypeID, CFTypeRef, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::{CFString, CFStringGetTypeID, CFStringRef};
    use std::ffi::c_void;

    /// Returns true if the app already holds Accessibility permission.
    /// Does NOT prompt the user.
    pub fn is_ax_trusted() -> bool {
        unsafe { AXIsProcessTrusted() }
    }

    /// Triggers the macOS Accessibility consent dialog (deeplinks to System
    /// Settings) if not yet granted. Idempotent — returns the current trust
    /// state. Safe to call once on app startup.
    pub fn ensure_ax_trusted() -> bool {
        unsafe {
            let key = CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt);
            let val = CFBoolean::true_value();
            let dict = CFDictionary::from_CFType_pairs(&[(key, val)]);
            AXIsProcessTrustedWithOptions(dict.as_concrete_TypeRef())
        }
    }

    pub fn get_focused_field_context() -> Option<FocusedFieldContext> {
        unsafe {
            let system = AXUIElementCreateSystemWide();
            if system.is_null() {
                return None;
            }
            // Take ownership so it gets released when this scope exits.
            let _system_guard = OwnedAxRef(system);

            let focused = copy_attr(system, kAXFocusedUIElementAttribute)?;
            let focused_elem = focused.0 as AXUIElementRef;

            let value_ref = copy_attr(focused_elem, kAXValueAttribute)?;
            if value_ref.0.is_null() {
                return None;
            }
            // AXValue is not always a CFString — sliders, ranges, etc. return
            // AXValueRef. Skip non-string values to avoid UB.
            if CFGetTypeID(value_ref.0) != CFStringGetTypeID() {
                return None;
            }
            let cf_str: CFStringRef = value_ref.0 as CFStringRef;
            let s = CFString::wrap_under_get_rule(cf_str).to_string();
            // We retained `value_ref` and `focused`; OwnedCFType drops them on
            // scope exit. CFString::wrap_under_get_rule retained again, so the
            // String we return is fully detached.
            drop(value_ref);
            drop(focused);

            let (text, truncated) = if s.chars().count() > MAX_CONTEXT_CHARS {
                let truncated: String = s.chars().take(MAX_CONTEXT_CHARS).collect();
                (truncated, true)
            } else {
                (s, false)
            };

            if text.is_empty() {
                None
            } else {
                Some(FocusedFieldContext { text, truncated })
            }
        }
    }

    /// Wraps an AX/CF reference returned from a Copy* call so it gets released
    /// on drop. The pointer is always +1 retain at this point.
    struct OwnedCFType(CFTypeRef);
    impl Drop for OwnedCFType {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { core_foundation::base::CFRelease(self.0) };
            }
        }
    }

    /// AXUIElement is a CFType subclass; release with CFRelease.
    struct OwnedAxRef(AXUIElementRef);
    impl Drop for OwnedAxRef {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { core_foundation::base::CFRelease(self.0 as CFTypeRef) };
            }
        }
    }

    unsafe fn copy_attr(element: AXUIElementRef, attr_name: &str) -> Option<OwnedCFType> {
        if element.is_null() {
            return None;
        }
        let cf_attr = CFString::new(attr_name);
        let mut value: CFTypeRef = std::ptr::null();
        let err = AXUIElementCopyAttributeValue(
            element,
            cf_attr.as_concrete_TypeRef(),
            &mut value as *mut CFTypeRef as *mut *const c_void,
        );
        if err != kAXErrorSuccess || value.is_null() {
            return None;
        }
        Some(OwnedCFType(value))
    }
}
