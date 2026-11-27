//! The Linux shell's product directories.

use std::{
    env,
    ffi::OsStr,
    path::{Path, PathBuf},
};

use addict_core::dirs::Dirs;

/// Resolves the product directories once, before dispatching the invocation.
pub fn from_env() -> Dirs {
    resolve(
        env::var_os("XDG_CONFIG_HOME").as_deref(),
        env::var_os("XDG_DATA_HOME").as_deref(),
        env::var_os("XDG_CACHE_HOME").as_deref(),
        env::var_os("HOME").as_deref(),
    )
}

fn resolve(
    config_home: Option<&OsStr>,
    data_home: Option<&OsStr>,
    cache_home: Option<&OsStr>,
    home: Option<&OsStr>,
) -> Dirs {
    let home = home.filter(|home| !home.is_empty()).map(Path::new);
    Dirs {
        config: product_dir(config_home, home, ".config"),
        data: product_dir(data_home, home, ".local/share"),
        cache: product_dir(cache_home, home, ".cache"),
    }
}

fn product_dir(xdg: Option<&OsStr>, home: Option<&Path>, fallback: &str) -> Option<PathBuf> {
    let base = match xdg.map(Path::new) {
        Some(path) if path.is_absolute() => path.to_owned(),
        _ => home?.join(fallback),
    };
    Some(base.join("addict"))
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::ffi::OsStrExt};

    use addict_deepl::{
        DeepL,
        key::{self, KeySource},
    };
    use addict_dictionary::import::{Source, index_path};
    use addict_engine::{at_once, cache::Cached, english, language, my_language};

    use super::*;
    use crate::{capture, entries, gpu};

    fn os(text: &str) -> Option<&OsStr> {
        Some(OsStr::new(text))
    }

    #[test]
    fn an_absolute_config_home_holds_the_key_file() {
        let dirs = resolve(os("/x/config"), None, None, os("/home/u"));
        assert_eq!(
            key::key_path(dirs.config.as_deref()),
            Some(PathBuf::from("/x/config/addict/deepl.key"))
        );
    }

    #[test]
    fn an_unset_empty_or_relative_config_home_falls_back_to_home() {
        for config_home in [None, os(""), os("config"), os("./config")] {
            let dirs = resolve(config_home, None, None, os("/home/u"));
            assert_eq!(
                key::key_path(dirs.config.as_deref()),
                Some(PathBuf::from("/home/u/.config/addict/deepl.key")),
                "{config_home:?}"
            );
        }
    }

    #[test]
    fn without_a_config_home_and_a_home_there_is_no_key_file() {
        for home in [None, os("")] {
            for config_home in [None, os(""), os("relative")] {
                let dirs = resolve(config_home, None, None, home);
                assert_eq!(key::key_path(dirs.config.as_deref()), None);
            }
        }
    }

    #[test]
    fn the_key_file_lies_in_the_configuration_directory() {
        for config_home in [os("/x/config"), None, os("relative")] {
            let dirs = resolve(config_home, None, None, os("/home/u"));
            let dir = dirs.config.as_deref().expect("a directory");
            assert!(dir.ends_with("addict"), "{dir:?}");
            assert_eq!(key::key_path(Some(dir)), Some(dir.join("deepl.key")));
        }
        assert_eq!(resolve(None, None, None, None).config, None);
    }

    #[test]
    fn the_data_directory_follows_the_base_directory_rules() {
        assert_eq!(
            resolve(None, os("/data"), None, os("/home/u")).data,
            Some(PathBuf::from("/data/addict"))
        );
        for ignored in [None, os("relative/data"), os("")] {
            assert_eq!(
                resolve(None, ignored, None, os("/home/u")).data,
                Some(PathBuf::from("/home/u/.local/share/addict"))
            );
            for home in [None, os("")] {
                assert_eq!(resolve(None, ignored, None, home).data, None);
            }
        }
        assert_eq!(
            resolve(None, os("/data"), None, None).data,
            Some(PathBuf::from("/data/addict"))
        );
    }

    #[test]
    fn the_cache_directory_follows_the_base_directory_specification() {
        assert_eq!(
            resolve(None, None, os("/x/cache"), os("/home/u")).cache,
            Some(PathBuf::from("/x/cache/addict"))
        );
        for ignored in [None, os("relative"), os("")] {
            assert_eq!(
                resolve(None, None, ignored, os("/home/u")).cache,
                Some(PathBuf::from("/home/u/.cache/addict"))
            );
            for home in [None, os("")] {
                assert_eq!(resolve(None, None, ignored, home).cache, None);
            }
        }
    }

    #[test]
    fn each_directory_resolves_independently_and_preserves_home_bytes() {
        let home = OsStr::from_bytes(b"relative/\xffhome");
        let dirs = resolve(os("relative-config"), os("/data"), os(""), Some(home));
        assert_eq!(dirs.config, Some(Path::new(home).join(".config/addict")));
        assert_eq!(dirs.data, Some(PathBuf::from("/data/addict")));
        assert_eq!(dirs.cache, Some(Path::new(home).join(".cache/addict")));

        let dirs = resolve(None, os("/data"), os("relative"), None);
        assert!(dirs.config.is_none());
        assert_eq!(dirs.data, Some(PathBuf::from("/data/addict")));
        assert!(dirs.cache.is_none());
    }

    #[test]
    fn product_files_keep_their_linux_paths() {
        struct Temp(PathBuf);

        impl Drop for Temp {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }

        let root = env::temp_dir().join(format!("addict-paths-{}", std::process::id()));
        fs::create_dir(&root).expect("temporary directory");
        let root = Temp(root);
        let home = root.0.join("home");
        let config = root.0.join("config");
        let data = root.0.join("data");
        let cache = root.0.join("cache");
        let cases = [
            (
                resolve(
                    Some(config.as_os_str()),
                    Some(data.as_os_str()),
                    Some(cache.as_os_str()),
                    Some(home.as_os_str()),
                ),
                config.join("addict"),
                data.join("addict"),
                cache.join("addict"),
            ),
            (
                resolve(None, None, None, Some(home.as_os_str())),
                home.join(".config/addict"),
                home.join(".local/share/addict"),
                home.join(".cache/addict"),
            ),
        ];
        for (dirs, config, data, cache) in cases {
            let config_dir = dirs.config.as_deref();
            let (program, tessdata) = capture::tesseract_paths(config_dir);
            for (actual, name) in [
                (language::path(config_dir), "language"),
                (my_language::path(config_dir), "my-language"),
                (english::path(config_dir), "english"),
                (at_once::path(config_dir), "translate-sentence"),
                (
                    addict_controller::first_run_choice_path(config_dir),
                    "first-run-download",
                ),
                (entries::startup_file(config_dir), "login-startup"),
                (entries::shortcut_file(config_dir), "shortcut-generation"),
                (gpu::path(config_dir), "vulkan-drivers"),
                (program, "tesseract"),
                (tessdata, "tessdata"),
            ] {
                assert_eq!(actual, Some(config.join(name)));
            }
            let key_path = key::key_path(config_dir).expect("configuration directory");
            assert_eq!(key_path, config.join("deepl.key"));
            let deepl = DeepL::new(KeySource::new(key::read(&key_path)));
            let data_dir = dirs.data.as_deref().expect("data directory");
            assert_eq!(index_path(data_dir, Source::Oewn), data.join("oewn.index"));
            assert_eq!(
                index_path(data_dir, Source::KaikkiRu),
                data.join("kaikki-ru-Russkiy.index")
            );
            let cached = Cached::new(deepl, dirs.cache);
            cached.store().put(&[0; 32], "translation");
            assert!(cache.join("translations.sqlite3").is_file());
            assert!(cache.join("CACHEDIR.TAG").is_file());
        }
    }
}
