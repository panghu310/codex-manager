#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StartupPermissionStatus {
    pub screen_capture_granted: bool,
    pub accessibility_trusted: bool,
}

trait PermissionApi {
    fn screen_capture_granted(&self) -> bool;
    fn accessibility_trusted(&self) -> bool;
}

fn check_startup_permissions_with(api: &impl PermissionApi) -> StartupPermissionStatus {
    StartupPermissionStatus {
        screen_capture_granted: api.screen_capture_granted(),
        accessibility_trusted: api.accessibility_trusted(),
    }
}

pub fn check_startup_permissions() -> StartupPermissionStatus {
    platform::check_startup_permissions()
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{check_startup_permissions_with, PermissionApi, StartupPermissionStatus};

    type Boolean = u8;

    pub fn check_startup_permissions() -> StartupPermissionStatus {
        check_startup_permissions_with(&MacPermissionApi)
    }

    struct MacPermissionApi;

    impl PermissionApi for MacPermissionApi {
        fn screen_capture_granted(&self) -> bool {
            unsafe { CGPreflightScreenCaptureAccess() }
        }

        fn accessibility_trusted(&self) -> bool {
            unsafe { AXIsProcessTrusted() != 0 }
        }
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> Boolean;
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::StartupPermissionStatus;

    pub fn check_startup_permissions() -> StartupPermissionStatus {
        StartupPermissionStatus::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakePermissionApi {
        screen_capture_granted: bool,
        accessibility_trusted: bool,
    }

    impl PermissionApi for FakePermissionApi {
        fn screen_capture_granted(&self) -> bool {
            self.screen_capture_granted
        }

        fn accessibility_trusted(&self) -> bool {
            self.accessibility_trusted
        }
    }

    #[test]
    fn startup_permissions_report_missing_permissions_without_requesting() {
        let api = FakePermissionApi::default();

        let status = check_startup_permissions_with(&api);

        assert_eq!(
            status,
            StartupPermissionStatus {
                screen_capture_granted: false,
                accessibility_trusted: false,
            }
        );
    }

    #[test]
    fn startup_permissions_report_already_granted_permissions() {
        let api = FakePermissionApi {
            screen_capture_granted: true,
            accessibility_trusted: true,
            ..Default::default()
        };

        let status = check_startup_permissions_with(&api);

        assert_eq!(
            status,
            StartupPermissionStatus {
                screen_capture_granted: true,
                accessibility_trusted: true,
            }
        );
    }
}
