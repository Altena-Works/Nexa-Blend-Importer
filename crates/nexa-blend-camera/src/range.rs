//! Bounded, one-process camera-range evaluation.
use super::{CameraSample, DecodeError, MAX_PACKET_BYTES, Reader, decode};
use std::{fmt, fs::{self, File}, io::Read, path::Path, process::{Command, Stdio}};

pub const MAX_RANGE_SAMPLES: usize = 108_001;
pub const MAX_RANGE_BYTES: usize = 128 * 1024 * 1024;
const COMMON: &str = include_str!("../../../tools/blender_camera_export.py");
const RANGE: &str = include_str!("../../../tools/blender_camera_range_export.py");

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RangeRequest<'a> {
    pub start: Option<f64>,
    pub end: Option<f64>,
    pub scene: Option<&'a str>,
    pub camera: Option<&'a str>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct CameraRange { pub samples: Vec<CameraSample>, pub input_fps: f64 }
#[derive(Debug)]
pub enum RangeError {
    Invalid(&'static str),
    Container(DecodeError),
    Packet { sample: usize, source: DecodeError },
    Io(std::io::Error),
    BlenderFailed(Option<i32>),
}
impl fmt::Display for RangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(s) => f.write_str(s),
            Self::Container(e) => write!(f, "range container: {e}"),
            Self::Packet {sample,source} => write!(f, "camera sample {sample}: {source}"),
            Self::Io(e) => write!(f,"camera range I/O: {e}"),
            Self::BlenderFailed(code) => write!(f,"Blender range evaluation failed: {code:?}"),
        }
    }
}
impl std::error::Error for RangeError {}
impl From<std::io::Error> for RangeError { fn from(e:std::io::Error)->Self {Self::Io(e)} }

pub fn decode_range(data: &[u8]) -> Result<CameraRange, RangeError> {
    if data.len() > MAX_RANGE_BYTES { return Err(RangeError::Invalid("camera range is too large")); }
    let mut r = Reader { data, offset: 0 };
    if r.take(8).map_err(RangeError::Container)? != b"NEXARAN1" {
        return Err(RangeError::Invalid("invalid camera range magic"));
    }
    let count = r.u32().map_err(RangeError::Container)? as usize;
    if !(1..=MAX_RANGE_SAMPLES).contains(&count) { return Err(RangeError::Invalid("invalid sample count")); }
    // No allocation from a count that cannot fit even packet-size prefixes.
    if count > data.len().saturating_sub(r.offset) / 4 {
        return Err(RangeError::Invalid("truncated camera range"));
    }
    let mut samples: Vec<CameraSample> = Vec::with_capacity(count);
    for sample in 0..count {
        let size = r.u32().map_err(RangeError::Container)? as usize;
        if !(1..=MAX_PACKET_BYTES).contains(&size) {return Err(RangeError::Invalid("invalid sample size"));}
        let packet = r.take(size).map_err(RangeError::Container)?;
        let next = decode(packet).map_err(|source| RangeError::Packet {sample,source})?;
        if let Some(first) = samples.first() {
            if first.scene_name != next.scene_name || first.view_layer_name != next.view_layer_name
                || first.fps != next.fps || first.fps_base != next.fps_base {
                return Err(RangeError::Invalid("scene, view layer or FPS changed during the range"));
            }
        }
        samples.push(next);
    }
    if r.offset != data.len() {return Err(RangeError::Invalid("trailing camera range bytes"));}
    let input_fps = samples[0].fps as f64 / samples[0].fps_base;
    if !input_fps.is_finite() || input_fps <= 0.0 {return Err(RangeError::Invalid("invalid input FPS"));}
    Ok(CameraRange {samples,input_fps})
}
pub fn read_range(path: &Path) -> Result<CameraRange, RangeError> {
    let mut bytes=Vec::new();
    File::open(path)?.take(MAX_RANGE_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    decode_range(&bytes)
}
fn literal(text:&str) -> String {
    let mut out=String::from("'");
    for c in text.chars() {
        match c { '\\' => out.push_str("\\\\"), '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"), '\r' => out.push_str("\\r"), '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('\''); out
}
pub fn import_range(blender:&Path, source:&Path, output:&Path, request:RangeRequest<'_>)
    -> Result<CameraRange,RangeError>
{
    for frame in [request.start,request.end].into_iter().flatten() {
        if !frame.is_finite() || !(-1_048_574.0..=1_048_574.0).contains(&frame) {
            return Err(RangeError::Invalid("range endpoint outside Blender frame limits"));
        }
    }
    if let (Some(start),Some(end))=(request.start,request.end) {
        if start>end {return Err(RangeError::Invalid("start must not exceed end"));}
    }
    match fs::symlink_metadata(output) {
        Ok(_) => return Err(RangeError::Invalid("range output already exists")),
        Err(e) if e.kind()==std::io::ErrorKind::NotFound => {},
        Err(e) => return Err(e.into()),
    }
    let expression=format!("n={{'__name__':'nexa_camera_common'}};exec({},n);n['__name__']='__main__';exec({},n)",literal(COMMON),literal(RANGE));
    let mut command=Command::new(blender);
    command.args(["--background","--factory-startup","--disable-autoexec","--python-exit-code","1","--python-expr"])
        .arg(expression).args(["--","--input"]).arg(source).arg("--output").arg(output);
    if let Some(start)=request.start {command.arg("--start").arg(start.to_string());}
    if let Some(end)=request.end {command.arg("--end").arg(end.to_string());}
    if let Some(scene)=request.scene {command.arg("--scene").arg(scene);}
    if let Some(camera)=request.camera {command.arg("--camera").arg(camera);}
    let status=command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::inherit()).status()?;
    if !status.success() {return Err(RangeError::BlenderFailed(status.code()));}
    read_range(output)
}
