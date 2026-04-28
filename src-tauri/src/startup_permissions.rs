#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StartupPermissionPrompts {
    pub screen_capture_requested: bool,
    pub accessibility_prompted: bool,
}

trait PermissionApi {
    fn screen_capture_granted(&self) -> bool;
    fn request_screen_capture(&self) -> bool;
    fn accessibility_trusted(&self) -> bool;
    fn request_accessibility_prompt(&self) -> bool;
}

fn request_startup_permissions_with(api: &impl PermissionApi) -> StartupPermissionPrompts {
    let mut prompts = StartupPermissionPrompts::default();
    if !api.screen_capture_granted() {
        api.request_screen_capture();
        prompts.screen_capture_requested = true;
    }
    if !api.accessibility_trusted() {
        api.request_accessibility_prompt();
        prompts.accessibility_prompted = true;
    }
    prompts
}

pub fn request_startup_permissions() -> StartupPermissionPrompts {
    platform::request_startup_permissions()
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{request_startup_permissions_with, PermissionApi, StartupPermissionPrompts};
    use std::ffi::c_void;
    use std::ptr;

    type Boolean = u8;
    type CFIndex = isize;
    type CFAllocatorRef = *const c_void;
    type CFDictionaryRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFTypeRef = *const c_void;

    pub fn request_startup_permissions() -> StartupPermissionPrompts {
        request_startup_permissions_with(&MacPermissionApi)
    }

    struct MacPermissionApi;

    impl PermissionApi for MacPermissionApi {
        fn screen_capture_granted(&self) -> bool {
            unsafe { CGPreflightScreenCaptureAccess() }
        }

        fn request_screen_capture(&self) -> bool {
            unsafe { CGRequestScreenCaptureAccess() }
        }

        fn accessibility_trusted(&self) -> bool {
            unsafe { AXIsProcessTrusted() != 0 }
        }

        fn request_accessibility_prompt(&self) -> bool {
            request_accessibility_prompt()
        }
    }

    fn request_accessibility_prompt() -> bool {
        let keys = [unsafe { kAXTrustedCheckOptionPrompt as CFTypeRef }];
        let values = [unsafe { kCFBooleanTrue as CFTypeRef }];
        let options = unsafe {
            CFDictionaryCreate(
                ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                1,
                ptr::null(),
                ptr::null(),
            )
        };
        if options.is_null() {
            return false;
        }
        let trusted = unsafe { AXIsProcessTrustedWithOptions(options) != 0 };
        unsafe { CFRelease(options as CFTypeRef) };
        trusted
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        static kAXTrustedCheckOptionPrompt: CFStringRef;
        fn AXIsProcessTrusted() -> Boolean;
        fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> Boolean;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFBooleanTrue: CFTypeRef;
        fn CFDictionaryCreate(
            allocator: CFAllocatorRef,
            keys: *const CFTypeRef,
            values: *const CFTypeRef,
            num_values: CFIndex,
            key_callbacks: *const c_void,
            value_callbacks: *const c_void,
        ) -> CFDictionaryRef;
        fn CFRelease(cf: CFTypeRef);
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::StartupPermissionPrompts;

    pub fn request_startup_permissions() -> StartupPermissionPrompts {
        StartupPermissionPrompts::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[derive(Default)]
    struct FakePermissionApi {
        screen_capture_granted: bool,
        accessibility_trusted: bool,
        screen_capture_requests: Cell<usize>,
        accessibility_prompts: Cell<usize>,
    }

    impl PermissionApi for FakePermissionApi {
        fn screen_capture_granted(&self) -> bool {
            self.screen_capture_granted
        }

        fn request_screen_capture(&self) -> bool {
            self.screen_capture_requests
                .set(self.screen_capture_requests.get() + 1);
            true
        }

        fn accessibility_trusted(&self) -> bool {
            self.accessibility_trusted
        }

        fn request_accessibility_prompt(&self) -> bool {
            self.accessibility_prompts
                .set(self.accessibility_prompts.get() + 1);
            false
        }
    }

    #[test]
    fn startup_permissions_request_missing_screen_capture_and_accessibility() {
        let api = FakePermissionApi::default();

        let prompts = request_startup_permissions_with(&api);

        assert_eq!(
            prompts,
            StartupPermissionPrompts {
                screen_capture_requested: true,
                accessibility_prompted: true,
            }
        );
        assert_eq!(api.screen_capture_requests.get(), 1);
        assert_eq!(api.accessibility_prompts.get(), 1);
    }

    #[test]
    fn startup_permissions_skip_already_granted_permissions() {
        let api = FakePermissionApi {
            screen_capture_granted: true,
            accessibility_trusted: true,
            ..Default::default()
        };

        let prompts = request_startup_permissions_with(&api);

        assert_eq!(prompts, StartupPermissionPrompts::default());
        assert_eq!(api.screen_capture_requests.get(), 0);
        assert_eq!(api.accessibility_prompts.get(), 0);
    }
}
