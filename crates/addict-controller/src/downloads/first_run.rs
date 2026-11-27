//! First-run eligibility and the persisted decline.

use addict_core::translation::Source as Language;
use addict_dictionary::{
    pairs,
    release::{self, Manifest, Published},
};
use addict_engine::{
    dictionaries::Indexes,
    download::{DownloadError, Downloader},
};
use std::{
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
};

/// The name of the choice file inside the configuration directory.
const CHOICE_FILE: &str = "first-run-download";

/// What the choice file holds once Not now was pressed.
const DECLINED: &str = "declined";

/// How many bytes of the file are read.
const CHOICE_LIMIT: u64 = 64;

/// The choice file of the product, or `None` when no configuration directory
/// was supplied. Nothing is read here.
pub fn path(config: Option<&Path>) -> Option<PathBuf> {
    Some(config?.join(CHOICE_FILE))
}

/// Whether the file at `path` says that the offer was declined. No path, a
/// missing or unreadable file and anything but `declined`, white space
/// around it aside, is no choice. This reads the file, so it runs on the
/// runner.
pub fn read(path: Option<&Path>) -> bool {
    path.and_then(|path| read_choice(path).ok())
        .is_some_and(|content| content.trim() == DECLINED)
}

/// The first bytes of the regular file at `path`, as text.
fn read_choice(path: &Path) -> io::Result<String> {
    // Before opening: opening a pipe would wait for its writer.
    if !fs::metadata(path)?.is_file() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut content = String::new();
    File::open(path)?
        .take(CHOICE_LIMIT)
        .read_to_string(&mut content)?;
    Ok(content)
}

/// Writes that the offer was declined to the file at `path`, making its
/// directory when there is none. This writes a file, so it runs on the
/// runner.
pub fn write(path: Option<&Path>) -> io::Result<()> {
    let path = path.ok_or(io::ErrorKind::NotFound)?;
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)?;
    }
    fs::write(path, format!("{DECLINED}\n"))
}

/// Whether the offer is due for `my`: every required index is missing and
/// the user has not declined.
pub fn should_offer(indexes: &Indexes, my: Language, chosen: bool) -> bool {
    !chosen && indexes.required_indexes_missing(my)
}

/// The indexes of `manifest` that `my` needs and that are not `present`, by
/// name, in the order of the manifest: what the first-run offer lists and
/// installs. A name `my` needs that the manifest lacks is not listed.
pub fn offered(
    manifest: &[Published],
    my: Language,
    present: impl Fn(&str) -> bool,
) -> Vec<Published> {
    let required: Vec<&str> = pairs::required_index_names(my).collect();
    manifest
        .iter()
        .filter(|published| {
            required.contains(&published.name.as_str()) && !present(&published.name)
        })
        .cloned()
        .collect()
}

/// The indexes of my language's set that `indexes` does not show present,
/// in the set's order: what a change of my language in Settings offers.
pub fn missing(indexes: &Indexes, my: Language) -> Vec<&'static str> {
    pairs::required_index_names(my)
        .filter(|name| !is_present(indexes, name))
        .collect()
}

/// Whether the index `name` is open or unreadable in `indexes`; a name this
/// build does not know is not.
pub fn is_present(indexes: &Indexes, name: &str) -> bool {
    release::source_of(name).is_some_and(|source| indexes.presence.contains(source))
}

/// The manifest to offer from, when the offer is due for `my` given the
/// `indexes` and the `chosen` answer: `Ok(None)` when it is not due, which asks nothing of
/// `downloader`, and an error when the manifest cannot be had or lists no
/// index, which shows no dialog and keeps no choice, so the next start asks
/// again.
pub async fn offered_manifest(
    indexes: Indexes,
    my: Language,
    chosen: bool,
    downloader: &Downloader,
) -> Result<Option<Manifest>, DownloadError> {
    if !should_offer(&indexes, my, chosen) {
        return Ok(None);
    }
    let manifest = downloader.manifest().await?;
    if manifest.indexes.is_empty() {
        return Err(DownloadError::Manifest {
            cause: "it lists no index".to_owned(),
        });
    }
    Ok(Some(manifest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deadlines::MANIFEST_LIMIT;
    use addict_dictionary::Index;
    use addict_dictionary::sources;
    use addict_engine::download::recovery_manifest;
    use addict_engine::view::first_run::{Queue, RowState, nothing_text};

    fn row_state<'a>(queue: &'a Queue, name: &str) -> Option<&'a RowState> {
        queue
            .rows()
            .iter()
            .find(|(published, _)| published.name == name)
            .map(|(_, state)| state)
    }

    /// A directory of one test, removed when the test ends.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("addict-first-run-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("the directory of the test");
            Self(path)
        }

        fn file(&self) -> PathBuf {
            self.0.join("addict").join(CHOICE_FILE)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A missing or garbled manifest gives an error, so no dialog shows and
    /// nothing is kept; with an index or a choice, the release is not asked.
    #[tokio::test]
    async fn the_offer_needs_the_manifest_and_asks_it_only_when_due() {
        use std::collections::HashMap;

        use crate::stub::{Route, Stub};

        let manifest_path = format!("/{}/{}", release::tag(), release::MANIFEST_3);
        let empty = Stub::start(HashMap::new()).expect("the stub");
        let downloader = Downloader::with_base_url(empty.base_url());
        let error = offered_manifest(Indexes::MISSING, Language::Ru, false, &downloader)
            .await
            .expect_err("no manifest");
        assert!(
            matches!(error, DownloadError::Status { status: 404, .. }),
            "{error:?}"
        );

        let garbled = Stub::start(HashMap::from([(
            manifest_path.clone(),
            Route::File(b"<html>Not Found</html>".to_vec()),
        )]))
        .expect("the stub");
        let downloader = Downloader::with_base_url(garbled.base_url());
        let error = offered_manifest(Indexes::MISSING, Language::Ru, false, &downloader)
            .await
            .expect_err("garbled");
        assert!(matches!(error, DownloadError::Manifest { .. }), "{error:?}");

        // A manifest that lists no index offers nothing either.
        let nothing = Manifest {
            format: addict_dictionary::format::VERSION,
            indexes: Vec::new(),
        };
        let empty_list = Stub::start(HashMap::from([(
            manifest_path.clone(),
            Route::File(nothing.to_json().into_bytes()),
        )]))
        .expect("the stub");
        let downloader = Downloader::with_base_url(empty_list.base_url());
        let error = offered_manifest(Indexes::MISSING, Language::Ru, false, &downloader)
            .await
            .expect_err("no index");
        assert!(matches!(error, DownloadError::Manifest { .. }), "{error:?}");
        assert!(error.to_string().contains("no index"), "{error}");

        let published = Manifest {
            format: addict_dictionary::format::VERSION,
            indexes: vec![addict_dictionary::release::Published::of("oewn", b"xz")],
        };
        let good = Stub::start(HashMap::from([(
            manifest_path.clone(),
            Route::File(published.to_json().into_bytes()),
        )]))
        .expect("the stub");
        let downloader = Downloader::with_base_url(good.base_url());
        let due = offered_manifest(Indexes::MISSING, Language::Ru, false, &downloader).await;
        assert_eq!(due.expect("due"), Some(published));
        assert_eq!(good.seen().len(), 1);
        for (indexes, chosen) in [(Indexes::MISSING, true), (Indexes::OPEN, false)] {
            let due = offered_manifest(indexes, Language::Ru, chosen, &downloader).await;
            assert_eq!(due.expect("not due"), None);
        }
        assert_eq!(good.seen().len(), 1, "the release is asked only when due");
    }

    #[tokio::test]
    async fn first_run_and_recovery_prefer_the_newest_manifest_with_older_fallbacks() {
        use std::collections::HashMap;

        use crate::stub::{Route, Stub};

        let tag = release::tag();
        let newest_path = format!("/{tag}/{}", release::MANIFEST_3);
        let numbered_path = format!("/{tag}/{}", release::MANIFEST_2);
        let legacy_path = format!("/{tag}/{}", release::MANIFEST);
        let manifest = |name| Manifest {
            format: addict_dictionary::format::VERSION,
            indexes: vec![Published::of(name, b"xz")],
        };
        let newest = manifest("kaikki-ru-Russkiy");
        let numbered = manifest("wikdict-es-ca");
        let legacy = manifest("oewn");
        let dir = TempDir::new("manifest-selection");

        // How many of the manifests, newest first, the release has.
        for published in [3, 2, 1] {
            let mut routes = HashMap::from([(
                legacy_path.clone(),
                Route::File(legacy.to_json().into_bytes()),
            )]);
            if published >= 2 {
                routes.insert(
                    numbered_path.clone(),
                    Route::File(numbered.to_json().into_bytes()),
                );
            }
            if published == 3 {
                routes.insert(
                    newest_path.clone(),
                    Route::File(newest.to_json().into_bytes()),
                );
            }
            let (requests, expected): (Vec<_>, _) = match published {
                3 => (vec![&newest_path], &newest),
                2 => (vec![&newest_path, &numbered_path], &numbered),
                _ => (vec![&newest_path, &numbered_path, &legacy_path], &legacy),
            };
            let requests: Vec<String> = requests
                .into_iter()
                .map(|path| format!("GET {path} HTTP/1.1"))
                .collect();
            let stub = Stub::start(routes).unwrap();
            let downloader = Downloader::with_base_url(stub.base_url());
            let offer = offered_manifest(Indexes::MISSING, Language::Ru, false, &downloader)
                .await
                .unwrap();
            assert_eq!(offer.as_ref(), Some(expected));
            assert_eq!(stub.seen(), requests);

            let recovery = recovery_manifest(&downloader, dir.0.clone(), MANIFEST_LIMIT)
                .await
                .unwrap();
            assert_eq!(&recovery, expected);
            assert_eq!(stub.seen(), [requests.clone(), requests].concat());
        }
    }

    fn recovery_index(edition: &str) -> Vec<u8> {
        use addict_dictionary::format::{self, AnalysedForm, Contents, Headword, SenseRecord};
        format::encode(&Contents {
            source: edition.to_owned(),
            senses: vec![SenseRecord {
                definition: "a bank".to_owned(),
                members: vec!["bank".to_owned()],
                ..Default::default()
            }],
            headwords: vec![Headword {
                word: "bank".to_owned(),
                senses: vec![0],
                ..Default::default()
            }],
            analyses: vec![AnalysedForm {
                form: "banking".to_owned(),
                lemmas: vec![addict_dictionary::analysis::Analyses {
                    lemma: "bank".to_owned(),
                    shapes: vec![addict_dictionary::analysis::Shape::Gerund],
                }],
            }],
            ..Default::default()
        })
        .unwrap()
    }

    fn recovery_stub(indexes: Vec<Published>, asset: &[u8]) -> crate::stub::Stub {
        use crate::stub::{Route, Stub};
        let tag = release::tag();
        let manifest = Manifest {
            format: addict_dictionary::format::VERSION,
            indexes: indexes.clone(),
        };
        let mut routes = std::collections::HashMap::from([(
            format!("/{tag}/{}", release::MANIFEST_3),
            Route::File(manifest.to_json().into_bytes()),
        )]);
        for published in indexes {
            routes.insert(
                format!("/{tag}/{}", published.asset()),
                Route::File(asset.to_vec()),
            );
        }
        Stub::start(routes).unwrap()
    }

    fn recovery_xz(bytes: &[u8]) -> Vec<u8> {
        let mut xz = Vec::new();
        lzma_rs::xz_compress(&mut &bytes[..], &mut xz).unwrap();
        xz
    }

    #[tokio::test]
    async fn settings_restores_only_a_deleted_index_and_preserves_healthy_editions_and_choice() {
        let dir = TempDir::new("recovery-deleted");
        let data = dir.0.join("data");
        fs::create_dir(&data).unwrap();
        let manual = recovery_index("manual-edition");
        let published = recovery_index("published-edition");
        let xz = recovery_xz(&published);
        for name in ["oewn", "freedict", "wikdict-ru-es"] {
            fs::write(data.join(format!("{name}.index")), &manual).unwrap();
        }
        fs::remove_file(data.join("freedict.index")).unwrap();
        write(Some(&dir.file())).unwrap();
        let choice = fs::read(dir.file()).unwrap();
        let entries = ["oewn", "freedict", "wikdict-ru-es"]
            .into_iter()
            .map(|name| Published::of(name, &xz))
            .collect();
        let stub = recovery_stub(entries, &xz);
        let downloader = Downloader::with_base_url(stub.base_url());
        let manifest = recovery_manifest(&downloader, data.clone(), MANIFEST_LIMIT)
            .await
            .unwrap();
        assert_eq!(
            manifest
                .indexes
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["freedict"]
        );
        let mut queue = Queue::new(manifest.indexes);
        while let Some(published) = queue.next() {
            queue.finished(
                &published.name,
                downloader
                    .install(&published, &data, |_| {})
                    .await
                    .map(drop)
                    .map_err(|error| error.to_string()),
            );
        }
        assert_eq!(fs::read(data.join("oewn.index")).unwrap(), manual);
        assert_eq!(fs::read(data.join("wikdict-ru-es.index")).unwrap(), manual);
        assert_eq!(fs::read(data.join("freedict.index")).unwrap(), published);
        assert_eq!(fs::read(dir.file()).unwrap(), choice);
        let complete = recovery_manifest(&downloader, data, MANIFEST_LIMIT)
            .await
            .unwrap();
        assert!(complete.indexes.is_empty());
        assert_eq!(
            stub.seen()
                .iter()
                .filter(|line| line.contains(".index.xz"))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn settings_selects_each_unreadable_source_and_retains_unknown_names_in_order() {
        use addict_dictionary::format::{self, Header, SectionId};
        let dir = TempDir::new("recovery-unreadable");
        let good = recovery_index("manual");
        let xz = recovery_xz(&good);
        let mut names = Vec::new();
        for (name, section) in [
            ("oewn", SectionId::WordRecords),
            ("freedict", SectionId::Senses),
            ("wikdict-ru-es", SectionId::AnalysisRecords),
        ] {
            let mut bad = good.clone();
            let header = Header::parse(&good, good.len() as u64).unwrap();
            let offset = header.section(section).offset as usize
                + if section == SectionId::Senses { 4 } else { 0 };
            bad[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
            let path = dir.0.join(format!("{name}.index"));
            fs::write(&path, &bad).unwrap();
            let index = Index::open(&path).expect("header-valid corruption");
            let result = if section == SectionId::AnalysisRecords {
                index.analyses("banking").map(drop)
            } else {
                index.find("bank").map(drop)
            };
            assert!(result.is_err(), "a real lookup must expose the damage");
            names.push(name);
        }
        let mut unsupported = good.clone();
        unsupported[8..12].copy_from_slice(&(format::VERSION + 1).to_le_bytes());
        fs::write(dir.0.join("omw-es.index"), &unsupported).unwrap();
        fs::write(dir.0.join("sinonims-cat.index"), &good).unwrap();
        names.extend(["omw-es", "unknown", "sinonims-cat"]);
        let stub = recovery_stub(
            names
                .into_iter()
                .map(|name| Published::of(name, &xz))
                .collect(),
            &xz,
        );
        let downloader = Downloader::with_base_url(stub.base_url());
        let manifest = recovery_manifest(&downloader, dir.0.clone(), MANIFEST_LIMIT)
            .await
            .unwrap();
        assert_eq!(
            manifest
                .indexes
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["oewn", "freedict", "wikdict-ru-es", "omw-es", "unknown"]
        );
        // A failed replacement keeps the original damaged bytes.
        let original = fs::read(dir.0.join("oewn.index")).unwrap();
        let mut wrong = manifest.indexes[0].clone();
        wrong.size = 0;
        assert!(downloader.install(&wrong, &dir.0, |_| {}).await.is_err());
        assert_eq!(fs::read(dir.0.join("oewn.index")).unwrap(), original);

        let healthy = fs::read(dir.0.join("sinonims-cat.index")).unwrap();
        let recovering = Downloader::with_base_url(stub.base_url()).recovering();
        let mut queue = Queue::new(manifest.indexes);
        while let Some(published) = queue.next() {
            let result = recovering.install(&published, &dir.0, |_| {}).await;
            queue.finished(
                &published.name,
                result.map(drop).map_err(|error| error.to_string()),
            );
        }
        for name in ["oewn", "freedict", "wikdict-ru-es", "omw-es"] {
            assert_eq!(
                row_state(&queue, name),
                Some(&RowState::Installed),
                "{name}"
            );
            let path = dir.0.join(format!("{name}.index"));
            let restored = Index::open(&path).expect("repaired index opens");
            restored.validate().expect("all repaired records decode");
            assert!(restored.find("bank").unwrap().is_some(), "{name}");
            assert!(!restored.analyses("banking").unwrap().is_empty(), "{name}");
            assert_eq!(
                fs::read(path).unwrap(),
                good,
                "replacement installed the checked asset"
            );
        }
        let Some(RowState::Failed(error)) = row_state(&queue, "unknown") else {
            panic!("unknown source must fail on its own row");
        };
        assert!(
            error.contains("unknown") && error.contains("does not know"),
            "{error}"
        );
        assert_eq!(fs::read(dir.0.join("sinonims-cat.index")).unwrap(), healthy);
        assert!(!stub.seen().iter().any(
            |line| line.contains("sinonims-cat.index.xz") || line.contains("unknown.index.xz")
        ));
    }

    #[tokio::test]
    async fn settings_manifest_failures_leave_installed_files_unchanged_and_can_retry() {
        use crate::stub::{Route, Stub};
        let dir = TempDir::new("recovery-manifests");
        let good = recovery_index("manual");
        fs::write(dir.0.join("oewn.index"), &good).unwrap();
        for body in [
            None,
            Some(b"garbled".to_vec()),
            Some(
                Manifest {
                    format: addict_dictionary::format::VERSION,
                    indexes: Vec::new(),
                }
                .to_json()
                .into_bytes(),
            ),
        ] {
            // Without a manifest, all three are asked: 3, 2, then the legacy one.
            let requests_per_attempt = if body.is_some() { 1 } else { 3 };
            let routes = body
                .map(|body| {
                    (
                        format!("/{}/{}", release::tag(), release::MANIFEST_3),
                        Route::File(body),
                    )
                })
                .into_iter()
                .collect();
            let stub = Stub::start(routes).unwrap();
            let downloader = Downloader::with_base_url(stub.base_url());
            for _ in 0..2 {
                assert!(
                    recovery_manifest(&downloader, dir.0.clone(), MANIFEST_LIMIT)
                        .await
                        .is_err()
                );
                assert_eq!(fs::read(dir.0.join("oewn.index")).unwrap(), good);
            }
            assert_eq!(
                stub.seen().len(),
                2 * requests_per_attempt,
                "retry fetches afresh"
            );
        }
    }
    #[test]
    fn the_offer_checks_only_required_indexes_and_respects_decline() {
        use addict_dictionary::{
            format::{Contents, encode},
            pairs,
        };
        use addict_engine::dictionaries::{Articles, Dictionaries};

        let dir = TempDir::new("required-offer");
        let dictionaries = Dictionaries::new(Some(dir.0.clone()));
        let empty = dictionaries.indexes();
        for my in Language::ALL {
            assert!(should_offer(&empty, my, false));
            assert!(!should_offer(&empty, my, true));
        }

        let install = |name: &str| {
            let index = encode(&Contents {
                source: name.to_owned(),
                headwords: Vec::new(),
                senses: Vec::new(),
                forms: Vec::new(),
                analyses: Vec::new(),
            })
            .expect("encode");
            fs::write(dir.0.join(format!("{name}.index")), index).expect("install");
        };
        install("wikdict-ru-es");
        let indexes = dictionaries.indexes();
        assert!(should_offer(&indexes, Language::En, false));
        assert!(!should_offer(&indexes, Language::En, true));
        assert!(!should_offer(&indexes, Language::Ru, false));

        let oewn = dir.0.join("oewn.index");
        fs::write(&oewn, b"not an index").expect("write corrupt index");
        assert!(!should_offer(&dictionaries.indexes(), Language::En, false));
        install("oewn");
        assert!(!should_offer(&dictionaries.indexes(), Language::En, false));

        for name in pairs::required_index_names(Language::En) {
            install(name);
        }
        assert!(!dir.0.join("wikdict-ca-es.index").exists());
        assert!(!should_offer(&dictionaries.indexes(), Language::En, false));
    }

    #[test]
    fn a_declined_offer_is_read_back() {
        let dir = TempDir::new("round");
        let file = dir.file();
        assert!(!read(Some(&file)));
        write(Some(&file)).expect("written");
        assert_eq!(fs::read_to_string(&file).expect("the file"), "declined\n");
        assert!(read(Some(&file)));
    }

    #[test]
    fn a_missing_unreadable_or_other_file_is_no_choice() {
        let dir = TempDir::new("bad");
        let file = dir.file();
        assert!(!read(None));
        assert!(!read(Some(&file)));
        fs::create_dir_all(file.parent().expect("parent")).expect("dir");
        for content in ["", "accepted\n", "DECLINED", "declined later", "\u{0}"] {
            fs::write(&file, content).expect("write");
            assert!(!read(Some(&file)), "{content:?}");
        }
        fs::write(&file, "  declined \n").expect("write");
        assert!(read(Some(&file)));
        fs::remove_file(&file).expect("remove");
        fs::create_dir(&file).expect("a directory");
        assert!(!read(Some(&file)));
    }

    #[test]
    fn a_write_without_a_place_fails() {
        assert!(write(None).is_err());
        let dir = TempDir::new("blocked");
        fs::write(dir.0.join("addict"), "").expect("a file");
        assert!(write(Some(&dir.file())).is_err());
        assert!(!read(Some(&dir.file())));
    }

    /// An entry of `name` whose asset has `size` bytes.
    fn entry(name: &str, size: u64) -> Published {
        Published {
            name: name.to_owned(),
            format: addict_dictionary::format::VERSION,
            size,
            sha256: "0".repeat(64),
        }
    }

    /// The first run offers my language's set out of the 20 of
    /// `manifest-3.json`, in manifest order; out of the 19 of
    /// `manifest-2.json` or the legacy 13 it offers what they have of it,
    /// and an index already present is not offered.
    #[test]
    fn the_first_run_offers_the_set_of_my_language_only() {
        let all: Vec<Published> = sources::ROWS
            .iter()
            .filter(|row| release::MANIFEST_3_PUBLISHED.contains(&row.index))
            .enumerate()
            .map(|(i, row)| entry(row.index, i as u64 + 1))
            .collect();
        assert_eq!(all.len(), 20);
        let names = |offered: Vec<Published>| -> Vec<String> {
            offered
                .into_iter()
                .map(|published| published.name)
                .collect()
        };
        for my in Language::ALL {
            let required: Vec<String> =
                pairs::required_index_names(my).map(str::to_owned).collect();
            assert_eq!(names(offered(&all, my, |_| false)), required, "{my:?}");
            // The manifest's order, not the set's.
            let reversed: Vec<Published> = all.iter().rev().cloned().collect();
            let mut backwards = required.clone();
            backwards.reverse();
            assert_eq!(names(offered(&reversed, my, |_| false)), backwards);
        }
        assert_eq!(
            names(offered(&all, Language::Ru, |_| false)),
            release::LEGACY_PUBLISHED
        );
        let english = names(offered(&all, Language::En, |_| false));
        assert!(english.contains(&"wikdict-en-es".to_owned()));
        assert!(english.contains(&"kaikki-ru-Russkiy".to_owned()));
        assert!(!english.contains(&"wikdict-es-ca".to_owned()));
        assert!(!english.contains(&"wikdict-ru-es".to_owned()));
        assert!(
            !names(offered(&all, Language::Ru, |_| false))
                .contains(&"kaikki-ru-Russkiy".to_owned())
        );

        // Only manifest-2: English gets its set but the Russian article.
        let numbered: Vec<Published> = all
            .iter()
            .filter(|published| release::MANIFEST_2_PUBLISHED.contains(&published.name.as_str()))
            .cloned()
            .collect();
        assert_eq!(numbered.len(), 19);
        let english_numbered = names(offered(&numbered, Language::En, |_| false));
        let expected: Vec<String> = english
            .iter()
            .filter(|name| *name != "kaikki-ru-Russkiy")
            .cloned()
            .collect();
        assert_eq!(english_numbered, expected);

        // Only the legacy manifest: English gets what it lists of its set.
        let legacy: Vec<Published> = all
            .iter()
            .filter(|published| release::LEGACY_PUBLISHED.contains(&published.name.as_str()))
            .cloned()
            .collect();
        let english_legacy = names(offered(&legacy, Language::En, |_| false));
        assert!(!english_legacy.is_empty());
        assert!(english_legacy.iter().all(
            |name| english.contains(name) && release::LEGACY_PUBLISHED.contains(&name.as_str())
        ));
        assert!(!english_legacy.contains(&"wikdict-en-es".to_owned()));

        // A present index is not offered again.
        let indexes = Indexes {
            presence: Indexes::MISSING
                .presence
                .with(addict_dictionary::import::Source::WikdictEnEs),
            ..Indexes::MISSING
        };
        let without = names(offered(&all, Language::En, |name| {
            is_present(&indexes, name)
        }));
        assert_eq!(without.len(), english.len() - 1);
        assert!(!without.contains(&"wikdict-en-es".to_owned()));
        assert!(!is_present(&indexes, "unknown-index"));
    }

    #[test]
    fn a_change_of_my_language_misses_only_what_the_new_set_lacks() {
        let russian = pairs::required_sources(Language::Ru)
            .fold(Indexes::MISSING.presence, |presence, source| {
                presence.with(source)
            });
        let indexes = Indexes {
            presence: russian,
            ..Indexes::MISSING
        };
        assert!(missing(&indexes, Language::Ru).is_empty());
        assert_eq!(
            missing(&indexes, Language::En),
            [
                "kaikki-ru-Russkiy",
                "wikdict-en-es",
                "wikdict-en-ca",
                "wikdict-es-en",
                "wikdict-ca-en"
            ]
        );
        assert_eq!(
            missing(&Indexes::MISSING, Language::Es),
            pairs::required_index_names(Language::Es).collect::<Vec<_>>()
        );
        assert_eq!(
            nothing_text(Language::Ru),
            "All Russian dictionaries are readable. Nothing to download."
        );
    }
}
