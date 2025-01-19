use objc2::runtime::Bool;
use objc2_foundation::{
    NSSearchPathDirectory, NSSearchPathDomainMask, NSSearchPathForDirectoriesInDomains,
};

pub fn get_app_support_dir() -> Option<String> {
    let paths = unsafe {
        NSSearchPathForDirectoriesInDomains(
            NSSearchPathDirectory::NSApplicationSupportDirectory,
            NSSearchPathDomainMask::NSUserDomainMask,
            Bool::YES,
        )
    };

    let paths = unsafe { paths.as_ref() };
    paths.to_vec_retained().first().map(|s| s.to_string())
}

pub fn get_cache_dir() -> Option<String> {
    let paths = unsafe {
        NSSearchPathForDirectoriesInDomains(
            NSSearchPathDirectory::NSCachesDirectory,
            NSSearchPathDomainMask::NSUserDomainMask,
            Bool::YES,
        )
    };

    let paths = unsafe { paths.as_ref() };
    paths.to_vec_retained().first().map(|s| s.to_string())
}

pub fn get_preferences_dir() -> Option<String> {
    let paths = unsafe {
        NSSearchPathForDirectoriesInDomains(
            NSSearchPathDirectory::NSPreferencePanesDirectory,
            NSSearchPathDomainMask::NSUserDomainMask,
            Bool::YES,
        )
    };

    let paths = unsafe { paths.as_ref() };
    paths.to_vec_retained().first().map(|s| s.to_string())
}
