use std::any::TypeId;
use std::collections::hash_map::{Entry, HashMap};
use std::ffi::CString;
#[cfg(windows)]
use std::ffi::OsStr;
use std::fs::File;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use std::{io, ptr, sync};

use once_cell::sync::Lazy;
use synchronoise::event::SignalEvent;

use crate::cursor::RoCursor;
use crate::flags::Flags;
use crate::mdb::error::mdb_result;
use crate::mdb::ffi;
use crate::{Database, Error, PolyDatabase, Result, RoTxn, RwTxn};

/// The list of opened environments, the value is an optional environment, it is None
/// when someone asks to close the environment, closing is a two-phase step, to make sure
/// noone tries to open the same environment between these two phases.
///
/// Trying to open a None marked environment returns an error to the user trying to open it.
static OPENED_ENV: Lazy<RwLock<HashMap<PathBuf, EnvEntry>>> = Lazy::new(RwLock::default);

struct EnvEntry {
    env: Option<Env>,
    signal_event: Arc<SignalEvent>,
    options: EnvOpenOptions,
}

// Thanks to the mozilla/rkv project
// Workaround the UNC path on Windows, see https://github.com/rust-lang/rust/issues/42869.
// Otherwise, `Env::from_env()` will panic with error_no(123).
#[cfg(not(windows))]
fn canonicalize_path(path: &Path) -> io::Result<PathBuf> {
    path.canonicalize()
}

#[cfg(windows)]
fn canonicalize_path(path: &Path) -> io::Result<PathBuf> {
    let canonical = path.canonicalize()?;
    let url = url::Url::from_file_path(&canonical)
        .map_err(|_e| io::Error::new(io::ErrorKind::Other, "URL passing error"))?;
    url.to_file_path()
        .map_err(|_e| io::Error::new(io::ErrorKind::Other, "path canonicalization error"))
}

#[cfg(windows)]
/// Adding a 'missing' trait from windows OsStrExt
trait OsStrExtLmdb {
    fn as_bytes(&self) -> &[u8];
}
#[cfg(windows)]
impl OsStrExtLmdb for OsStr {
    fn as_bytes(&self) -> &[u8] {
        &self.to_str().unwrap().as_bytes()
    }
}

#[cfg(windows)]
fn get_file_fd(file: &File) -> std::os::windows::io::RawHandle {
    use std::os::windows::io::AsRawHandle;
    file.as_raw_handle()
}

#[cfg(unix)]
fn get_file_fd(file: &File) -> std::os::unix::io::RawFd {
    use std::os::unix::io::AsRawFd;
    file.as_raw_fd()
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct Geometry {
    #[cfg(feature = "mdbx")]
    min_size: Option<usize>,
    #[cfg(feature = "mdbx")]
    max_size: Option<usize>,
    #[cfg(feature = "mdbx")]
    growth_step: Option<usize>,
    #[cfg(feature = "mdbx")]
    shrink_step: Option<usize>,

    map_size: Option<usize>,
    page_size: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnvOpenOptions {
    geometry: Geometry,

    max_readers: Option<u32>,
    max_dbs: Option<u32>,

    flags: u32, // LMDB flags
}

impl EnvOpenOptions {
    pub fn new() -> EnvOpenOptions {
        EnvOpenOptions { geometry: Geometry::default(), max_readers: None, max_dbs: None, flags: 0 }
    }

    pub fn map_size(&mut self, size: usize) -> &mut Self {
        self.geometry.map_size = Some(size);
        self
    }
    pub fn page_size(&mut self, size: usize) -> &mut Self {
        self.geometry.page_size = Some(size);
        self
    }

    #[cfg(feature = "mdbx")]
    pub fn min_size(&mut self, size: usize) -> &mut Self {
        self.geometry.min_size = Some(size);
        self
    }

    #[cfg(feature = "mdbx")]
    pub fn max_size(&mut self, size: usize) -> &mut Self {
        self.geometry.max_size = Some(size);
        self
    }

    #[cfg(feature = "mdbx")]
    pub fn growth_step(&mut self, size: usize) -> &mut Self {
        self.geometry.growth_step = Some(size);
        self
    }

    #[cfg(feature = "mdbx")]
    pub fn shrink_threshold(&mut self, size: usize) -> &mut Self {
        self.geometry.shrink_step = Some(size);
        self
    }

    pub fn max_readers(&mut self, readers: u32) -> &mut Self {
        self.max_readers = Some(readers);
        self
    }

    pub fn max_dbs(&mut self, dbs: u32) -> &mut Self {
        self.max_dbs = Some(dbs);
        self
    }

    /// Set one or more LMDB flags (see http://www.lmdb.tech/doc/group__mdb__env.html).
    /// ```
    /// use std::fs;
    /// use std::path::Path;
    /// use heed::{EnvOpenOptions, Database};
    /// use heed::types::*;
    /// use heed::flags::Flags;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// fs::create_dir_all(Path::new("target").join("zerocopy.mdb"))?;
    /// let mut env_builder = EnvOpenOptions::new();
    /// unsafe {
    ///     env_builder.flag(Flags::MdbNoTls);
    ///     env_builder.flag(Flags::MdbNoMetaSync);
    /// }
    /// let env = env_builder.open(Path::new("target").join("zerocopy.mdb"))?;
    ///
    /// // we will open the default unamed database
    /// let mut wtxn = env.write_txn()?;
    /// let db: Database<Str, OwnedType<i32>> = env.create_database(&mut wtxn, None)?;
    ///
    /// // opening a write transaction
    /// db.put(&mut wtxn, "seven", &7)?;
    /// db.put(&mut wtxn, "zero", &0)?;
    /// db.put(&mut wtxn, "five", &5)?;
    /// db.put(&mut wtxn, "three", &3)?;
    /// wtxn.commit()?;
    ///
    /// // Force the OS to flush the buffers (see Flags::MdbNoSync and Flags::MdbNoMetaSync).
    /// env.force_sync();
    ///
    /// // opening a read transaction
    /// // to check if those values are now available
    /// let mut rtxn = env.read_txn()?;
    ///
    /// let ret = db.get(&rtxn, "zero")?;
    /// assert_eq!(ret, Some(0));
    ///
    /// let ret = db.get(&rtxn, "five")?;
    /// assert_eq!(ret, Some(5));
    /// # Ok(()) }
    /// ```
    pub unsafe fn flag(&mut self, flag: Flags) -> &mut Self {
        self.flags = self.flags | flag as u32;
        self
    }

    pub fn open<P: AsRef<Path>>(&self, path: P) -> Result<Env> {
        let path = canonicalize_path(path.as_ref())?;

        let mut lock = OPENED_ENV.write().unwrap();

        match lock.entry(path) {
            Entry::Occupied(entry) => {
                if &entry.get().options != self {
                    return Err(Error::BadOpenOptions);
                }
                entry.get().env.clone().ok_or(Error::DatabaseClosing)
            }
            Entry::Vacant(entry) => {
                let path = entry.key();
                let path_str = CString::new(path.as_os_str().as_bytes()).unwrap();

                unsafe {
                    let mut env: *mut ffi::MDB_env = ptr::null_mut();
                    mdb_result(ffi::mdb_env_create(&mut env))?;

                    // if let Some(size) = self.geometry.page_size {
                    //     if size % page_size::get() != 0 {
                    //         let msg = format!(
                    //             "map size ({}) must be a multiple of the system page size ({})",
                    //             size,
                    //             page_size::get()
                    //         );
                    //         return Err(Error::Io(io::Error::new(
                    //             io::ErrorKind::InvalidInput,
                    //             msg,
                    //         )));
                    //     }
                    // }
                    #[cfg(all(feature = "lmdb", not(feature = "mdbx")))]
                    mdb_result(ffi::mdb_env_set_mapsize(
                        env,
                        self.geometry.page_size.unwrap_or(page_size::get()),
                    ))?;
                    #[cfg(all(not(feature = "lmdb"), feature = "mdbx"))]
                    {
                        mdb_result(ffi::mdb_env_set_geometry(
                            env,
                            self.geometry.min_size.map(|v| v as isize).unwrap_or(-1),
                            self.geometry.map_size.map(|v| v as isize).unwrap_or(-1),
                            self.geometry.max_size.map(|v| v as isize).unwrap_or(-1),
                            self.geometry.growth_step.map(|v| v as isize).unwrap_or(-1),
                            self.geometry.shrink_step.map(|v| v as isize).unwrap_or(-1),
                            self.geometry.page_size.unwrap_or(page_size::get()) as isize,
                        ))?
                    }

                    if let Some(readers) = self.max_readers {
                        mdb_result(ffi::mdb_env_set_maxreaders(env, readers))?;
                    }

                    if let Some(dbs) = self.max_dbs {
                        mdb_result(ffi::mdb_env_set_maxdbs(env, dbs))?;
                    }

                    // When the `read-txn-no-tls` feature is enabled, we must force LMDB
                    // to avoid using the thread local storage, this way we allow users
                    // to use references of RoTxn between threads safely.
                    let flags = if cfg!(feature = "read-txn-no-tls") {
                        self.flags | Flags::MdbNoTls as u32
                    } else {
                        self.flags
                    };

                    let result =
                        mdb_result(ffi::mdb_env_open(env, path_str.as_ptr(), flags, 0o600));

                    match result {
                        Ok(()) => {
                            let signal_event = Arc::new(SignalEvent::manual(false));
                            let inner = EnvInner {
                                env,
                                dbi_open_mutex: sync::Mutex::default(),
                                path: path.clone(),
                            };
                            let env = Env(Arc::new(inner));
                            let cache_entry = EnvEntry {
                                env: Some(env.clone()),
                                options: self.clone(),
                                signal_event,
                            };
                            entry.insert(cache_entry);
                            Ok(env)
                        }
                        Err(e) => {
                            ffi::mdb_env_close(env);
                            Err(e.into())
                        }
                    }
                }
            }
        }
    }
}

/// Returns a struct that allows to wait for the effective closing of an environment.
pub fn env_closing_event<P: AsRef<Path>>(path: P) -> Option<EnvClosingEvent> {
    let lock = OPENED_ENV.read().unwrap();
    lock.get(path.as_ref()).map(|e| EnvClosingEvent(e.signal_event.clone()))
}

#[derive(Clone)]
pub struct Env(Arc<EnvInner>);

struct EnvInner {
    env: *mut ffi::MDB_env,
    dbi_open_mutex: sync::Mutex<HashMap<u32, Option<(TypeId, TypeId)>>>,
    path: PathBuf,
}

unsafe impl Send for EnvInner {}

unsafe impl Sync for EnvInner {}

impl Drop for EnvInner {
    fn drop(&mut self) {
        let mut lock = OPENED_ENV.write().unwrap();

        match lock.remove(&self.path) {
            None => panic!("It seems another env closed this env before"),
            Some(EnvEntry { signal_event, .. }) => {
                unsafe {
                    let _ = ffi::mdb_env_close(self.env);
                }
                // We signal to all the waiters that we have closed the env.
                signal_event.signal();
            }
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub enum CompactionOption {
    Enabled,
    Disabled,
}

impl Env {
    /// The real size used by this environment on disk.
    pub fn real_disk_size(&self) -> Result<u64> {
        let path = if self.contains_flag(Flags::MdbNoSubDir)? {
            self.path().to_path_buf()
        } else {
            self.path().join("data.mdb")
        };

        Ok(path.metadata()?.len())
    }

    /// Check if a flag was specified when opening the environment.
    pub fn contains_flag(&self, flag: Flags) -> Result<bool> {
        let flags = self.raw_flags()?;
        let or = flags & (flag as u32);

        Ok(or != 0)
    }

    /// Return the raw flags the environment was opened with.
    pub fn raw_flags(&self) -> Result<u32> {
        let mut flags = std::mem::MaybeUninit::uninit();
        unsafe { mdb_result(ffi::mdb_env_get_flags(self.env_mut_ptr(), flags.as_mut_ptr()))? };
        let flags = unsafe { flags.assume_init() };

        Ok(flags)
    }

    /// The map size that was set when configuring the environment.
    pub fn map_size(&self) -> Result<usize> {
        ffi::map_size(self.env_mut_ptr())
    }

    /// Returns the size used by all the databases in the environment without the free pages.
    pub fn non_free_pages_size(&self) -> Result<u64> {
        let compute_size = |stat: ffi::MDB_stat| {
            (stat.ms_leaf_pages + stat.ms_branch_pages + stat.ms_overflow_pages) as u64
                * stat.ms_psize as u64
        };

        let mut size = 0;

        let mut stat = std::mem::MaybeUninit::uninit();
        unsafe { mdb_result(ffi::mdb_env_stat(self.env_mut_ptr(), stat.as_mut_ptr()))? };
        let stat = unsafe { stat.assume_init() };
        size += compute_size(stat);

        let rtxn = self.read_txn()?;
        let dbi = self.raw_open_dbi(rtxn.txn, None, 0)?;

        // we don’t want anyone to open an environment while we’re computing the stats
        // thus we take a lock on the dbi
        let dbi_open = self.0.dbi_open_mutex.lock().unwrap();

        // We’re going to iterate on the unnamed database
        let mut cursor = RoCursor::new(&rtxn, dbi)?;

        while let Some((key, _value)) = cursor.move_on_next()? {
            if key.contains(&0) {
                continue;
            }

            let key = String::from_utf8(key.to_vec()).unwrap();
            if let Ok(dbi) = self.raw_open_dbi(rtxn.txn, Some(&key), 0) {
                let mut stat = std::mem::MaybeUninit::uninit();
                unsafe { mdb_result(ffi::mdb_stat(rtxn.txn, dbi, stat.as_mut_ptr()))? };
                let stat = unsafe { stat.assume_init() };

                size += compute_size(stat);

                // if the db wasn’t already opened
                if !dbi_open.contains_key(&dbi) {
                    unsafe {
                        ffi::mdb_dbi_close(self.env_mut_ptr(), dbi);
                    }
                }
            }
        }

        Ok(size)
    }
    pub(crate) fn env_mut_ptr(&self) -> *mut ffi::MDB_env {
        self.0.env
    }

    pub fn open_database<KC, DC>(
        &self,
        rtxn: &RoTxn,
        name: Option<&str>,
    ) -> Result<Option<Database<KC, DC>>>
    where
        KC: 'static,
        DC: 'static,
    {
        let types = (TypeId::of::<KC>(), TypeId::of::<DC>());
        match self.raw_init_database(rtxn.txn, name, Some(types), false) {
            Ok(dbi) => Ok(Some(Database::new(self.env_mut_ptr() as _, dbi))),
            Err(Error::Mdb(e)) if e.not_found() => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn open_poly_database(
        &self,
        rtxn: &RoTxn,
        name: Option<&str>,
    ) -> Result<Option<PolyDatabase>> {
        match self.raw_init_database(rtxn.txn, name, None, false) {
            Ok(dbi) => Ok(Some(PolyDatabase::new(self.env_mut_ptr() as _, dbi))),
            Err(Error::Mdb(e)) if e.not_found() => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn create_database<KC, DC>(
        &self,
        wtxn: &mut RwTxn,
        name: Option<&str>,
    ) -> Result<Database<KC, DC>>
    where
        KC: 'static,
        DC: 'static,
    {
        let types = (TypeId::of::<KC>(), TypeId::of::<DC>());
        match self.raw_init_database(wtxn.txn.txn, name, Some(types), true) {
            Ok(dbi) => Ok(Database::new(self.env_mut_ptr() as _, dbi)),
            Err(e) => Err(e),
        }
    }

    pub fn create_poly_database(
        &self,
        wtxn: &mut RwTxn,
        name: Option<&str>,
    ) -> Result<PolyDatabase> {
        match self.raw_init_database(wtxn.txn.txn, name, None, true) {
            Ok(dbi) => Ok(PolyDatabase::new(self.env_mut_ptr() as _, dbi)),
            Err(e) => Err(e),
        }
    }

    fn raw_open_dbi(
        &self,
        raw_txn: *mut ffi::MDB_txn,
        name: Option<&str>,
        flags: u32,
    ) -> std::result::Result<u32, crate::mdb::error::Error> {
        let mut dbi = 0;
        let name = name.map(|n| CString::new(n).unwrap());
        let name_ptr = match name {
            Some(ref name) => name.as_bytes_with_nul().as_ptr() as *const _,
            None => ptr::null(),
        };

        // safety: The name cstring is cloned by LMDB, we can drop it after.
        //         If a read-only is used with the MDB_CREATE flag, LMDB will throw an error.
        unsafe { mdb_result(ffi::mdb_dbi_open(raw_txn, name_ptr, flags, &mut dbi))? };

        Ok(dbi)
    }

    fn raw_init_database(
        &self,
        raw_txn: *mut ffi::MDB_txn,
        name: Option<&str>,
        types: Option<(TypeId, TypeId)>,
        create: bool,
    ) -> Result<u32> {
        let mut lock = self.0.dbi_open_mutex.lock().unwrap();

        let flags = if create { ffi::MDB_CREATE } else { 0 };
        match self.raw_open_dbi(raw_txn, name, flags) {
            Ok(dbi) => {
                let old_types = lock.entry(dbi).or_insert(types);
                if *old_types == types {
                    Ok(dbi)
                } else {
                    Err(Error::InvalidDatabaseTyping)
                }
            }
            Err(e) => Err(e.into()),
        }
    }

    pub fn write_txn(&self) -> Result<RwTxn> {
        RwTxn::new(self)
    }

    pub fn typed_write_txn<T>(&self) -> Result<RwTxn<T>> {
        RwTxn::<T>::new(self)
    }

    pub fn nested_write_txn<'e, 'p: 'e, T>(
        &'e self,
        parent: &'p mut RwTxn<T>,
    ) -> Result<RwTxn<'e, 'p, T>> {
        RwTxn::nested(self, parent)
    }

    pub fn read_txn(&self) -> Result<RoTxn> {
        RoTxn::new(self)
    }

    pub fn typed_read_txn<T>(&self) -> Result<RoTxn<T>> {
        RoTxn::new(self)
    }

    // TODO rename into `copy_to_file` for more clarity
    pub fn copy_to_path<P: AsRef<Path>>(&self, path: P, option: CompactionOption) -> Result<File> {
        let file = File::options().create_new(true).write(true).open(&path)?;
        let fd = get_file_fd(&file);

        unsafe {
            self.copy_to_fd(fd, option)?;
        }

        // We reopen the file to make sure the cursor is at the start,
        // even a seek to start doesn't work properly.
        let file = File::open(path)?;

        Ok(file)
    }

    pub unsafe fn copy_to_fd(
        &self,
        fd: ffi::mdb_filehandle_t,
        option: CompactionOption,
    ) -> Result<()> {
        let flags = if let CompactionOption::Enabled = option { ffi::MDB_CP_COMPACT } else { 0 };

        mdb_result(ffi::mdb_env_copy2fd(self.0.env, fd, flags))?;

        Ok(())
    }

    #[cfg(all(feature = "lmdb", not(feature = "mdbx")))]
    pub fn force_sync(&self) -> Result<()> {
        unsafe { mdb_result(ffi::mdb_env_sync(self.0.env, 1))? }

        Ok(())
    }

    #[cfg(all(feature = "mdbx", not(feature = "lmdb")))]
    pub fn force_sync(&self) -> Result<()> {
        unsafe { mdb_result(ffi::mdb_env_sync(self.0.env))? }

        Ok(())
    }

    /// Returns the canonicalized path where this env lives.
    pub fn path(&self) -> &Path {
        &self.0.path
    }

    /// Returns an `EnvClosingEvent` that can be used to wait for the closing event,
    /// multiple threads can wait on this event.
    ///
    /// Make sure that you drop all the copies of `Env`s you have, env closing are triggered
    /// when all references are dropped, the last one will eventually close the environment.
    pub fn prepare_for_closing(self) -> EnvClosingEvent {
        let mut lock = OPENED_ENV.write().unwrap();
        let env = lock.get_mut(&self.0.path);

        match env {
            None => panic!("cannot find the env that we are trying to close"),
            Some(EnvEntry { env, signal_event, .. }) => {
                // We remove the env from the global list and replace it with a None.
                let _env = env.take();
                let signal_event = signal_event.clone();

                // we must make sure we release the lock before we drop the env
                // as the drop of the EnvInner also tries to lock the OPENED_ENV
                // global and we don't want to trigger a dead-lock.
                drop(lock);

                EnvClosingEvent(signal_event)
            }
        }
    }
}

#[derive(Clone)]
pub struct EnvClosingEvent(Arc<SignalEvent>);

impl EnvClosingEvent {
    /// Blocks this thread until the environment is effectively closed.
    ///
    /// # Safety
    ///
    /// Make sure that you don't have any copy of the environment in the thread
    /// that is waiting for a close event, if you do, you will have a dead-lock.
    pub fn wait(&self) {
        self.0.wait()
    }

    /// Blocks this thread until either the environment has been closed
    /// or until the timeout elapses, returns `true` if the environment
    /// has been effectively closed.
    pub fn wait_timeout(&self, timeout: Duration) -> bool {
        self.0.wait_timeout(timeout)
    }
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use tempfile::tempdir;

    use crate::types::*;
    use crate::{env_closing_event, EnvOpenOptions};

    #[test]
    fn close_env() {
        let dir = tempdir().unwrap();
        let path = dir.path();
        let env = EnvOpenOptions::new()
            .map_size(10 * 1024 * 1024) // 10MB
            .max_dbs(30)
            .open(&path)
            .unwrap();

        // Force a thread to keep the env for 1 second.
        let env_cloned = env.clone();
        thread::spawn(move || {
            let _env = env_cloned;
            thread::sleep(Duration::from_secs(1));
        });

        let mut wtxn = env.write_txn().unwrap();
        let db = env.create_database::<Str, Str>(&mut wtxn, None).unwrap();

        // Create an ordered list of keys...
        db.put(&mut wtxn, "hello", "hello").unwrap();
        db.put(&mut wtxn, "world", "world").unwrap();

        // Lets check that we can prefix_iter on that sequence with the key "255".
        let mut iter = db.iter(&wtxn).unwrap();
        // assert_eq!(iter.next().transpose().unwrap(), Some(("hello", "hello")));
        // assert_eq!(iter.next().transpose().unwrap(), Some(("world", "world")));
        assert_eq!(iter.next().transpose().unwrap(), None);
        drop(iter);

        wtxn.commit().unwrap();

        let signal_event = env.prepare_for_closing();

        eprintln!("waiting for the env to be closed");
        signal_event.wait();
        eprintln!("env closed successfully");

        // Make sure we don't have a reference to the env
        assert!(env_closing_event(&path).is_none());
    }

    #[test]
    fn reopen_env_with_different_options_is_err() {
        let dir = tempdir().unwrap();
        let path = dir.path();
        let _env = EnvOpenOptions::new()
            .map_size(10 * 1024 * 1024) // 10MB
            .open(&path)
            .unwrap();

        let env = EnvOpenOptions::new()
            .map_size(12 * 1024 * 1024) // 10MB
            .open(&path);

        assert!(env.is_err());
    }
    #[test]
    fn test_geometry() {
        let dir = tempdir().unwrap();
        let path = dir.path();
        let _env = EnvOpenOptions::new()
            .map_size(10 * 1024 * 1024) // 10MB
            .open(&path)
            .unwrap();
    }
}
