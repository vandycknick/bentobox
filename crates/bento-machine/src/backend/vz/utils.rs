use objc2_virtualization::{VZGenericPlatformConfiguration, VZVirtualMachine};

pub fn os_version() -> (i64, i64, i64) {
    use objc2_foundation::NSProcessInfo;
    let version = NSProcessInfo::processInfo().operatingSystemVersion();
    (
        version.majorVersion as i64,
        version.minorVersion as i64,
        version.patchVersion as i64,
    )
}

pub fn is_os_version_at_least(major: i64, minor: i64, patch: i64) -> bool {
    let current = os_version();
    current >= (major, minor, patch)
}

#[inline]
pub fn vz_virtual_machine_is_supported() -> bool {
    unsafe { VZVirtualMachine::isSupported() }
}

#[inline]
pub fn vz_nested_virtualization_is_supported() -> bool {
    unsafe { VZGenericPlatformConfiguration::isNestedVirtualizationSupported() }
}
