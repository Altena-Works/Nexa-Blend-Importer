//! Version-one scene packet validation. This module does not run or extract from Blender.
//!
//! NEXASCN1 is little-endian. Strings/arrays have u32 byte/item prefixes; numbers are
//! f64, indices/ticks are u32, absent parents/images are i32(-1). Struct fields follow
//! declaration order. Camera enum tags are u8 in declaration order. Matrices are
//! Blender-space row-major matrices acting on column vectors. Units are not applied here.

use std::{collections::BTreeSet, fmt, fs::File, io::Read, path::Path};

pub const MAX_PACKET_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_GROUPS: usize = 256;
pub const MAX_GROUP_VERTICES: usize = 4_000_000;
pub const MAX_GROUP_BONES: usize = 65_535;
pub const MAX_IMAGE_EDGE: u32 = 16_384;
pub const MAX_TICKS: usize = 108_001;
const MAX_ITEMS: usize = (MAX_PACKET_BYTES / 4) as usize;
const MAX_STRING_BYTES: usize = 1024 * 1024;

pub type Matrix = [f64; 16];

#[derive(Clone, Debug, PartialEq)]
pub struct ScenePacket {
    pub scene_name: String,
    pub reference_frame: f64,
    pub frame_start: f64,
    pub frame_end: f64,
    pub frames_per_second: f64,
    pub unit_scale: f64,
    pub render_size: [u32; 2],
    pub groups: Vec<ModelPacket>,
    pub images: Vec<ImagePacket>,
    pub cameras: Vec<CameraSample>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelPacket {
    pub source_id: String,
    pub name: String,
    /// Evaluated group root at reference_frame, before sampling and unit conversion.
    pub reference_object_world: Matrix,
    pub meshes: Vec<MeshPacket>,
    pub bones: Vec<BonePacket>,
    pub poses: Vec<PoseSample>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MeshPacket {
    pub source_id: String,
    pub name: String,
    pub world_from_mesh: Matrix,
    /// Seam-split rest geometry. Morph and weight indices address these same vertices.
    pub vertices: Vec<Vertex>,
    pub triangles: Vec<Triangle>,
    pub materials: Vec<MaterialPacket>,
    pub morphs: Vec<VertexMorph>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Vertex {
    pub position: [f64; 3],
    pub normal: [f64; 3],
    pub uv: [f64; 2],
    /// Empty means the generated model root. Positive weights are normalized by the writer.
    pub weights: Vec<BoneWeight>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BoneWeight {
    pub bone: u32,
    pub weight: f64,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Triangle {
    pub vertices: [u32; 3],
    pub material: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BonePacket {
    pub source_id: String,
    pub name: String,
    pub parent: Option<u32>,
    /// Global rest basis including the armature object's world transform.
    pub rest_world: Matrix,
    pub head_world: [f64; 3],
    pub tail_world: [f64; 3],
}

#[derive(Clone, Debug, PartialEq)]
pub struct VertexMorph {
    pub source_id: String,
    pub name: String,
    /// Relative Basis differences in mesh-local source space.
    pub offsets: Vec<MorphOffset>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct MorphOffset {
    pub vertex: u32,
    pub delta: [f64; 3],
}

#[derive(Clone, Debug, PartialEq)]
pub struct MaterialPacket {
    pub name: String,
    /// Defined diffuse conversion of Principled Base Color/Alpha, not PBR equivalence.
    pub diffuse: [f64; 4],
    pub image: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImagePacket {
    pub source_id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Exported bytes only; source paths/URLs are never output paths.
    pub png: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PoseSample {
    pub tick: u32,
    pub source_frame: f64,
    pub object_world: Matrix,
    /// Evaluated world skin matrices in the exact rest-bone order; no local-quaternion shortcut.
    pub bone_skin_world: Vec<Matrix>,
    pub morph_weights: Vec<MorphWeight>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct MorphWeight {
    pub mesh: u32,
    pub morph: u32,
    pub weight: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Projection {
    Perspective,
    Orthographic,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensorFit {
    Auto,
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CameraSample {
    pub tick: u32,
    pub source_frame: f64,
    pub source_id: String,
    pub name: String,
    pub projection: Projection,
    pub sensor_fit: SensorFit,
    pub lens_mm: f64,
    pub sensor_mm: [f64; 2],
    pub shift: [f64; 2],
    pub ortho_scale: f64,
    pub clip: [f64; 2],
    pub pixel_aspect: [f64; 2],
    pub world_from_camera: Matrix,
    pub projection_matrix: Matrix,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PacketError {
    TooLarge,
    InvalidMagic,
    Truncated,
    Invalid(&'static str),
    Limit(&'static str),
    Allocation,
    TrailingBytes,
    Io(String),
}

impl PacketError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::TooLarge => "scene_packet_too_large",
            Self::InvalidMagic => "scene_packet_magic",
            Self::Truncated => "scene_packet_truncated",
            Self::Invalid(_) => "invalid_scene_packet",
            Self::Limit(_) => "scene_packet_limit",
            Self::Allocation => "scene_packet_allocation",
            Self::TrailingBytes => "scene_packet_trailing",
            Self::Io(_) => "scene_packet_io",
        }
    }
}
impl fmt::Display for PacketError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(field) | Self::Limit(field) => write!(f, "{}: {field}", self.code()),
            Self::Io(error) => write!(f, "{}: {error}", self.code()),
            _ => f.write_str(self.code()),
        }
    }
}
impl std::error::Error for PacketError {}

fn require(condition: bool, field: &'static str) -> Result<(), PacketError> {
    if condition {
        Ok(())
    } else {
        Err(PacketError::Invalid(field))
    }
}
fn finite(values: &[f64], field: &'static str) -> Result<(), PacketError> {
    require(values.iter().all(|v| v.is_finite()), field)
}
fn affine(m: &Matrix, field: &'static str) -> Result<(), PacketError> {
    finite(m, field)?;
    require(
        m[12..15].iter().all(|v| v.abs() <= 1e-10) && (m[15] - 1.0).abs() <= 1e-10,
        field,
    )
}

struct Size(u64);
impl Size {
    fn add(&mut self, amount: usize) -> Result<(), PacketError> {
        self.0 = self
            .0
            .checked_add(amount as u64)
            .filter(|v| *v <= MAX_PACKET_BYTES)
            .ok_or(PacketError::TooLarge)?;
        Ok(())
    }
    fn text(&mut self, value: &str) -> Result<(), PacketError> {
        require(!value.is_empty() && !value.contains('\0'), "name/id")?;
        if value.len() > MAX_STRING_BYTES {
            return Err(PacketError::Limit("string bytes"));
        }
        self.add(4 + value.len())
    }
    fn items(
        &mut self,
        length: usize,
        max: usize,
        stride: usize,
        field: &'static str,
    ) -> Result<(), PacketError> {
        if length > max {
            return Err(PacketError::Limit(field));
        }
        self.add(length.checked_mul(stride).ok_or(PacketError::TooLarge)?)
    }
}

/// A borrow can cross a downstream writer boundary only after complete packet validation.
pub struct ValidatedPacket<'a>(&'a ScenePacket);
impl<'a> ValidatedPacket<'a> {
    pub fn packet(&self) -> &'a ScenePacket {
        self.0
    }
}

impl ScenePacket {
    pub fn validate(&self) -> Result<ValidatedPacket<'_>, PacketError> {
        let mut size = Size(68); // magic, five numbers, resolution and three array counts
        size.text(&self.scene_name)?;
        finite(
            &[
                self.reference_frame,
                self.frame_start,
                self.frame_end,
                self.frames_per_second,
                self.unit_scale,
            ],
            "scene numbers",
        )?;
        require(
            self.frame_start <= self.frame_end
                && self.frames_per_second > 0.0
                && self.unit_scale > 0.0,
            "scene range/units",
        )?;
        require(
            self.render_size
                .iter()
                .all(|n| (1..=MAX_IMAGE_EDGE).contains(n)),
            "render size",
        )?;
        size.items(self.groups.len(), MAX_GROUPS, 0, "groups")?;
        size.items(self.images.len(), 4096, 0, "images")?;
        size.items(self.cameras.len(), MAX_TICKS, 0, "camera ticks")?;
        let mut group_ids = BTreeSet::new();
        for group in &self.groups {
            size.text(&group.source_id)?;
            size.text(&group.name)?;
            size.add(140)?;
            affine(&group.reference_object_world, "reference object world")?;
            require(
                group_ids.insert(group.source_id.as_str()),
                "duplicate group id",
            )?;
            size.items(group.bones.len(), MAX_GROUP_BONES, 0, "bones")?;
            size.items(group.poses.len(), MAX_TICKS, 0, "pose ticks")?;
            let mut bone_ids = BTreeSet::new();
            for (index, bone) in group.bones.iter().enumerate() {
                size.text(&bone.source_id)?;
                size.text(&bone.name)?;
                size.add(180)?;
                require(
                    bone_ids.insert(bone.source_id.as_str()),
                    "duplicate bone id",
                )?;
                require(
                    bone.parent.is_none_or(|p| (p as usize) < index),
                    "bone parent/order",
                )?;
                affine(&bone.rest_world, "bone rest")?;
                finite(&bone.head_world, "bone head")?;
                finite(&bone.tail_world, "bone tail")?;
            }
            let mut vertices = 0_usize;
            let mut mesh_ids = BTreeSet::new();
            for mesh in &group.meshes {
                size.text(&mesh.source_id)?;
                size.text(&mesh.name)?;
                size.add(144)?;
                require(
                    mesh_ids.insert(mesh.source_id.as_str()),
                    "duplicate mesh id",
                )?;
                affine(&mesh.world_from_mesh, "mesh world")?;
                vertices = vertices
                    .checked_add(mesh.vertices.len())
                    .ok_or(PacketError::Limit("vertices"))?;
                if vertices > MAX_GROUP_VERTICES {
                    return Err(PacketError::Limit("vertices"));
                }
                size.items(mesh.vertices.len(), MAX_GROUP_VERTICES, 68, "vertices")?;
                size.items(mesh.triangles.len(), MAX_ITEMS, 16, "triangles")?;
                for vertex in &mesh.vertices {
                    finite(&vertex.position, "vertex position")?;
                    finite(&vertex.normal, "normal")?;
                    finite(&vertex.uv, "uv")?;
                    require(vertex.normal.iter().any(|v| *v != 0.0), "zero normal")?;
                    size.items(vertex.weights.len(), 4, 12, "positive weights")?;
                    let mut indices = BTreeSet::new();
                    let mut sum = 0.0;
                    for weight in &vertex.weights {
                        require(
                            (weight.bone as usize) < group.bones.len()
                                && weight.weight.is_finite()
                                && weight.weight > 0.0
                                && indices.insert(weight.bone),
                            "skin weight",
                        )?;
                        sum += weight.weight;
                    }
                    require(sum.is_finite(), "weight sum")?;
                }
                for triangle in &mesh.triangles {
                    require(
                        triangle
                            .vertices
                            .iter()
                            .all(|i| (*i as usize) < mesh.vertices.len())
                            && (triangle.material as usize) < mesh.materials.len(),
                        "triangle index",
                    )?;
                }
                for material in &mesh.materials {
                    size.text(&material.name)?;
                    size.add(36)?;
                    finite(&material.diffuse, "material diffuse")?;
                    require(
                        material.diffuse.iter().all(|v| (0.0..=1.0).contains(v))
                            && material
                                .image
                                .is_none_or(|i| (i as usize) < self.images.len()),
                        "material/image",
                    )?;
                }
                let mut morph_ids = BTreeSet::new();
                for morph in &mesh.morphs {
                    size.text(&morph.source_id)?;
                    size.text(&morph.name)?;
                    size.add(4)?;
                    require(
                        morph_ids.insert(morph.source_id.as_str()),
                        "duplicate morph id",
                    )?;
                    size.items(
                        morph.offsets.len(),
                        mesh.vertices.len(),
                        28,
                        "morph offsets",
                    )?;
                    let mut indices = BTreeSet::new();
                    for offset in &morph.offsets {
                        require(
                            (offset.vertex as usize) < mesh.vertices.len()
                                && indices.insert(offset.vertex),
                            "morph vertex",
                        )?;
                        finite(&offset.delta, "morph delta")?;
                    }
                }
            }
            let mut previous = None;
            for (index, pose) in group.poses.iter().enumerate() {
                size.add(148)?;
                sample_time(pose.tick, pose.source_frame, index, &mut previous)?;
                affine(&pose.object_world, "object pose")?;
                require(
                    pose.bone_skin_world.len() == group.bones.len(),
                    "pose bone count",
                )?;
                size.items(
                    pose.bone_skin_world.len(),
                    MAX_GROUP_BONES,
                    128,
                    "pose bones",
                )?;
                for matrix in &pose.bone_skin_world {
                    affine(matrix, "world skin pose")?;
                }
                size.items(pose.morph_weights.len(), MAX_ITEMS, 16, "morph weights")?;
                let mut indices = BTreeSet::new();
                for weight in &pose.morph_weights {
                    require(
                        weight.weight.is_finite()
                            && indices.insert((weight.mesh, weight.morph))
                            && group
                                .meshes
                                .get(weight.mesh as usize)
                                .is_some_and(|mesh| (weight.morph as usize) < mesh.morphs.len()),
                        "pose morph index/weight",
                    )?;
                }
            }
        }
        let mut image_ids = BTreeSet::new();
        for image in &self.images {
            size.text(&image.source_id)?;
            size.text(&image.name)?;
            size.add(12)?;
            size.add(image.png.len())?;
            require(
                image_ids.insert(image.source_id.as_str()),
                "duplicate image id",
            )?;
            require(
                (1..=MAX_IMAGE_EDGE).contains(&image.width)
                    && (1..=MAX_IMAGE_EDGE).contains(&image.height),
                "image dimensions",
            )?;
            // This validates framing/dimensions; pixel decoding and CRC/readback remain writer gates.
            require(
                image.png.len() >= 33
                    && &image.png[..8] == b"\x89PNG\r\n\x1a\n"
                    && image.png[8..12] == 13_u32.to_be_bytes()
                    && &image.png[12..16] == b"IHDR"
                    && image.png[16..20] == image.width.to_be_bytes()
                    && image.png[20..24] == image.height.to_be_bytes(),
                "PNG header/dimensions",
            )?;
        }
        let mut previous = None;
        for (index, camera) in self.cameras.iter().enumerate() {
            size.text(&camera.source_id)?;
            size.text(&camera.name)?;
            size.add(350)?;
            sample_time(camera.tick, camera.source_frame, index, &mut previous)?;
            affine(&camera.world_from_camera, "camera world")?;
            finite(&camera.projection_matrix, "camera projection")?;
            finite(&camera.shift, "camera shift")?;
            require(
                [
                    camera.lens_mm,
                    camera.sensor_mm[0],
                    camera.sensor_mm[1],
                    camera.ortho_scale,
                    camera.clip[0],
                    camera.clip[1],
                    camera.pixel_aspect[0],
                    camera.pixel_aspect[1],
                ]
                .iter()
                .all(|v| v.is_finite() && *v > 0.0)
                    && camera.clip[0] < camera.clip[1],
                "camera optics",
            )?;
        }
        Ok(ValidatedPacket(self))
    }
}

fn sample_time(
    tick: u32,
    frame: f64,
    index: usize,
    previous: &mut Option<f64>,
) -> Result<(), PacketError> {
    require(
        tick as usize == index && frame.is_finite() && previous.is_none_or(|p| p < frame),
        "sample time/tick",
    )?;
    *previous = Some(frame);
    Ok(())
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], PacketError> {
        let end = self
            .offset
            .checked_add(n)
            .filter(|end| *end <= self.bytes.len())
            .ok_or(PacketError::Truncated)?;
        let value = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(value)
    }
    fn u32(&mut self) -> Result<u32, PacketError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn number(&mut self) -> Result<f64, PacketError> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn numbers<const N: usize>(&mut self) -> Result<[f64; N], PacketError> {
        let mut values = [0.0; N];
        for value in &mut values {
            *value = self.number()?;
        }
        Ok(values)
    }
    fn optional(&mut self) -> Result<Option<u32>, PacketError> {
        let value = self.u32()? as i32;
        if value == -1 {
            Ok(None)
        } else if value >= 0 {
            Ok(Some(value as u32))
        } else {
            Err(PacketError::Invalid("optional index"))
        }
    }
    fn count(&mut self, max: usize, stride: usize) -> Result<usize, PacketError> {
        let count = self.u32()? as usize;
        if count > max {
            return Err(PacketError::Limit("wire count"));
        }
        if count
            .checked_mul(stride)
            .is_none_or(|n| n > self.bytes.len() - self.offset)
        {
            return Err(PacketError::Truncated);
        }
        Ok(count)
    }
    fn list<T>(
        &mut self,
        max: usize,
        stride: usize,
        mut read: impl FnMut(&mut Self) -> Result<T, PacketError>,
    ) -> Result<Vec<T>, PacketError> {
        let count = self.count(max, stride)?;
        let mut result = Vec::new();
        result
            .try_reserve_exact(count)
            .map_err(|_| PacketError::Allocation)?;
        for _ in 0..count {
            result.push(read(self)?);
        }
        Ok(result)
    }
    fn string(&mut self) -> Result<String, PacketError> {
        let n = self.count(MAX_STRING_BYTES, 1)?;
        let text = std::str::from_utf8(self.take(n)?).map_err(|_| PacketError::Invalid("UTF-8"))?;
        let mut value = String::new();
        value
            .try_reserve_exact(n)
            .map_err(|_| PacketError::Allocation)?;
        value.push_str(text);
        Ok(value)
    }
    fn mesh(&mut self, remaining_vertices: usize) -> Result<MeshPacket, PacketError> {
        Ok(MeshPacket {
            source_id: self.string()?,
            name: self.string()?,
            world_from_mesh: self.numbers()?,
            vertices: self.list(remaining_vertices, 68, |r| {
                Ok(Vertex {
                    position: r.numbers()?,
                    normal: r.numbers()?,
                    uv: r.numbers()?,
                    weights: r.list(4, 12, |r| {
                        Ok(BoneWeight {
                            bone: r.u32()?,
                            weight: r.number()?,
                        })
                    })?,
                })
            })?,
            triangles: self.list(MAX_ITEMS, 16, |r| {
                Ok(Triangle {
                    vertices: [r.u32()?, r.u32()?, r.u32()?],
                    material: r.u32()?,
                })
            })?,
            materials: self.list(MAX_ITEMS, 40, |r| {
                Ok(MaterialPacket {
                    name: r.string()?,
                    diffuse: r.numbers()?,
                    image: r.optional()?,
                })
            })?,
            morphs: self.list(MAX_ITEMS, 12, |r| {
                Ok(VertexMorph {
                    source_id: r.string()?,
                    name: r.string()?,
                    offsets: r.list(MAX_GROUP_VERTICES, 28, |r| {
                        Ok(MorphOffset {
                            vertex: r.u32()?,
                            delta: r.numbers()?,
                        })
                    })?,
                })
            })?,
        })
    }
    fn group(&mut self) -> Result<ModelPacket, PacketError> {
        let source_id = self.string()?;
        let name = self.string()?;
        let reference_object_world = self.numbers()?;
        let mut remaining = MAX_GROUP_VERTICES;
        let meshes = self.list(MAX_ITEMS, 152, |r| {
            let mesh = r.mesh(remaining)?;
            remaining -= mesh.vertices.len();
            Ok(mesh)
        })?;
        let bones = self.list(MAX_GROUP_BONES, 188, |r| {
            Ok(BonePacket {
                source_id: r.string()?,
                name: r.string()?,
                parent: r.optional()?,
                rest_world: r.numbers()?,
                head_world: r.numbers()?,
                tail_world: r.numbers()?,
            })
        })?;
        let poses = self.list(MAX_TICKS, 148, |r| {
            Ok(PoseSample {
                tick: r.u32()?,
                source_frame: r.number()?,
                object_world: r.numbers()?,
                bone_skin_world: r.list(bones.len(), 128, |r| r.numbers())?,
                morph_weights: r.list(MAX_ITEMS, 16, |r| {
                    Ok(MorphWeight {
                        mesh: r.u32()?,
                        morph: r.u32()?,
                        weight: r.number()?,
                    })
                })?,
            })
        })?;
        Ok(ModelPacket {
            source_id,
            name,
            reference_object_world,
            meshes,
            bones,
            poses,
        })
    }
    fn camera(&mut self) -> Result<CameraSample, PacketError> {
        Ok(CameraSample {
            tick: self.u32()?,
            source_frame: self.number()?,
            source_id: self.string()?,
            name: self.string()?,
            projection: match self.take(1)?[0] {
                0 => Projection::Perspective,
                1 => Projection::Orthographic,
                _ => return Err(PacketError::Invalid("camera projection tag")),
            },
            sensor_fit: match self.take(1)?[0] {
                0 => SensorFit::Auto,
                1 => SensorFit::Horizontal,
                2 => SensorFit::Vertical,
                _ => return Err(PacketError::Invalid("camera sensor-fit tag")),
            },
            lens_mm: self.number()?,
            sensor_mm: self.numbers()?,
            shift: self.numbers()?,
            ortho_scale: self.number()?,
            clip: self.numbers()?,
            pixel_aspect: self.numbers()?,
            world_from_camera: self.numbers()?,
            projection_matrix: self.numbers()?,
        })
    }
}

/// Counts are checked against the remaining wire bytes and limits before each allocation.
pub fn decode(bytes: &[u8]) -> Result<ScenePacket, PacketError> {
    if bytes.len() as u64 > MAX_PACKET_BYTES {
        return Err(PacketError::TooLarge);
    }
    let mut r = Reader { bytes, offset: 0 };
    if r.take(8)? != b"NEXASCN1" {
        return Err(PacketError::InvalidMagic);
    }
    let packet = ScenePacket {
        scene_name: r.string()?,
        reference_frame: r.number()?,
        frame_start: r.number()?,
        frame_end: r.number()?,
        frames_per_second: r.number()?,
        unit_scale: r.number()?,
        render_size: [r.u32()?, r.u32()?],
        groups: r.list(MAX_GROUPS, 148, Reader::group)?,
        images: r.list(4096, 20, |r| {
            Ok(ImagePacket {
                source_id: r.string()?,
                name: r.string()?,
                width: r.u32()?,
                height: r.u32()?,
                png: {
                    let n = r.count(MAX_PACKET_BYTES as usize, 1)?;
                    r.take(n)?.to_vec()
                },
            })
        })?,
        cameras: r.list(MAX_TICKS, 358, Reader::camera)?,
    };
    if r.offset != bytes.len() {
        return Err(PacketError::TrailingBytes);
    }
    packet.validate()?;
    Ok(packet)
}

pub fn read_packet(path: &Path) -> Result<ScenePacket, PacketError> {
    let file = File::open(path).map_err(|e| PacketError::Io(e.to_string()))?;
    if file
        .metadata()
        .map_err(|e| PacketError::Io(e.to_string()))?
        .len()
        > MAX_PACKET_BYTES
    {
        return Err(PacketError::TooLarge);
    }
    let mut bytes = Vec::new();
    file.take(MAX_PACKET_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| PacketError::Io(e.to_string()))?;
    decode(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    const IDENTITY: Matrix = [
        1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
    ];

    fn fixture() -> ScenePacket {
        ScenePacket {
            scene_name: "Scene".into(),
            reference_frame: 1.,
            frame_start: 1.,
            frame_end: 2.,
            frames_per_second: 30.,
            unit_scale: 1.,
            render_size: [1920, 1080],
            images: vec![],
            cameras: vec![],
            groups: vec![ModelPacket {
                source_id: "rig".into(),
                name: "Rig".into(),
                poses: vec![PoseSample {
                    tick: 0,
                    source_frame: 1.,
                    object_world: IDENTITY,
                    bone_skin_world: vec![IDENTITY],
                    morph_weights: vec![MorphWeight {
                        mesh: 0,
                        morph: 0,
                        weight: 0.5,
                    }],
                }],
                bones: vec![BonePacket {
                    source_id: "bone".into(),
                    name: "Bone".into(),
                    parent: None,
                    rest_world: IDENTITY,
                    head_world: [0.; 3],
                    tail_world: [0., 1., 0.],
                }],
                reference_object_world: IDENTITY,
                meshes: vec![MeshPacket {
                    source_id: "mesh".into(),
                    name: "Mesh".into(),
                    world_from_mesh: IDENTITY,
                    vertices: vec![
                        Vertex {
                            position: [0.; 3],
                            normal: [0., 0., 1.],
                            uv: [0.; 2],
                            weights: vec![BoneWeight {
                                bone: 0,
                                weight: 1.
                            }]
                        };
                        3
                    ],
                    triangles: vec![Triangle {
                        vertices: [0, 1, 2],
                        material: 0,
                    }],
                    materials: vec![MaterialPacket {
                        name: "Diffuse".into(),
                        diffuse: [1.; 4],
                        image: None,
                    }],
                    morphs: vec![VertexMorph {
                        source_id: "key".into(),
                        name: "Key".into(),
                        offsets: vec![MorphOffset {
                            vertex: 1,
                            delta: [0., 0.1, 0.],
                        }],
                    }],
                }],
            }],
        }
    }

    #[test]
    fn normal_scene_packet_and_wire_header() {
        let packet = fixture();
        let checked = packet.validate().unwrap();
        assert_eq!(
            checked.packet().groups[0].meshes[0].morphs[0].offsets[0].vertex,
            1
        );
        // Independently construct the smallest complete wire packet.
        let mut wire = b"NEXASCN1".to_vec();
        wire.extend(5_u32.to_le_bytes());
        wire.extend(b"Scene");
        for number in [1_f64, 1., 2., 30., 1.] {
            wire.extend(number.to_le_bytes());
        }
        for number in [1920_u32, 1080, 0, 0, 0] {
            wire.extend(number.to_le_bytes());
        }
        assert_eq!(decode(&wire).unwrap().scene_name, "Scene");
        wire.push(0);
        assert!(matches!(decode(&wire), Err(PacketError::TrailingBytes)));
    }

    #[test]
    fn malformed_indices_numbers_and_counts_are_rejected() {
        let mut packet = fixture();
        packet.groups[0].meshes[0].triangles[0].vertices[2] = 3;
        assert!(matches!(
            packet.validate(),
            Err(PacketError::Invalid("triangle index"))
        ));
        let mut packet = fixture();
        packet.groups[0].meshes[0].vertices[0].position[0] = f64::NAN;
        assert!(matches!(
            packet.validate(),
            Err(PacketError::Invalid("vertex position"))
        ));
        let mut packet = fixture();
        packet.groups[0].bones[0].parent = Some(0);
        assert!(matches!(
            packet.validate(),
            Err(PacketError::Invalid("bone parent/order"))
        ));
        let mut packet = fixture();
        packet.groups = vec![packet.groups[0].clone(); MAX_GROUPS + 1];
        assert!(matches!(
            packet.validate(),
            Err(PacketError::Limit("groups"))
        ));
        let mut wire = b"NEXASCN1".to_vec();
        wire.extend(u32::MAX.to_le_bytes());
        assert!(matches!(
            decode(&wire),
            Err(PacketError::Limit("wire count"))
        ));
        assert!(matches!(decode(b"NEXASCN1"), Err(PacketError::Truncated)));
        let mut reader = Reader {
            bytes: &u32::MAX.to_le_bytes(),
            offset: 0,
        };
        assert!(matches!(
            reader.list::<u32>(usize::MAX, usize::MAX, |r| r.u32()),
            Err(PacketError::Truncated)
        ));
    }
}
