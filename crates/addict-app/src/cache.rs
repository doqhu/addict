//! The on-disk cache of the online answers: one SQLite table in
//! `$XDG_CACHE_HOME/addict/translations.sqlite3`, or
//! `$HOME/.cache/addict/translations.sqlite3`.
//!
//! [`Cached`] wraps a [`Translator`] and answers a request it has answered
//! before, even in an earlier process, without asking the inner one. The key
//! of a request is a SHA-256 over every field the DeepL client sends that can
//! change the answer, as [`addict_deepl::sent`] names them, so the key cannot
//! drift from the wire. Only answers are kept; a refusal never is.
//!
//! The cache is opened lazily, by the first request or the first read of its
//! size, and always on the thread of the runner, never the UI thread. The
//! directory has mode 0700 and holds a `CACHEDIR.TAG`; the files have mode
//! 0600. Rows unused for 90 days go, and the oldest go while the rows hold
//! more than 10 MB; this is enforced at the first open of a process and then
//! at most once a day. Any error of the open, a read, a write or the schema
//! drops the connection and deletes the files: the request then goes to the
//! inner translator as it would without a cache, so the cache never blocks or
//! fails a translation. Nothing is encrypted: the files are as private as the
//! permissions and the disk make them, and deleted rows are overwritten
//! (`secure_delete`).

use std::{
    fmt, fs,
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::{SystemTime, UNIX_EPOCH},
};

use addict_core::translation::{Request, Translating, Translator};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params, types::Value};
use sha2::{Digest, Sha256};

/// The version of the schema and of the key. Bumping it drops the whole
/// cache: an old file is a schema error, and an error deletes the files.
const SCHEMA_VERSION: i64 = 1;

/// The name of the database file in the cache directory.
const FILE: &str = "translations.sqlite3";

/// The files SQLite keeps beside the database, by their suffixes.
const SIDE_FILES: [&str; 3] = ["-wal", "-shm", "-journal"];

/// The tag that keeps backups out of the directory, as
/// <https://bford.info/cachedir/> specifies it.
const TAG_FILE: &str = "CACHEDIR.TAG";
const TAG_SIGNATURE: &str = "Signature: 8a477f597d28d172789f06886806bc55";
const TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n\
                   # This file is a cache directory tag created by addict.\n\
                   # For information about cache directory tags, see:\n\
                   #\thttps://bford.info/cachedir/\n";

/// The most bytes the rows may hold after an eviction.
pub const SIZE_LIMIT: u64 = 10_000_000;

/// How long a row is kept after its last use, in seconds: 90 days.
pub const AGE_LIMIT: i64 = 90 * DAY;

/// How often the limits are enforced at most, in seconds, after the first
/// open of a process.
const DAY: i64 = 24 * 60 * 60;

const DIR_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;

/// Why the cache could not be used. It goes no further than this module,
/// except as a line of text.
#[derive(Debug)]
pub enum Error {
    /// The environment names no cache directory.
    NoDirectory,
    Io(io::Error),
    Sqlite(rusqlite::Error),
    /// The file holds another version of the schema, or a broken one.
    Schema,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NoDirectory => formatter.write_str("there is no cache directory"),
            Error::Io(error) => write!(formatter, "{error}"),
            Error::Sqlite(error) => write!(formatter, "{error}"),
            Error::Schema => formatter.write_str("the cache file has another schema"),
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Error::Io(error)
    }
}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Error::Sqlite(error)
    }
}

/// The cache directory of the product: `$XDG_CACHE_HOME/addict` when the
/// variable is an absolute path, else `$HOME/.cache/addict`; `None` when
/// neither names one. Nothing is read or created here.
pub fn cache_dir() -> Option<PathBuf> {
    cache_dir_from(std::env::var_os("XDG_CACHE_HOME"), std::env::var_os("HOME"))
}

fn cache_dir_from(
    cache_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let cache = match cache_home.map(PathBuf::from) {
        Some(cache) if cache.is_absolute() => cache,
        // The base directory specification says to ignore a relative value.
        _ => PathBuf::from(home.filter(|home| !home.is_empty())?).join(".cache"),
    };
    Some(cache.join("addict"))
}

/// The key of `request`: SHA-256 of the schema version, then `source_lang`,
/// `target_lang`, `model_type`, `formality`, `context` and `text`, with the
/// exact values the client sends. Each field is its length, eight bytes
/// little-endian, then its bytes, so no two requests share a serialization;
/// a missing context is one byte 0, a context one byte 1 then the field.
pub fn key_of(request: &Request) -> [u8; 32] {
    fn field(hash: &mut Sha256, bytes: &[u8]) {
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    let sent = addict_deepl::sent(request);
    let mut hash = Sha256::new();
    field(&mut hash, SCHEMA_VERSION.to_string().as_bytes());
    field(&mut hash, sent.source_lang.as_bytes());
    field(&mut hash, sent.target_lang.as_bytes());
    field(&mut hash, sent.model_type.as_bytes());
    field(&mut hash, sent.formality.as_bytes());
    match sent.context {
        None => hash.update([0]),
        Some(context) => {
            hash.update([1]);
            field(&mut hash, context.as_bytes());
        }
    }
    field(&mut hash, sent.text.as_bytes());
    hash.finalize().into()
}

/// What the cache holds: its rows and the bytes of its files, the database,
/// `-wal` and `-shm` together.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Footprint {
    pub rows: u64,
    pub bytes: u64,
}

/// One open cache file.
pub struct Cache {
    connection: Connection,
    path: PathBuf,
    /// When the limits were last enforced, in seconds since the epoch.
    evicted_at: i64,
}

impl Cache {
    /// Opens the cache in `dir`, creating the directory with mode 0700, its
    /// `CACHEDIR.TAG` and the file with mode 0600, and the tables when the
    /// file is new.
    pub fn open_at(dir: &Path) -> Result<Self, Error> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(DIR_MODE)
            .create(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(DIR_MODE))?;
        write_tag(dir)?;
        let path = dir.join(FILE);
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(FILE_MODE)
            .open(&path)?;
        // The umask may have taken bits away, and an older file may have
        // more: the mode is exactly 0600. SQLite gives its `-wal` and `-shm`
        // the mode of the database.
        file.set_permissions(fs::Permissions::from_mode(FILE_MODE))?;
        drop(file);
        let connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        // Before any table exists, or it has no effect.
        pragma(&connection, "PRAGMA auto_vacuum = INCREMENTAL")?;
        let mode = pragma(&connection, "PRAGMA journal_mode = WAL")?;
        if !matches!(&mode, Some(Value::Text(mode)) if mode.eq_ignore_ascii_case("wal")) {
            return Err(Error::Schema);
        }
        pragma(&connection, "PRAGMA synchronous = NORMAL")?;
        pragma(&connection, "PRAGMA secure_delete = ON")?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        match version {
            0 => {
                let tables: i64 =
                    connection
                        .query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get(0))?;
                if tables != 0 {
                    return Err(Error::Schema);
                }
                connection.execute_batch(&format!(
                    "BEGIN;
                     CREATE TABLE answers(
                         key BLOB PRIMARY KEY,
                         answer TEXT NOT NULL,
                         size INTEGER NOT NULL,
                         created_at INTEGER NOT NULL,
                         last_used INTEGER NOT NULL
                     ) WITHOUT ROWID;
                     CREATE INDEX answers_last_used ON answers(last_used);
                     CREATE TABLE meta(name TEXT PRIMARY KEY, value INTEGER NOT NULL);
                     PRAGMA user_version = {SCHEMA_VERSION};
                     COMMIT;"
                ))?;
            }
            SCHEMA_VERSION => {}
            _ => return Err(Error::Schema),
        }
        let vacuum: i64 = connection.query_row("PRAGMA auto_vacuum", [], |row| row.get(0))?;
        // 2 is INCREMENTAL.
        if vacuum != 2 {
            return Err(Error::Schema);
        }
        let evicted_at = connection
            .query_row(
                "SELECT value FROM meta WHERE name = 'evicted_at'",
                [],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(i64::MIN);
        Ok(Self {
            connection,
            path,
            evicted_at,
        })
    }

    /// The answer kept for `key`, which is marked as used at `now`.
    pub fn get(&mut self, key: &[u8; 32], now: i64) -> Result<Option<String>, Error> {
        let answer = self
            .connection
            .query_row(
                "UPDATE answers SET last_used = ?2 WHERE key = ?1 RETURNING answer",
                params![&key[..], now],
                |row| row.get(0),
            )
            .optional()?;
        Ok(answer)
    }

    /// Keeps `answer` for `key`, used at `now`.
    pub fn put(&mut self, key: &[u8; 32], answer: &str, now: i64) -> Result<(), Error> {
        let size = (key.len() + answer.len()) as i64;
        self.connection.execute(
            "INSERT INTO answers(key, answer, size, created_at, last_used)
             VALUES (?1, ?2, ?3, ?4, ?4)
             ON CONFLICT(key) DO UPDATE SET
                 answer = excluded.answer,
                 size = excluded.size,
                 last_used = excluded.last_used",
            params![&key[..], answer, size, now],
        )?;
        Ok(())
    }

    /// Deletes every row and rewrites the file without them.
    pub fn clear(&mut self) -> Result<(), Error> {
        self.connection.execute("DELETE FROM answers", [])?;
        self.connection.execute_batch("VACUUM")?;
        pragma(&self.connection, "PRAGMA wal_checkpoint(TRUNCATE)")?;
        Ok(())
    }

    /// How many rows the cache holds.
    pub fn rows(&self) -> Result<u64, Error> {
        let rows: i64 = self
            .connection
            .query_row("SELECT count(*) FROM answers", [], |row| row.get(0))?;
        Ok(rows.max(0) as u64)
    }

    /// The bytes of the database, its `-wal` and its `-shm`, as the file
    /// system has them.
    pub fn size(&self) -> u64 {
        files_of(&self.path)
            .iter()
            .take(3)
            .filter_map(|path| fs::metadata(path).ok())
            .map(|metadata| metadata.len())
            .sum()
    }

    /// Enforces the limits at `now`: rows unused for [`AGE_LIMIT`] go, then
    /// the least recently used go until the rows hold [`SIZE_LIMIT`] bytes
    /// at most.
    pub fn evict(&mut self, now: i64) -> Result<(), Error> {
        self.evict_within(now, SIZE_LIMIT, AGE_LIMIT)
    }

    /// [`Cache::evict`] when the limits were last enforced a day or more
    /// before `now`, or after it.
    pub fn evict_if_due(&mut self, now: i64) -> Result<(), Error> {
        if now < self.evicted_at || now.saturating_sub(self.evicted_at) >= DAY {
            self.evict(now)?;
        }
        Ok(())
    }

    fn evict_within(&mut self, now: i64, size_limit: u64, age_limit: i64) -> Result<(), Error> {
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "DELETE FROM answers WHERE last_used < ?1",
            params![now.saturating_sub(age_limit)],
        )?;
        // The newest rows are kept while their sizes, added up from the
        // newest, stay within the limit.
        transaction.execute(
            "DELETE FROM answers WHERE key IN (
                 SELECT key FROM (
                     SELECT key, sum(size) OVER (
                         ORDER BY last_used DESC, key
                         ROWS UNBOUNDED PRECEDING
                     ) AS total
                     FROM answers
                 ) WHERE total > ?1
             )",
            params![i64::try_from(size_limit).unwrap_or(i64::MAX)],
        )?;
        transaction.execute(
            "INSERT INTO meta(name, value) VALUES ('evicted_at', ?1)
             ON CONFLICT(name) DO UPDATE SET value = excluded.value",
            params![now],
        )?;
        transaction.commit()?;
        self.evicted_at = now;
        pragma(&self.connection, "PRAGMA incremental_vacuum")?;
        pragma(&self.connection, "PRAGMA wal_checkpoint(TRUNCATE)")?;
        Ok(())
    }
}

/// Runs the pragma `sql` and gives the first value of its first row, if it
/// gives one.
fn pragma(connection: &Connection, sql: &str) -> Result<Option<Value>, Error> {
    let mut statement = connection.prepare(sql)?;
    let mut rows = statement.query([])?;
    let value = match rows.next()? {
        Some(row) => Some(row.get::<_, Value>(0)?),
        None => None,
    };
    // Steps the rest, so that a pragma that works row by row ends.
    while rows.next()?.is_some() {}
    Ok(value)
}

/// Writes `CACHEDIR.TAG` in `dir` unless it already starts with the
/// signature.
fn write_tag(dir: &Path) -> Result<(), Error> {
    let path = dir.join(TAG_FILE);
    let mut start = [0_u8; TAG_SIGNATURE.len()];
    let tagged = fs::File::open(&path)
        .and_then(|mut file| file.read_exact(&mut start))
        .is_ok_and(|()| start == TAG_SIGNATURE.as_bytes());
    if tagged {
        return Ok(());
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(FILE_MODE)
        .open(&path)?;
    file.set_permissions(fs::Permissions::from_mode(FILE_MODE))?;
    file.write_all(TAG.as_bytes())?;
    Ok(())
}

/// The database at `path` and its side files.
fn files_of(path: &Path) -> Vec<PathBuf> {
    let mut files = vec![path.to_owned()];
    for suffix in SIDE_FILES {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        files.push(PathBuf::from(name));
    }
    files
}

/// Deletes the database in `dir` and its side files; a file that is not
/// there is no error. Call it only once no connection holds them.
fn remove_files(dir: &Path) -> io::Result<()> {
    let mut first = Ok(());
    for path in files_of(&dir.join(FILE)) {
        match fs::remove_file(&path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound && first.is_ok() => {
                first = Err(error);
            }
            _ => {}
        }
    }
    first
}

/// The seconds since the epoch.
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        })
}

/// The cache shared by the translator and the Settings window: the
/// directory and the connection, opened by the first use. Every call runs
/// SQLite inline, so it is made on the runner only.
pub struct Store {
    dir: Option<PathBuf>,
    open: Mutex<Option<Cache>>,
}

impl Store {
    /// A store in `dir`, `None` when there is none. Nothing is opened here.
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self {
            dir,
            open: Mutex::new(None),
        }
    }

    /// Runs `use_it` on the cache, opening it first, and enforcing the limits
    /// then, when it is not open. Any error drops the connection and deletes
    /// the files.
    fn with<R>(
        &self,
        use_it: impl FnOnce(&mut Cache, i64) -> Result<R, Error>,
    ) -> Result<R, Error> {
        let dir = self.dir.as_deref().ok_or(Error::NoDirectory)?;
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        let now = now();
        let used = (|| {
            if open.is_none() {
                let mut cache = Cache::open_at(dir)?;
                cache.evict(now)?;
                *open = Some(cache);
            }
            let cache = open.as_mut().ok_or(Error::Schema)?;
            use_it(cache, now)
        })();
        if used.is_err() {
            *open = None;
            let _ = remove_files(dir);
        }
        used
    }

    /// Whether the database file is there to be read, or open.
    fn exists(&self) -> bool {
        let open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        open.is_some()
            || self
                .dir
                .as_deref()
                .is_some_and(|dir| dir.join(FILE).exists())
    }

    /// The answer kept for `key`; `None` on a miss and on any error.
    pub fn get(&self, key: &[u8; 32]) -> Option<String> {
        self.with(|cache, now| cache.get(key, now)).ok().flatten()
    }

    /// Keeps `answer` for `key`, and enforces the limits when they are due.
    /// An error is not reported: the answer is simply not kept.
    pub fn put(&self, key: &[u8; 32], answer: &str) {
        let _ = self.with(|cache, now| {
            cache.put(key, answer, now)?;
            cache.evict_if_due(now)
        });
    }

    /// What the cache holds. A cache that was never written is not created
    /// for this, and one that gives an error is deleted and so is empty.
    pub fn footprint(&self) -> Footprint {
        if !self.exists() {
            return Footprint::default();
        }
        let Some(dir) = self.dir.as_deref() else {
            return Footprint::default();
        };
        let open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        if open.is_some() {
            drop(open);
            return self
                .with(|cache, _| {
                    Ok(Footprint {
                        rows: cache.rows()?,
                        bytes: cache.size(),
                    })
                })
                .unwrap_or_default();
        }
        // Not open yet: a connection of its own, dropped at once, with no
        // eviction, so the cache stays unopened until the first request.
        let read = Cache::open_at(dir).and_then(|cache| {
            let rows = cache.rows()?;
            let path = cache.path.clone();
            drop(cache);
            Ok(Footprint {
                rows,
                bytes: files_of(&path)
                    .iter()
                    .take(3)
                    .filter_map(|path| fs::metadata(path).ok())
                    .map(|metadata| metadata.len())
                    .sum(),
            })
        });
        read.unwrap_or_else(|_| {
            let _ = remove_files(dir);
            Footprint::default()
        })
    }

    /// Deletes every row and rewrites the file. When that fails, the files
    /// are deleted instead; the error is the one of that deletion.
    pub fn clear(&self) -> Result<(), String> {
        let Some(dir) = self.dir.as_deref() else {
            return Ok(());
        };
        if !self.exists() {
            return Ok(());
        }
        match self.with(|cache, _| cache.clear()) {
            Ok(()) => Ok(()),
            // The files are gone unless their deletion failed too.
            Err(_) => remove_files(dir).map_err(|error| error.to_string()),
        }
    }
}

/// A translator that answers from the cache what it answered before, and
/// asks `T` for the rest.
pub struct Cached<T> {
    inner: T,
    store: Arc<Store>,
}

impl<T: Translator> Cached<T> {
    /// `inner` with a cache in `dir`; nothing is opened here.
    pub fn new(inner: T, dir: Option<PathBuf>) -> Self {
        Self {
            inner,
            store: Arc::new(Store::new(dir)),
        }
    }

    /// The cache, for its size and its clear.
    pub fn store(&self) -> Arc<Store> {
        self.store.clone()
    }

    #[cfg(test)]
    pub fn inner(&self) -> &T {
        &self.inner
    }
}

impl<T: Translator> Translator for Cached<T> {
    fn translate(&self, request: Request) -> Translating<'_> {
        Box::pin(async move {
            let key = key_of(&request);
            // The lock is released before the inner request is awaited.
            if let Some(answer) = self.store.get(&key) {
                return Ok(answer);
            }
            let answer = self.inner.translate(request).await;
            if let Ok(text) = &answer {
                self.store.put(&key, text);
            }
            answer
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        task::{Context, Poll, Waker},
    };

    use addict_core::translation::{Canned, Refusal, Request, Source, Translating, Translator};

    use super::{
        AGE_LIMIT, Cache, Cached, DAY, FILE, Footprint, SIZE_LIMIT, Store, TAG_FILE, TAG_SIGNATURE,
        cache_dir_from, key_of,
    };

    /// A directory of one test, removed when the test ends.
    pub(crate) struct TempDir(pub(crate) PathBuf);

    impl TempDir {
        pub(crate) fn new(tag: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("addict-cache-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }

        /// The cache directory inside it.
        pub(crate) fn cache(&self) -> PathBuf {
            self.0.join("addict")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = make_writable(&self.0);
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn make_writable(path: &Path) -> std::io::Result<()> {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
    }

    /// The answer of a future that is ready at its first poll.
    fn ready(mut translating: Translating<'_>) -> Result<String, Refusal> {
        let mut context = Context::from_waker(Waker::noop());
        match translating.as_mut().poll(&mut context) {
            Poll::Ready(answer) => answer,
            Poll::Pending => panic!("the answer is not ready"),
        }
    }

    fn bank(context: &str) -> Request {
        Request {
            text: "bank".to_owned(),
            context: Some(context.to_owned()),
            source: Source::En,
        }
    }

    const RIVER: &str = "I sat on the bank of the river.";
    const MONEY: &str = "The bank closed at three.";

    fn cached(dir: &Path, answer: Result<&str, Refusal>) -> Cached<Canned> {
        Cached::new(Canned::new(answer), Some(dir.to_owned()))
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777
    }

    #[test]
    fn a_miss_asks_once_shows_the_answer_and_keeps_it() {
        let temp = TempDir::new("miss");
        let translator = cached(&temp.cache(), Ok("берег"));
        assert_eq!(
            ready(translator.translate(bank(RIVER))),
            Ok("берег".to_owned())
        );
        assert_eq!(translator.inner().asked(), [bank(RIVER)]);
        assert_eq!(translator.store().footprint().rows, 1);
        // The same request again is a hit.
        assert_eq!(
            ready(translator.translate(bank(RIVER))),
            Ok("берег".to_owned())
        );
        assert_eq!(translator.inner().asked().len(), 1);
    }

    #[test]
    fn a_hit_after_a_restart_asks_nothing() {
        let temp = TempDir::new("restart");
        let first = cached(&temp.cache(), Ok("берег"));
        assert_eq!(ready(first.translate(bank(RIVER))), Ok("берег".to_owned()));
        drop(first);
        let second = cached(&temp.cache(), Ok("другое"));
        assert_eq!(ready(second.translate(bank(RIVER))), Ok("берег".to_owned()));
        assert!(second.inner().asked().is_empty());
    }

    #[test]
    fn every_language_and_kind_of_request_is_kept_apart() {
        let temp = TempDir::new("kinds");
        let translator = cached(&temp.cache(), Ok("ответ"));
        let requests = [
            bank(RIVER),
            bank(MONEY),
            Request {
                text: "bank".to_owned(),
                context: None,
                source: Source::En,
            },
            Request {
                text: "bank".to_owned(),
                context: Some(String::new()),
                source: Source::En,
            },
            Request {
                text: RIVER.to_owned(),
                context: None,
                source: Source::En,
            },
            Request {
                text: "banco".to_owned(),
                context: Some("El banco cerró.".to_owned()),
                source: Source::Es,
            },
            Request {
                text: "banco".to_owned(),
                context: Some("El banco cerró.".to_owned()),
                source: Source::Ca,
            },
        ];
        for request in &requests {
            assert_eq!(
                ready(translator.translate(request.clone())),
                Ok("ответ".to_owned())
            );
        }
        assert_eq!(translator.inner().asked(), requests);
        let keys: std::collections::HashSet<_> = requests.iter().map(key_of).collect();
        assert_eq!(keys.len(), requests.len());
        assert_eq!(translator.store().footprint().rows, requests.len() as u64);
    }

    #[test]
    fn the_same_word_in_another_sentence_is_a_miss() {
        let temp = TempDir::new("context");
        let translator = cached(&temp.cache(), Ok("банк"));
        let _ = ready(translator.translate(bank(RIVER)));
        let _ = ready(translator.translate(bank(MONEY)));
        assert_eq!(translator.inner().asked(), [bank(RIVER), bank(MONEY)]);
    }

    #[test]
    fn a_refusal_is_shown_and_never_kept() {
        let temp = TempDir::new("refusal");
        let translator = cached(&temp.cache(), Err(Refusal::Quota));
        assert_eq!(
            ready(translator.translate(bank(RIVER))),
            Err(Refusal::Quota)
        );
        assert_eq!(translator.store().footprint().rows, 0);
        assert_eq!(
            ready(translator.translate(bank(RIVER))),
            Err(Refusal::Quota)
        );
        assert_eq!(translator.inner().asked().len(), 2);
    }

    #[test]
    fn a_corrupt_file_is_deleted_and_the_next_run_caches_again() {
        let temp = TempDir::new("corrupt");
        let dir = temp.cache();
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join(FILE), vec![0x5a_u8; 8192]).expect("garbage");
        let translator = cached(&dir, Ok("берег"));
        assert_eq!(
            ready(translator.translate(bank(RIVER))),
            Ok("берег".to_owned())
        );
        assert_eq!(translator.inner().asked().len(), 1);
        drop(translator);
        let next = cached(&dir, Ok("берег"));
        let _ = ready(next.translate(bank(RIVER)));
        drop(next);
        let after = cached(&dir, Ok("другое"));
        assert_eq!(ready(after.translate(bank(RIVER))), Ok("берег".to_owned()));
        assert!(after.inner().asked().is_empty());
    }

    #[test]
    fn a_file_of_another_schema_is_deleted() {
        let temp = TempDir::new("schema");
        let dir = temp.cache();
        drop(Cache::open_at(&dir).expect("open"));
        {
            let connection = rusqlite::Connection::open(dir.join(FILE)).expect("open");
            connection
                .execute_batch("PRAGMA user_version = 99")
                .expect("version");
        }
        assert!(matches!(Cache::open_at(&dir), Err(super::Error::Schema)));
        let translator = cached(&dir, Ok("берег"));
        assert_eq!(
            ready(translator.translate(bank(RIVER))),
            Ok("берег".to_owned())
        );
        assert_eq!(translator.inner().asked().len(), 1);
        drop(translator);
        assert!(Cache::open_at(&dir).is_ok());
    }

    #[test]
    fn without_a_cache_directory_the_translation_works_uncached() {
        let translator = Cached::new(Canned::new(Ok("берег")), None);
        assert_eq!(
            ready(translator.translate(bank(RIVER))),
            Ok("берег".to_owned())
        );
        assert_eq!(
            ready(translator.translate(bank(RIVER))),
            Ok("берег".to_owned())
        );
        assert_eq!(translator.inner().asked().len(), 2);
        assert_eq!(translator.store().footprint(), Footprint::default());
        assert_eq!(translator.store().clear(), Ok(()));
    }

    #[test]
    fn an_unwritable_directory_leaves_the_translation_uncached() {
        let temp = TempDir::new("unwritable");
        let parent = temp.0.join("locked");
        std::fs::create_dir(&parent).expect("dir");
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o500)).expect("mode");
        let translator = cached(&parent.join("addict"), Ok("берег"));
        assert_eq!(
            ready(translator.translate(bank(RIVER))),
            Ok("берег".to_owned())
        );
        assert_eq!(
            ready(translator.translate(bank(RIVER))),
            Ok("берег".to_owned())
        );
        assert_eq!(translator.inner().asked().len(), 2);
        make_writable(&parent).expect("mode");
    }

    #[test]
    fn stale_rows_go_and_the_oldest_go_beyond_the_size_limit() {
        let temp = TempDir::new("evict");
        let mut cache = Cache::open_at(&temp.cache()).expect("open");
        let now = 1_000 * DAY;
        let key = |n: u8| [n; 32];
        let answer = "x".repeat(68);
        // Each row is 32 + 68 = 100 bytes.
        cache.put(&key(1), &answer, now - 100 * DAY).expect("put");
        for n in 2..=6 {
            cache
                .put(&key(n), &answer, now - i64::from(10 - n))
                .expect("put");
        }
        cache.evict_within(now, 300, AGE_LIMIT).expect("evict");
        assert_eq!(cache.get(&key(1), now).expect("get"), None, "stale");
        for n in 2..=3 {
            assert_eq!(cache.get(&key(n), now).expect("get"), None, "oldest {n}");
        }
        for n in 4..=6 {
            assert!(
                cache.get(&key(n), now).expect("get").is_some(),
                "newest {n}"
            );
        }
        assert_eq!(cache.rows().expect("rows"), 3);
        assert_eq!(SIZE_LIMIT, 10_000_000);
        assert_eq!(AGE_LIMIT, 90 * 24 * 60 * 60);
    }

    #[test]
    fn a_hit_counts_as_a_use_for_the_age_limit() {
        let temp = TempDir::new("used");
        let mut cache = Cache::open_at(&temp.cache()).expect("open");
        let now = 1_000 * DAY;
        cache.put(&[1; 32], "старое", now - 89 * DAY).expect("put");
        assert!(cache.get(&[1; 32], now - DAY).expect("get").is_some());
        cache.evict(now + 60 * DAY).expect("evict");
        assert!(cache.get(&[1; 32], now).expect("get").is_some());
        cache.evict(now + 100 * DAY).expect("evict");
        assert_eq!(cache.rows().expect("rows"), 0);
    }

    #[test]
    fn the_limits_are_enforced_at_most_once_a_day_after_the_open() {
        let temp = TempDir::new("daily");
        let mut cache = Cache::open_at(&temp.cache()).expect("open");
        let now = 1_000 * DAY;
        cache.evict(now).expect("evict");
        cache.put(&[1; 32], "старое", now - 200 * DAY).expect("put");
        cache.evict_if_due(now + DAY - 1).expect("evict");
        assert_eq!(cache.rows().expect("rows"), 1, "not due");
        cache.evict_if_due(now + DAY).expect("evict");
        assert_eq!(cache.rows().expect("rows"), 0, "due");
        drop(cache);
        // The time of the last eviction outlives the process.
        let mut reopened = Cache::open_at(&temp.cache()).expect("open");
        reopened
            .put(&[2; 32], "старое", now - 200 * DAY)
            .expect("put");
        reopened.evict_if_due(now + DAY + 1).expect("evict");
        assert_eq!(reopened.rows().expect("rows"), 1);
    }

    #[test]
    fn a_clear_empties_the_cache_and_the_next_request_asks_again() {
        let temp = TempDir::new("clear");
        let translator = cached(&temp.cache(), Ok("берег"));
        let _ = ready(translator.translate(bank(RIVER)));
        let _ = ready(translator.translate(bank(MONEY)));
        let store = translator.store();
        assert_eq!(store.footprint().rows, 2);
        assert_eq!(store.clear(), Ok(()));
        assert_eq!(store.footprint().rows, 0);
        assert_eq!(
            crate::settings::cache_text(crate::settings::CacheState::Read(store.footprint())),
            "The cache is empty."
        );
        let _ = ready(translator.translate(bank(RIVER)));
        assert_eq!(translator.inner().asked().len(), 3);
    }

    #[test]
    fn a_cleared_file_no_longer_holds_the_text() {
        let temp = TempDir::new("scrub");
        let translator = cached(&temp.cache(), Ok("берег"));
        let secret = "A sentence nobody else may read, unique 7f3a.";
        let _ = ready(translator.translate(bank(secret)));
        let answer = Request {
            text: secret.to_owned(),
            context: None,
            source: Source::En,
        };
        let _ = ready(translator.translate(answer));
        let store = translator.store();
        // The context is not stored, only the answer: it is the answer that
        // must go.
        assert_eq!(store.clear(), Ok(()));
        for path in super::files_of(&temp.cache().join(FILE)) {
            if let Ok(bytes) = std::fs::read(&path) {
                assert!(
                    !bytes
                        .windows("берег".len())
                        .any(|w| w == "берег".as_bytes()),
                    "{}",
                    path.display()
                );
            }
        }
    }

    #[test]
    fn the_size_of_an_existing_file_leaves_the_store_unopened() {
        let temp = TempDir::new("unopened");
        let first = cached(&temp.cache(), Ok("берег"));
        let _ = ready(first.translate(bank(RIVER)));
        drop(first);
        let store = Store::new(Some(temp.cache()));
        let footprint = store.footprint();
        assert_eq!(footprint.rows, 1);
        assert!(footprint.bytes > 0);
        assert!(store.open.lock().expect("lock").is_none());
    }

    #[test]
    fn the_size_of_a_corrupt_file_deletes_it_and_is_empty() {
        let temp = TempDir::new("corrupt-size");
        let dir = temp.cache();
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join(FILE), vec![0x5a_u8; 8192]).expect("garbage");
        let store = Store::new(Some(dir.clone()));
        assert_eq!(store.footprint(), Footprint::default());
        assert!(!dir.join(FILE).exists());
        assert!(store.open.lock().expect("lock").is_none());
    }

    #[test]
    fn a_store_that_was_never_used_creates_nothing_for_its_size() {
        let temp = TempDir::new("untouched");
        let store = Store::new(Some(temp.cache()));
        assert_eq!(store.footprint(), Footprint::default());
        assert_eq!(store.clear(), Ok(()));
        assert!(!temp.cache().exists());
    }

    #[test]
    fn the_size_counts_the_database_and_its_side_files() {
        let temp = TempDir::new("size");
        let translator = cached(&temp.cache(), Ok("берег"));
        let _ = ready(translator.translate(bank(RIVER)));
        let footprint = translator.store().footprint();
        let on_disk: u64 = ["", "-wal", "-shm"]
            .iter()
            .filter_map(|suffix| {
                std::fs::metadata(temp.cache().join(format!("{FILE}{suffix}"))).ok()
            })
            .map(|metadata| metadata.len())
            .sum();
        assert_eq!(footprint.rows, 1);
        assert_eq!(footprint.bytes, on_disk);
        assert!(footprint.bytes > 0);
    }

    #[test]
    fn the_directory_and_every_file_are_private_and_tagged() {
        let temp = TempDir::new("modes");
        let dir = temp.cache();
        let translator = cached(&dir, Ok("берег"));
        let _ = ready(translator.translate(bank(RIVER)));
        // The connection is still open, so the side files of WAL are there.
        assert_eq!(mode(&dir), 0o700);
        for name in [
            FILE,
            "translations.sqlite3-wal",
            "translations.sqlite3-shm",
            TAG_FILE,
        ] {
            let path = dir.join(name);
            assert!(path.exists(), "{name}");
            assert_eq!(mode(&path), 0o600, "{name}");
        }
        let tag = std::fs::read_to_string(dir.join(TAG_FILE)).expect("tag");
        assert!(tag.starts_with(TAG_SIGNATURE));
        assert_eq!(TAG_SIGNATURE.len(), 43);
    }

    #[test]
    fn an_older_open_directory_is_made_private_again() {
        let temp = TempDir::new("reopen");
        let dir = temp.cache();
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).expect("mode");
        std::fs::write(dir.join(FILE), b"").expect("file");
        std::fs::set_permissions(dir.join(FILE), std::fs::Permissions::from_mode(0o644))
            .expect("mode");
        drop(Cache::open_at(&dir).expect("open"));
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join(FILE)), 0o600);
    }

    #[test]
    fn the_cache_directory_follows_the_base_directory_specification() {
        let some = |s: &str| Some(std::ffi::OsString::from(s));
        assert_eq!(
            cache_dir_from(some("/x/cache"), some("/home/u")),
            Some(PathBuf::from("/x/cache/addict"))
        );
        assert_eq!(
            cache_dir_from(some("relative"), some("/home/u")),
            Some(PathBuf::from("/home/u/.cache/addict"))
        );
        assert_eq!(
            cache_dir_from(None, some("/home/u")),
            Some(PathBuf::from("/home/u/.cache/addict"))
        );
        assert_eq!(cache_dir_from(None, some("")), None);
        assert_eq!(cache_dir_from(None, None), None);
    }
}
