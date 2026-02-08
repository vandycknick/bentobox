use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

pub struct Directory {
    pub prefix: PathBuf,
    pub data_home: Option<PathBuf>,
}

impl Directory {
    pub fn with_prefix<P: AsRef<Path>>(prefix: P) -> Directory {
        fn abspath(path: OsString) -> Option<PathBuf> {
            let path: PathBuf = PathBuf::from(path);
            if path.is_absolute() {
                Some(path)
            } else {
                None
            }
        }

        let home: Option<PathBuf> = std::env::home_dir();

        let data_home = std::env::var_os("XDG_DATA_HOME")
            .and_then(abspath)
            .or_else(|| home.as_ref().map(|home| home.join(".local/share")));

        Directory {
            prefix: prefix.as_ref().to_path_buf(),
            data_home: data_home.and_then(|d| Some(d.join("bento"))),
        }
    }

    pub fn get_data_home(&self) -> Option<PathBuf> {
        return self.data_home.as_ref().map(|h| h.join(&self.prefix));
    }
}
