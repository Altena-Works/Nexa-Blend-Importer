//! Owned, new-folder output publication. Callers validate generated contents before publishing.

use std::{
    collections::BTreeSet,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

pub const MAX_OUTPUT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const MAX_OUTPUT_FILES: usize = 4096;
static NEXT_NAME: AtomicU64 = AtomicU64::new(0);

/// Source names and caller-provided paths cannot enter the output tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtifactName {
    ModelPmx(u16),
    ModelMotion(u16),
    CameraVmd,
    TexturePng(u16),
    NameMap,
}

impl ArtifactName {
    pub fn relative_path(self) -> Result<PathBuf, TransactionError> {
        let path = match self {
            Self::ModelPmx(n) if (1..=256).contains(&n) => format!("model_{n:04}.pmx"),
            Self::ModelMotion(n) if (1..=256).contains(&n) => format!("model_{n:04}.motion.vmd"),
            Self::TexturePng(n) if (1..=4096).contains(&n) => {
                format!("textures/texture_{n:04}.png")
            }
            Self::CameraVmd => "camera.vmd".into(),
            Self::NameMap => "name-map.tsv".into(),
            _ => return Err(TransactionError::InvalidArtifact),
        };
        Ok(path.into())
    }
}

#[derive(Debug)]
pub enum TransactionError {
    InvalidRoot,
    InvalidArtifact,
    UnsafeEntry(PathBuf),
    ExistingOutput(PathBuf),
    Limit(&'static str),
    Empty,
    Poisoned,
    Io {
        operation: &'static str,
        source: io::Error,
    },
}

impl TransactionError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidRoot => "invalid_output_root",
            Self::InvalidArtifact => "invalid_generated_artifact",
            Self::UnsafeEntry(_) => "unsafe_output_entry",
            Self::ExistingOutput(_) => "existing_output",
            Self::Limit(_) => "output_limit",
            Self::Empty => "empty_output",
            Self::Poisoned => "failed_transaction",
            Self::Io { .. } => "output_io",
        }
    }
}

impl fmt::Display for TransactionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsafeEntry(p) => write!(f, "unsafe output entry: {}", p.display()),
            Self::ExistingOutput(p) => write!(f, "output already exists: {}", p.display()),
            Self::Limit(field) => write!(f, "output limit exceeded: {field}"),
            Self::Io { operation, source } => write!(f, "{operation}: {source}"),
            _ => f.write_str(self.code()),
        }
    }
}

impl std::error::Error for TransactionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

fn io_error(operation: &'static str, source: io::Error) -> TransactionError {
    TransactionError::Io { operation, source }
}

fn is_reparse(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn check_directory(path: &Path) -> Result<(), TransactionError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|e| io_error("inspect output directory", e))?;
    if !metadata.is_dir() || is_reparse(&metadata) {
        return Err(TransactionError::UnsafeEntry(path.into()));
    }
    Ok(())
}

fn pin_directory(path: &Path, owned: bool) -> Result<File, TransactionError> {
    check_directory(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // No DELETE sharing: checked ancestors cannot be replaced while the transaction lives.
        options
            .access_mode(0x80 | if owned { 0x10000 } else { 0 })
            .share_mode(0x1 | 0x2)
            .custom_flags(0x0020_0000 | 0x0200_0000);
    }
    #[cfg(not(windows))]
    let _ = owned;
    let file = options
        .open(path)
        .map_err(|e| io_error("pin output directory", e))?;
    let metadata = file
        .metadata()
        .map_err(|e| io_error("inspect pinned directory", e))?;
    if !metadata.is_dir() || is_reparse(&metadata) {
        return Err(TransactionError::UnsafeEntry(path.into()));
    }
    Ok(file)
}

fn absent(path: &Path) -> Result<(), TransactionError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(TransactionError::ExistingOutput(path.into())),
        Err(error) => Err(io_error("inspect output destination", error)),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FileIdentity([u32; 3]);

#[cfg(windows)]
fn file_identity(file: &File) -> io::Result<FileIdentity> {
    use std::os::windows::io::AsRawHandle;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandle(handle: *mut core::ffi::c_void, information: *mut core::ffi::c_void)
        -> i32;
    }
    // BY_HANDLE_FILE_INFORMATION consists of thirteen DWORDs (FILETIMEs are pairs).
    let mut information = [0_u32; 13];
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr().cast()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(FileIdentity([
        information[7],
        information[11],
        information[12],
    ]))
}

#[cfg(not(windows))]
fn file_identity(_file: &File) -> io::Result<FileIdentity> {
    Ok(FileIdentity([0; 3]))
}

#[cfg(windows)]
fn mark_delete(file: &File) {
    use std::os::windows::io::AsRawHandle;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetFileInformationByHandle(
            handle: *mut core::ffi::c_void,
            class: i32,
            information: *const core::ffi::c_void,
            size: u32,
        ) -> i32;
    }
    let delete = 1_u8;
    let _ = unsafe {
        SetFileInformationByHandle(file.as_raw_handle(), 4, (&delete as *const u8).cast(), 1)
    };
}

struct OwnedEntry {
    path: PathBuf,
    file: Option<File>,
    identity: FileIdentity,
}

impl OwnedEntry {
    fn new(path: PathBuf, file: File) -> Result<Self, TransactionError> {
        let identity = match file_identity(&file) {
            Ok(identity) => identity,
            Err(error) => {
                #[cfg(windows)]
                mark_delete(&file);
                return Err(io_error("identify owned output", error));
            }
        };
        Ok(Self {
            path,
            file: Some(file),
            identity,
        })
    }
}

/// Windows x64 is the production target. No operation recursively removes a directory.
pub struct OutputTransaction {
    root: PathBuf,
    stage: PathBuf,
    destination: PathBuf,
    _root_pins: Vec<File>,
    directories: Vec<OwnedEntry>,
    files: Vec<OwnedEntry>,
    bytes: u64,
    poisoned: bool,
    published: bool,
}

impl OutputTransaction {
    /// The existing root must be an absolute local directory with no reparse-point ancestor.
    pub fn begin(root: &Path) -> Result<Self, TransactionError> {
        if !root.is_absolute() || root.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(TransactionError::InvalidRoot);
        }
        #[cfg(windows)]
        if !matches!(root.components().next(), Some(Component::Prefix(p))
            if matches!(p.kind(), std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)))
        {
            return Err(TransactionError::InvalidRoot);
        }
        let mut pins = Vec::new();
        let mut ancestor = PathBuf::new();
        for part in root.components() {
            ancestor.push(part.as_os_str());
            if matches!(part, Component::RootDir | Component::Normal(_)) {
                pins.push(pin_directory(&ancestor, false)?);
            }
        }
        let root = fs::canonicalize(root).map_err(|e| io_error("canonicalize output root", e))?;
        check_directory(&root)?;
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| TransactionError::InvalidRoot)?;
        let suffix = format!(
            "{}-{:09}-{:04}",
            time.as_secs(),
            time.subsec_nanos(),
            NEXT_NAME.fetch_add(1, Ordering::Relaxed)
        );
        let stage = root.join(format!(".nexa-import-stage-{suffix}"));
        let destination = root.join(format!("NexaImport-{suffix}"));
        absent(&destination)?;
        fs::create_dir(&stage).map_err(|e| io_error("create exclusive staging directory", e))?;
        let stage_pin = match pin_directory(&stage, true) {
            Ok(file) => file,
            Err(error) => {
                let _ = fs::remove_dir(&stage);
                return Err(error);
            }
        };
        Ok(Self {
            root,
            stage: stage.clone(),
            destination,
            _root_pins: pins,
            directories: vec![OwnedEntry::new(stage, stage_pin)?],
            files: Vec::new(),
            bytes: 0,
            poisoned: false,
            published: false,
        })
    }

    pub fn planned_output(&self) -> &Path {
        &self.destination
    }

    pub fn write_artifact(
        &mut self,
        name: ArtifactName,
        data: &[u8],
    ) -> Result<(), TransactionError> {
        if self.poisoned {
            return Err(TransactionError::Poisoned);
        }
        let result = self.write_inner(name, data);
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn write_inner(&mut self, name: ArtifactName, data: &[u8]) -> Result<(), TransactionError> {
        if self.files.len() >= MAX_OUTPUT_FILES {
            return Err(TransactionError::Limit("files"));
        }
        let bytes = self
            .bytes
            .checked_add(data.len() as u64)
            .filter(|total| *total <= MAX_OUTPUT_BYTES)
            .ok_or(TransactionError::Limit("bytes"))?;
        let relative = name.relative_path()?;
        self.check_owned_directories()?;
        if matches!(name, ArtifactName::TexturePng(_)) && self.directories.len() == 1 {
            let folder = self.stage.join("textures");
            fs::create_dir(&folder)
                .map_err(|e| io_error("create exclusive texture directory", e))?;
            let pin = match pin_directory(&folder, true) {
                Ok(file) => file,
                Err(error) => {
                    let _ = fs::remove_dir(&folder);
                    return Err(error);
                }
            };
            self.directories.push(OwnedEntry::new(folder, pin)?);
        }
        let path = self.stage.join(relative);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options
                .access_mode(0x4000_0000 | 0x10000)
                .share_mode(0x1)
                .custom_flags(0x0020_0000);
        }
        let file = options
            .open(&path)
            .map_err(|e| io_error("create exclusive output file", e))?;
        self.files.push(OwnedEntry::new(path, file)?);
        let file = self
            .files
            .last_mut()
            .expect("owned file")
            .file
            .as_mut()
            .expect("open file");
        file.write_all(data)
            .map_err(|e| io_error("write output file", e))?;
        file.sync_all()
            .map_err(|e| io_error("flush output file", e))?;
        self.bytes = bytes;
        Ok(())
    }

    fn check_owned_directories(&self) -> Result<(), TransactionError> {
        check_directory(&self.root)?;
        for entry in &self.directories {
            check_directory(&entry.path)?;
            let canonical = fs::canonicalize(&entry.path)
                .map_err(|e| io_error("canonicalize staging directory", e))?;
            if !canonical.starts_with(&self.root) {
                return Err(TransactionError::UnsafeEntry(entry.path.clone()));
            }
        }
        Ok(())
    }

    /// Publish every completed artifact with one same-root rename. Never replaces an entry.
    pub fn publish(mut self) -> Result<PathBuf, TransactionError> {
        if self.poisoned {
            return Err(TransactionError::Poisoned);
        }
        if self.files.is_empty() {
            return Err(TransactionError::Empty);
        }
        self.check_owned_directories()?;
        // Reject foreign entries rather than publishing or deleting somebody else's data.
        let expected: BTreeSet<_> = self
            .files
            .iter()
            .chain(self.directories.iter().skip(1))
            .map(|entry| entry.path.clone())
            .collect();
        for directory in &self.directories {
            for entry in fs::read_dir(&directory.path)
                .map_err(|e| io_error("inspect staging contents", e))?
            {
                let path = entry
                    .map_err(|e| io_error("inspect staging entry", e))?
                    .path();
                let metadata = fs::symlink_metadata(&path)
                    .map_err(|e| io_error("inspect staging entry", e))?;
                if is_reparse(&metadata) || !expected.contains(&path) {
                    return Err(TransactionError::UnsafeEntry(path));
                }
            }
        }
        absent(&self.destination)?;
        for entry in self.files.iter_mut().chain(self.directories.iter_mut()) {
            entry.file.take();
        }
        rename_new(&self.stage, &self.destination)
            .map_err(|e| io_error("publish new output folder", e))?;
        self.published = true;
        Ok(self.destination.clone())
    }
}

#[cfg(windows)]
fn rename_new(source: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }
    let source: Vec<_> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let target: Vec<_> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    // Zero flags deliberately excludes MOVEFILE_REPLACE_EXISTING and cross-volume copy.
    if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), 0) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn rename_new(_source: &Path, _target: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic no-replace publication requires Windows",
    ))
}

impl Drop for OutputTransaction {
    fn drop(&mut self) {
        if self.published {
            return;
        }
        // Windows handles delete precisely our entries, including after a failed rename.
        #[allow(unused_mut)]
        let mut ancestors_safe = self.check_owned_directories().is_ok();
        #[cfg(windows)]
        if ancestors_safe {
            // Pin the original staging ancestors again before reopening any child.
            for directory in &mut self.directories {
                if directory.file.is_none() {
                    match pin_directory(&directory.path, true) {
                        Ok(file) if file_identity(&file).ok() == Some(directory.identity) => {
                            directory.file = Some(file);
                        }
                        _ => {
                            ancestors_safe = false;
                            break;
                        }
                    }
                }
            }
        }
        for entry in self
            .files
            .iter_mut()
            .rev()
            .chain(self.directories.iter_mut().rev())
        {
            #[cfg(windows)]
            {
                // A failed rename has released handles. Reopen only through checked ancestors,
                // without following a reparse point, and require the original file identity.
                if entry.file.is_none() && ancestors_safe {
                    use std::os::windows::fs::OpenOptionsExt;
                    if let Ok(file) = OpenOptions::new()
                        .read(true)
                        .access_mode(0x80 | 0x10000)
                        .share_mode(0x1 | 0x2)
                        .custom_flags(0x0020_0000 | 0x0200_0000)
                        .open(&entry.path)
                    {
                        if file_identity(&file).ok() == Some(entry.identity)
                            && file.metadata().is_ok_and(|m| !is_reparse(&m))
                        {
                            entry.file = Some(file);
                        }
                    }
                }
                if let Some(file) = entry.file.as_ref() {
                    mark_delete(file);
                    entry.file.take();
                }
                // Unknown identities/ancestors are deliberately left untouched.
            }
            #[cfg(not(windows))]
            {
                entry.file.take();
                if let Ok(metadata) = fs::symlink_metadata(&entry.path) {
                    if ancestors_safe
                        && !is_reparse(&metadata)
                        && self.stage.starts_with(&self.root)
                        && entry.path.starts_with(&self.stage)
                    {
                        if metadata.is_file() {
                            let _ = fs::remove_file(&entry.path);
                        } else if metadata.is_dir() {
                            let _ = fs::remove_dir(&entry.path);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "nexa-importer-{label}-{}-{}",
            std::process::id(),
            NEXT_NAME.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        root
    }

    #[test]
    #[cfg(windows)]
    fn success_and_failure_preserve_existing_output() {
        let root = test_root("transaction");
        let existing = root.join("existing");
        fs::create_dir(&existing).unwrap();
        fs::write(existing.join("keep.txt"), b"original").unwrap();
        let published = {
            let mut transaction = OutputTransaction::begin(&root).unwrap();
            transaction
                .write_artifact(ArtifactName::ModelPmx(1), b"model")
                .unwrap();
            transaction
                .write_artifact(ArtifactName::TexturePng(1), b"texture")
                .unwrap();
            transaction.publish().unwrap()
        };
        assert_eq!(
            fs::read(published.join("model_0001.pmx")).unwrap(),
            b"model"
        );
        assert_eq!(
            fs::read(published.join("textures/texture_0001.png")).unwrap(),
            b"texture"
        );
        let stage;
        let collision;
        {
            let mut transaction = OutputTransaction::begin(&root).unwrap();
            stage = transaction.stage.clone();
            collision = transaction.planned_output().to_path_buf();
            transaction
                .write_artifact(ArtifactName::CameraVmd, b"new")
                .unwrap();
            fs::create_dir(&collision).unwrap();
            fs::write(collision.join("keep.txt"), b"collision").unwrap();
            assert!(matches!(
                transaction.publish(),
                Err(TransactionError::ExistingOutput(_))
            ));
        }
        assert!(
            !stage.exists(),
            "failed transaction must remove its own staging"
        );
        assert_eq!(fs::read(collision.join("keep.txt")).unwrap(), b"collision");
        assert_eq!(fs::read(existing.join("keep.txt")).unwrap(), b"original");
        {
            let mut transaction = OutputTransaction::begin(&root).unwrap();
            let own_stage = transaction.stage.clone();
            transaction
                .write_artifact(ArtifactName::CameraVmd, b"first")
                .unwrap();
            assert!(
                transaction
                    .write_artifact(ArtifactName::CameraVmd, b"replace")
                    .is_err()
            );
            assert!(matches!(
                transaction.publish(),
                Err(TransactionError::Poisoned)
            ));
            assert!(!own_stage.exists());
        }
        assert!(ArtifactName::ModelPmx(0).relative_path().is_err());
        fs::remove_file(published.join("textures/texture_0001.png")).unwrap();
        fs::remove_file(published.join("model_0001.pmx")).unwrap();
        fs::remove_dir(published.join("textures")).unwrap();
        fs::remove_dir(published).unwrap();
        fs::remove_file(collision.join("keep.txt")).unwrap();
        fs::remove_dir(collision).unwrap();
        fs::remove_file(existing.join("keep.txt")).unwrap();
        fs::remove_dir(existing).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
