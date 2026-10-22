//! One owned Blender process evaluates the selected scene without saving the input.
use crate::{ImportRequest, packet::{self, ScenePacket}};
use std::{fs::{self, File, OpenOptions}, io::Write, path::{Component, Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio}, sync::atomic::{AtomicBool, AtomicU64, Ordering},
    thread, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};

const SCRIPT: &str = include_str!("../../../tools/blender_scene_export.py");
const CAMERA_COMMON: &str = include_str!("../../../tools/blender_camera_export.py");
static NEXT: AtomicU64 = AtomicU64::new(0);
pub const BLENDER_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Clone, Debug)]
pub struct SourceOptions {
    pub blender: PathBuf,
    pub input: PathBuf,
    pub scratch_parent: PathBuf,
    pub scene: Option<String>,
    pub request: ImportRequest,
}

#[derive(Debug)]
pub enum SourceError {
    Invalid(&'static str),
    Cancelled,
    Timeout,
    BlenderFailed(Option<i32>),
    Extraction(String),
    Io(std::io::Error),
    Packet(packet::PacketError),
}
impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(s) => f.write_str(s),
            Self::Cancelled => f.write_str("conversion cancelled"),
            Self::Timeout => f.write_str("Blender exceeded the 600-second limit"),
            Self::BlenderFailed(code) => write!(f, "Blender extraction failed: {code:?}"),
            Self::Extraction(reason) => f.write_str(reason),
            Self::Io(e) => write!(f, "scene I/O: {e}"),
            Self::Packet(e) => write!(f, "scene packet: {e}"),
        }
    }
}
impl std::error::Error for SourceError {}
impl From<std::io::Error> for SourceError { fn from(e: std::io::Error) -> Self { Self::Io(e) } }

fn no_reparse(path: &Path) -> Result<(), SourceError> {
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(SourceError::Invalid("an absolute local path without parent traversal is required"));
    }
    #[cfg(windows)]
    if !matches!(path.components().next(), Some(Component::Prefix(p))
        if matches!(p.kind(), std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_))) {
        return Err(SourceError::Invalid("network paths are not supported"));
    }
    let mut current = PathBuf::new();
    for part in path.components() {
        current.push(part.as_os_str());
        if matches!(part, Component::Normal(_) | Component::RootDir) {
            let metadata = fs::symlink_metadata(&current)?;
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(SourceError::Invalid("reparse points are not supported"));
                }
            }
            if metadata.file_type().is_symlink() {
                return Err(SourceError::Invalid("symbolic links are not supported"));
            }
        }
    }
    Ok(())
}

struct Scratch { directory: PathBuf, script: PathBuf, common: PathBuf, packet: PathBuf, error: PathBuf }
impl Scratch {
    fn new(parent: &Path) -> Result<Self, SourceError> {
        no_reparse(parent)?;
        if !parent.is_dir() { return Err(SourceError::Invalid("scratch parent must be an existing directory")); }
        let parent = fs::canonicalize(parent)?;
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH)
            .map_err(|_| SourceError::Invalid("system clock precedes the epoch"))?.as_nanos();
        let directory = parent.join(format!(".nexa-scene-{}-{stamp}-{serial}", std::process::id()));
        fs::create_dir(&directory)?;
        let scratch = Self { script: directory.join("export.py"), common: directory.join("blender_camera_export.py"), packet: directory.join("scene.bin"), error: directory.join("error.txt"), directory };
        for (path, contents) in [(&scratch.script, SCRIPT), (&scratch.common, CAMERA_COMMON)] {
            let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
            file.write_all(contents.as_bytes())?;
            file.sync_all()?;
        }
        Ok(scratch)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        // Exact disposable entries only; never recursively delete a user directory.
        if no_reparse(&self.directory).is_err() { return; }
        // Blender's finally block cannot run after a cancellation kill. Reclaim
        // only its generated PNG names inside this invocation's private scratch.
        if let Ok(entries) = fs::read_dir(&self.directory) {
            for entry in entries.take(16).flatten() {
                let path = entry.path();
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue; };
                if !name.strip_prefix(".nexa-scene-").is_some_and(|suffix|
                    !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
                    || no_reparse(&path).is_err() || !path.is_dir() { continue; }
                if let Ok(images) = fs::read_dir(&path) {
                    for image in images.take(4096).flatten() {
                        let image_path = image.path();
                        let name = image.file_name();
                        let Some(name) = name.to_str() else { continue; };
                        let generated = name.strip_prefix("image_").and_then(|n| n.strip_suffix(".png"))
                            .and_then(|n| n.parse::<u32>().ok()).is_some_and(|n|
                                (1..=4096).contains(&n) && name == format!("image_{n:04}.png"));
                        if generated && no_reparse(&image_path).is_ok() && image_path.is_file() {
                            let _ = fs::remove_file(image_path);
                        }
                    }
                }
                let _ = fs::remove_dir(path);
            }
        }
        for path in [&self.script, &self.common, &self.packet, &self.error] {
            if fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && !m.file_type().is_symlink()) {
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if fs::symlink_metadata(path).is_ok_and(|m| m.file_attributes() & 0x400 != 0) { continue; }
                }
                let _ = fs::remove_file(path);
            }
        }
        let _ = fs::remove_dir(&self.directory);
    }
}

struct OwnedChild(Option<Child>);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 { let _ = child.kill(); let _ = child.wait(); }
    }
}
fn wait_owned(child: Child, cancelled: &AtomicBool, timeout: Duration) -> Result<ExitStatus, SourceError> {
    let mut owned = OwnedChild(Some(child));
    let started = Instant::now();
    loop {
        if let Some(status) = owned.0.as_mut().expect("owned child").try_wait()? {
            owned.0.take();
            return Ok(status);
        }
        if cancelled.load(Ordering::Acquire) { return Err(SourceError::Cancelled); }
        if started.elapsed() >= timeout { return Err(SourceError::Timeout); }
        thread::sleep(Duration::from_millis(20));
    }
}

fn input_file(path: &Path) -> Result<File, SourceError> {
    no_reparse(path)?;
    if !path.extension().is_some_and(|s| s.eq_ignore_ascii_case("blend")) {
        return Err(SourceError::Invalid("input must be a .blend file"));
    }
    let mut options = OpenOptions::new(); options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Hold the source read-only throughout extraction, denying writes and replacement.
        options.share_mode(1).custom_flags(0x0020_0000);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() { return Err(SourceError::Invalid("input must be a regular file")); }
    Ok(file)
}

pub fn import_scene(options: &SourceOptions, cancelled: &AtomicBool) -> Result<ScenePacket, SourceError> {
    options.request.selection.validate().map_err(|_| SourceError::Invalid("at least one component is required"))?;
    if !options.request.units_per_meter.is_finite() || options.request.units_per_meter <= 0.0 {
        return Err(SourceError::Invalid("unit scale must be positive and finite"));
    }
    let frames = options.request.frame_range;
    for n in options.request.reference_frame.into_iter().chain(frames.into_iter().flat_map(|r| [r.start, r.end])) {
        if !n.is_finite() || !(-1_048_574.0..=1_048_574.0).contains(&n) {
            return Err(SourceError::Invalid("frame is outside Blender's supported range"));
        }
    }
    if frames.is_some_and(|r| r.end < r.start) { return Err(SourceError::Invalid("frame range is reversed")); }
    if cancelled.load(Ordering::Acquire) { return Err(SourceError::Cancelled); }
    no_reparse(&options.blender)?;
    if !options.blender.is_file() { return Err(SourceError::Invalid("Blender executable is missing")); }
    let _input = input_file(&options.input)?;
    let scratch = Scratch::new(&options.scratch_parent)?;
    let selected = options.request.selection;
    let selection = [(selected.model, "model"), (selected.camera, "camera"), (selected.animation, "animation")]
        .into_iter().filter_map(|(yes, name)| yes.then_some(name)).collect::<Vec<_>>().join(",");
    let mut command = Command::new(&options.blender);
    command.args(["--background", "--factory-startup", "--disable-autoexec", "--python-exit-code", "1", "--python"])
        .arg(&scratch.script).arg("--").arg("--input").arg(&options.input)
        .arg("--error").arg(&scratch.error)
        .arg("--packet").arg(&scratch.packet).arg("--selection").arg(selection)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    { use std::os::windows::process::CommandExt; command.creation_flags(0x0800_0000); }
    if let Some(scene) = &options.scene { command.arg("--scene").arg(scene); }
    if let Some(frame) = options.request.reference_frame { command.arg("--reference-frame").arg(frame.to_string()); }
    if let Some(range) = frames { command.arg("--frame-start").arg(range.start.to_string()).arg("--frame-end").arg(range.end.to_string()); }
    let status = wait_owned(command.spawn()?, cancelled, BLENDER_TIMEOUT)?;
    if !status.success() {
        if no_reparse(&scratch.error).is_ok() {
            use std::io::Read;
            if let Ok(file) = File::open(&scratch.error) {
                let mut bytes = Vec::new();
                if file.take(8192).read_to_end(&mut bytes).is_ok() && !bytes.is_empty() {
                    return Err(SourceError::Extraction(String::from_utf8_lossy(&bytes).into_owned()));
                }
            }
        }
        return Err(SourceError::BlenderFailed(status.code()));
    }
    no_reparse(&scratch.packet)?;
    packet::read_packet(&scratch.packet).map_err(SourceError::Packet)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "only launched as the owned child by lifecycle test"]
    fn controlled_child() { thread::sleep(Duration::from_secs(30)); }

    #[test]
    fn cancel_and_short_timeout_reap_owned_children() {
        let launch = || Command::new(std::env::current_exe().unwrap())
            .args(["source::tests::controlled_child", "--exact", "--ignored"])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let cancelled = AtomicBool::new(true);
        assert!(matches!(wait_owned(launch(), &cancelled, Duration::from_secs(5)), Err(SourceError::Cancelled)));
        assert!(matches!(wait_owned(launch(), &AtomicBool::new(false), Duration::from_millis(50)), Err(SourceError::Timeout)));
    }

    #[test]
    fn scratch_reclaims_interrupted_pngs_and_preserves_unknown_files() {
        let parent = std::env::current_dir().unwrap();
        let scratch = Scratch::new(&parent).unwrap();
        let directory = scratch.directory.clone();
        let assets = directory.join(".nexa-scene-owned01");
        fs::create_dir(&assets).unwrap();
        fs::write(assets.join("image_0001.png"), b"partial PNG").unwrap();
        fs::write(assets.join("keep.txt"), b"unowned").unwrap();
        let clean = directory.join(".nexa-scene-owned02");
        fs::create_dir(&clean).unwrap();
        fs::write(clean.join("image_4096.png"), b"partial PNG").unwrap();
        drop(scratch);
        assert!(!assets.join("image_0001.png").exists());
        assert_eq!(fs::read(assets.join("keep.txt")).unwrap(), b"unowned");
        assert!(!clean.exists());
        assert!(!directory.join("export.py").exists());
        fs::remove_file(assets.join("keep.txt")).unwrap();
        fs::remove_dir(assets).unwrap();
        fs::remove_dir(directory).unwrap();
    }
}
