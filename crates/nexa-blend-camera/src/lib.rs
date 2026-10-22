//! Import an evaluated Blender camera packet without camera or coordinate conversion.

use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;
use std::process::{Command, Stdio};

pub const MAX_PACKET_BYTES: usize = 65_536;
const EXPORT_SCRIPT: &str = include_str!("../../../tools/blender_camera_export.py");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraKind { Perspective, Orthographic }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensorFit { Auto, Horizontal, Vertical }

#[derive(Clone, Debug, PartialEq)]
pub struct CameraSample {
    pub blender_version: [u32; 3],
    pub scene_name: String,
    pub view_layer_name: String,
    pub camera_name: String,
    pub frame: i32,
    pub subframe: f64,
    pub evaluated_frame: f64,
    pub frame_start: i32,
    pub frame_end: i32,
    pub fps: u32,
    pub fps_base: f64,
    pub frame_map_old: u32,
    pub frame_map_new: u32,
    pub kind: CameraKind,
    pub sensor_fit: SensorFit,
    pub lens_mm: f64,
    pub sensor_width_mm: f64,
    pub sensor_height_mm: f64,
    pub shift_x: f64,
    pub shift_y: f64,
    pub ortho_scale: f64,
    pub clip_start: f64,
    pub clip_end: f64,
    pub unit_scale: f64,
    pub resolution: [u32; 2],
    pub resolution_percentage: u32,
    pub pixel_aspect: [f64; 2],
    pub matrix_world: [[f64; 4]; 4],
    pub matrix_projection: [[f64; 4]; 4],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Request<'a> {
    pub frame: f64,
    pub scene: Option<&'a str>,
    pub camera: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    TooLarge { size: usize },
    InvalidMagic,
    Truncated { offset: usize, needed: usize, remaining: usize },
    InvalidUtf8 { offset: usize, valid_up_to: usize, error_len: Option<usize> },
    InvalidEnum { offset: usize, value: u8 },
    InvalidValue { field: &'static str },
    TrailingBytes { offset: usize, remaining: usize },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { size } => write!(f, "camera packet size {size} exceeds {MAX_PACKET_BYTES} bytes"),
            Self::InvalidMagic => f.write_str("invalid camera packet magic"),
            Self::Truncated { offset, needed, remaining } => write!(
                f, "truncated camera packet at byte {offset}: need {needed}, have {remaining}"
            ),
            Self::InvalidUtf8 { offset, valid_up_to, error_len } => write!(
                f, "invalid UTF-8 payload at byte {offset}: valid prefix {valid_up_to}, error length {error_len:?}"
            ),
            Self::InvalidEnum { offset, value } => write!(f, "invalid camera enum {value} at byte {offset}"),
            Self::InvalidValue { field } => write!(f, "invalid camera value: {field}"),
            Self::TrailingBytes { offset, remaining } => write!(
                f, "camera packet has {remaining} trailing bytes at byte {offset}"
            ),
        }
    }
}

impl std::error::Error for DecodeError {}

#[derive(Debug)]
pub enum ImportError {
    Io { operation: &'static str, source: io::Error },
    InvalidRequest,
    OutputExists,
    BlenderFailed { code: Option<i32> },
    Decode(DecodeError),
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { operation, source } => write!(f, "{operation}: {source}"),
            Self::InvalidRequest => f.write_str("frame must be finite with floor in [-1048574, 1048574]"),
            Self::OutputExists => f.write_str("camera output already exists"),
            Self::BlenderFailed { code } => write!(f, "Blender camera export failed (exit code {code:?})"),
            Self::Decode(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl std::error::Error for ImportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Decode(error) => Some(error),
            _ => None,
        }
    }
}

struct Reader<'a> { data: &'a [u8], offset: usize }

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], DecodeError> {
        let remaining = self.data.len() - self.offset;
        if count > remaining {
            return Err(DecodeError::Truncated { offset: self.offset, needed: count, remaining });
        }
        let start = self.offset;
        self.offset += count;
        Ok(&self.data[start..self.offset])
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let mut bytes = [0; N];
        bytes.copy_from_slice(self.take(N)?);
        Ok(bytes)
    }

    fn u32(&mut self) -> Result<u32, DecodeError> { Ok(u32::from_le_bytes(self.array()?)) }
    fn i32(&mut self) -> Result<i32, DecodeError> { Ok(i32::from_le_bytes(self.array()?)) }

    fn number(&mut self, field: &'static str) -> Result<f64, DecodeError> {
        let value = f64::from_le_bytes(self.array()?);
        if !value.is_finite() { return Err(DecodeError::InvalidValue { field }); }
        Ok(value)
    }

    fn positive(&mut self, field: &'static str) -> Result<f64, DecodeError> {
        let value = self.number(field)?;
        if value <= 0.0 { return Err(DecodeError::InvalidValue { field }); }
        Ok(value)
    }

    fn positive_u32(&mut self, field: &'static str) -> Result<u32, DecodeError> {
        let value = self.u32()?;
        if value == 0 { return Err(DecodeError::InvalidValue { field }); }
        Ok(value)
    }

    fn string(&mut self, field: &'static str) -> Result<String, DecodeError> {
        let length = self.u32()? as usize;
        let offset = self.offset;
        let bytes = self.take(length)?;
        let value = std::str::from_utf8(bytes).map_err(|error| DecodeError::InvalidUtf8 {
            offset, valid_up_to: error.valid_up_to(), error_len: error.error_len(),
        })?;
        if value.is_empty() || value.contains('\0') {
            return Err(DecodeError::InvalidValue { field });
        }
        Ok(value.to_owned())
    }

    fn matrix(&mut self, field: &'static str) -> Result<[[f64; 4]; 4], DecodeError> {
        let mut matrix = [[0.0; 4]; 4];
        for row in &mut matrix {
            for value in row { *value = self.number(field)?; }
        }
        Ok(matrix)
    }
}

/// Decode exactly one NEXACAM1 packet. Matrices retain Blender row-major values.
pub fn decode(data: &[u8]) -> Result<CameraSample, DecodeError> {
    if data.len() > MAX_PACKET_BYTES { return Err(DecodeError::TooLarge { size: data.len() }); }
    let mut reader = Reader { data, offset: 0 };
    if reader.take(8)? != b"NEXACAM1" { return Err(DecodeError::InvalidMagic); }
    let blender_version = [reader.u32()?, reader.u32()?, reader.u32()?];
    let scene_name = reader.string("scene_name")?;
    let view_layer_name = reader.string("view_layer_name")?;
    let camera_name = reader.string("camera_name")?;
    let frame = reader.i32()?;
    let subframe = reader.number("subframe")?;
    if !(0.0..1.0).contains(&subframe) { return Err(DecodeError::InvalidValue { field: "subframe" }); }
    let evaluated_frame = reader.number("evaluated_frame")?;
    let frame_start = reader.i32()?;
    let frame_end = reader.i32()?;
    if frame_start > frame_end {
        return Err(DecodeError::InvalidValue { field: "frame_end" });
    }
    let fps = reader.positive_u32("fps")?;
    let fps_base = reader.positive("fps_base")?;
    let frame_map_old = reader.positive_u32("frame_map_old")?;
    let frame_map_new = reader.positive_u32("frame_map_new")?;
    let offset = reader.offset;
    let kind = match reader.take(1)?[0] {
        0 => CameraKind::Perspective,
        1 => CameraKind::Orthographic,
        value => return Err(DecodeError::InvalidEnum { offset, value }),
    };
    let offset = reader.offset;
    let sensor_fit = match reader.take(1)?[0] {
        0 => SensorFit::Auto,
        1 => SensorFit::Horizontal,
        2 => SensorFit::Vertical,
        value => return Err(DecodeError::InvalidEnum { offset, value }),
    };
    let lens_mm = reader.positive("lens_mm")?;
    let sensor_width_mm = reader.positive("sensor_width_mm")?;
    let sensor_height_mm = reader.positive("sensor_height_mm")?;
    let shift_x = reader.number("shift_x")?;
    let shift_y = reader.number("shift_y")?;
    let ortho_scale = reader.positive("ortho_scale")?;
    let clip_start = reader.positive("clip_start")?;
    let clip_end = reader.positive("clip_end")?;
    if clip_end <= clip_start { return Err(DecodeError::InvalidValue { field: "clip_end" }); }
    let unit_scale = reader.positive("unit_scale")?;
    let resolution = [reader.positive_u32("resolution_x")?, reader.positive_u32("resolution_y")?];
    let resolution_percentage = reader.positive_u32("resolution_percentage")?;
    let pixel_aspect = [reader.positive("pixel_aspect_x")?, reader.positive("pixel_aspect_y")?];
    let matrix_world = reader.matrix("matrix_world")?;
    let matrix_projection = reader.matrix("matrix_projection")?;
    if reader.offset != data.len() {
        return Err(DecodeError::TrailingBytes { offset: reader.offset, remaining: data.len() - reader.offset });
    }
    Ok(CameraSample {
        blender_version, scene_name, view_layer_name, camera_name,
        frame, subframe, evaluated_frame, frame_start, frame_end, fps, fps_base,
        frame_map_old, frame_map_new, kind, sensor_fit, lens_mm, sensor_width_mm,
        sensor_height_mm, shift_x, shift_y, ortho_scale, clip_start, clip_end, unit_scale,
        resolution, resolution_percentage, pixel_aspect, matrix_world, matrix_projection,
    })
}

/// Read a packet with a MAX_PACKET_BYTES + 1 cap even if the file grows during reading.
pub fn read(path: &Path) -> Result<CameraSample, ImportError> {
    let file = File::open(path).map_err(|source| ImportError::Io { operation: "open camera packet", source })?;
    let mut bytes = Vec::new();
    file.take(MAX_PACKET_BYTES as u64 + 1).read_to_end(&mut bytes)
        .map_err(|source| ImportError::Io { operation: "read camera packet", source })?;
    decode(&bytes).map_err(ImportError::Decode)
}

/// Block while Blender loads the source and exports one evaluated camera sample.
///
/// Runs Blender directly with factory startup and automatic script execution disabled.
/// Existing output is rejected; the exporter also creates it exclusively. The source
/// is not saved, output directories are not created and failures are not rolled back.
/// Embedded script errors remain visible on inherited stderr.
pub fn import(
    blender: &Path,
    source: &Path,
    output: &Path,
    request: Request<'_>,
) -> Result<CameraSample, ImportError> {
    if !request.frame.is_finite() || !(-1_048_574.0..=1_048_574.0).contains(&request.frame.floor()) {
        return Err(ImportError::InvalidRequest);
    }
    match fs::symlink_metadata(output) {
        Ok(_) => return Err(ImportError::OutputExists),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => return Err(ImportError::Io { operation: "inspect camera output", source }),
    }
    let mut command = Command::new(blender);
    command.args([
        "--background", "--factory-startup", "--disable-autoexec", "--python-exit-code", "1",
        "--python-expr", EXPORT_SCRIPT, "--", "--input",
    ]).arg(source).arg("--output").arg(output).arg("--frame").arg(request.frame.to_string());
    if let Some(scene) = request.scene { command.arg("--scene").arg(scene); }
    if let Some(camera) = request.camera { command.arg("--camera").arg(camera); }
    let status = command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::inherit())
        .status().map_err(|source| ImportError::Io { operation: "launch Blender", source })?;
    if !status.success() { return Err(ImportError::BlenderFailed { code: status.code() }); }
    read(output)
}

#[cfg(test)]
#[path = "packet_tests.rs"]
mod tests;

pub mod range;
pub mod convert;
pub mod vmd;
pub mod worker;
#[cfg(windows)]
pub mod gui;
#[cfg(windows)]
pub mod blender_detection;
#[cfg(test)]
mod conversion_tests;
