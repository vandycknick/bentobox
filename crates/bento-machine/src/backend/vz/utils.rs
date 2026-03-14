use objc2_virtualization::{
    VZGenericPlatformConfiguration, VZLinuxRosettaAvailability, VZLinuxRosettaDirectoryShare,
    VZVirtualMachine,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RosettaAvailability {
    NotSupported,
    NotInstalled,
    Installed,
}

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

#[inline]
pub fn is_apple_silicon() -> bool {
    cfg!(target_arch = "aarch64")
}

#[inline]
pub fn vz_rosetta_availability() -> RosettaAvailability {
    match unsafe { VZLinuxRosettaDirectoryShare::availability() } {
        VZLinuxRosettaAvailability::NotSupported => RosettaAvailability::NotSupported,
        VZLinuxRosettaAvailability::NotInstalled => RosettaAvailability::NotInstalled,
        VZLinuxRosettaAvailability::Installed => RosettaAvailability::Installed,
        _ => RosettaAvailability::NotSupported,
    }
}
