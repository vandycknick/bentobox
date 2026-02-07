use objc2_foundation::{
    NSSearchPathDirectory, NSSearchPathDomainMask, NSSearchPathForDirectoriesInDomains,
};

pub fn get_app_support_dir() -> Option<String> {
    let paths = unsafe {
        NSSearchPathForDirectoriesInDomains(
            NSSearchPathDirectory::ApplicationSupportDirectory,
            NSSearchPathDomainMask::UserDomainMask,
            true,
        )
    };

    paths.to_vec().first().map(|s| s.to_string())
}

pub fn get_cache_dir() -> Option<String> {
    let paths = unsafe {
        NSSearchPathForDirectoriesInDomains(
            NSSearchPathDirectory::CachesDirectory,
            NSSearchPathDomainMask::UserDomainMask,
            true,
        )
    };

    paths.to_vec().first().map(|s| s.to_string())
}

pub fn get_preferences_dir() -> Option<String> {
    let paths = unsafe {
        NSSearchPathForDirectoriesInDomains(
            NSSearchPathDirectory::PreferencePanesDirectory,
            NSSearchPathDomainMask::UserDomainMask,
            true,
        )
    };

    paths.to_vec().first().map(|s| s.to_string())
}
