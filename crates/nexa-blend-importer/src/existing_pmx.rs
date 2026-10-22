//! Read-only, bounded PMX 2.0 inspection for animation-only target matching.

use crate::names::{decode_cp932, encode_cp932};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    fs::{File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
};

const MAX_PMX_BYTES: u64 = 512 * 1024 * 1024;
const MAX_STRING_BYTES: usize = 1024 * 1024;
const MAX_VERTICES: usize = 4_000_000;
const MAX_INDICES: usize = MAX_VERTICES * 3;
const MAX_RECORDS: usize = 65_535;
const REST_TOLERANCE: f64 = 1e-4;
const MORPH_TOLERANCE: f64 = 1e-6;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExistingMatch {
    pub canonical_path: PathBuf,
    /// Volume/file ID on Windows; aliases and hardlinks have the same identity.
    pub file_identity: String,
    /// Strict CP932, 1..=19 bytes, leaving a terminator in the VMD model field.
    pub model_name_cp932: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PmxMatchError {
    Io(String),
    TooLarge,
    Truncated,
    Invalid(&'static str),
    Limit(&'static str),
    Allocation,
    TrailingBytes,
    Unsupported(&'static str),
    Name { field: &'static str, reason: String },
    NamesMismatch,
    RestMismatch(&'static str),
    MorphMismatch(&'static str),
    ChangedDuringRead,
}

impl PmxMatchError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Io(_) => "existing_pmx_io",
            Self::TooLarge => "existing_pmx_too_large",
            Self::Truncated => "existing_pmx_truncated",
            Self::Invalid(_) => "existing_pmx_invalid",
            Self::Limit(_) => "existing_pmx_limit",
            Self::Allocation => "existing_pmx_allocation",
            Self::TrailingBytes => "existing_pmx_trailing_bytes",
            Self::Unsupported(_) => "existing_pmx_unsupported",
            Self::Name { .. } => "existing_pmx_name",
            Self::NamesMismatch => "existing_pmx_names_mismatch",
            Self::RestMismatch(_) => "existing_pmx_rest_mismatch",
            Self::MorphMismatch(_) => "existing_pmx_morph_mismatch",
            Self::ChangedDuringRead => "existing_pmx_changed",
        }
    }
}

impl fmt::Display for PmxMatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(reason) => write!(f, "cannot inspect existing PMX: {reason}"),
            Self::TooLarge => f.write_str("existing PMX exceeds 512 MiB"),
            Self::Truncated => f.write_str("truncated existing PMX"),
            Self::Invalid(field) => write!(f, "invalid PMX {field}"),
            Self::Limit(field) => write!(f, "PMX {field} exceeds the inspection limit"),
            Self::Allocation => f.write_str("PMX inspection allocation failed"),
            Self::TrailingBytes => f.write_str("unexpected bytes after PMX 2.0 joints"),
            Self::Unsupported(feature) => write!(f, "existing PMX uses unsupported {feature}"),
            Self::Name { field, reason } => write!(f, "invalid PMX {field}: {reason}"),
            Self::NamesMismatch => f.write_str("existing PMX bone or morph aliases do not match"),
            Self::RestMismatch(field) => write!(f, "existing PMX rest mismatch: {field}"),
            Self::MorphMismatch(field) => write!(f, "existing PMX morph mismatch: {field}"),
            Self::ChangedDuringRead => f.write_str("existing PMX changed during inspection"),
        }
    }
}
impl std::error::Error for PmxMatchError {}

fn io_error(error: std::io::Error) -> PmxMatchError {
    PmxMatchError::Io(error.to_string())
}

#[derive(Debug)]
struct Alias {
    bytes: Vec<u8>,
}

fn cp932(value: &str, limit: usize, field: &'static str) -> Result<Vec<u8>, PmxMatchError> {
    let fail = |reason: String| PmxMatchError::Name { field, reason };
    let bytes = encode_cp932(value).map_err(|e| fail(e.to_string()))?;
    if bytes.is_empty() || bytes.len() > limit || bytes.contains(&0) {
        return Err(fail(format!(
            "requires 1..={limit} CP932 bytes without NUL"
        )));
    }
    // CP932 has Unicode aliases. A successful conversion must also round-trip
    // exactly, so VMD decoding names the actual PMX entry.
    if decode_cp932(&bytes).map_err(|e| fail(e.to_string()))? != value {
        return Err(fail(
            "name does not round-trip exactly through CP932".into(),
        ));
    }
    Ok(bytes)
}

#[derive(Debug)]
enum Tail {
    Bone(i32),
    Offset([f32; 3]),
}

#[derive(Debug)]
struct Bone {
    name: Alias,
    position: [f32; 3],
    parent: i32,
    layer: i32,
    flags: u16,
    tail: Tail,
    axes: Option<[[f32; 3]; 2]>,
}

#[derive(Debug)]
struct Morph {
    name: Alias,
    kind: u8,
    offsets: Vec<(u32, [f32; 3])>,
}

#[derive(Debug)]
struct Inspection {
    model_name: String,
    vertices: Vec<[f32; 3]>,
    indices: Vec<u32>,
    bones: Vec<Bone>,
    morphs: Vec<Morph>,
    unsupported: Option<&'static str>,
}

#[derive(Clone, Copy)]
struct Widths {
    vertex: usize,
    texture: usize,
    material: usize,
    bone: usize,
    morph: usize,
    rigid: usize,
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
    encoding: u8,
    uv: usize,
    widths: Widths,
}

fn allocated<T>(count: usize) -> Result<Vec<T>, PmxMatchError> {
    let mut out = Vec::new();
    out.try_reserve_exact(count)
        .map_err(|_| PmxMatchError::Allocation)?;
    Ok(out)
}

impl Reader<'_> {
    fn take(&mut self, count: usize) -> Result<&[u8], PmxMatchError> {
        let end = self
            .offset
            .checked_add(count)
            .ok_or(PmxMatchError::Truncated)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(PmxMatchError::Truncated)?;
        self.offset = end;
        Ok(value)
    }
    fn byte(&mut self) -> Result<u8, PmxMatchError> {
        Ok(self.take(1)?[0])
    }
    fn i32(&mut self) -> Result<i32, PmxMatchError> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u16(&mut self) -> Result<u16, PmxMatchError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn number(&mut self) -> Result<f32, PmxMatchError> {
        let value = f32::from_le_bytes(self.take(4)?.try_into().unwrap());
        if !value.is_finite() {
            return Err(PmxMatchError::Invalid("non-finite number"));
        }
        Ok(value)
    }
    fn numbers<const N: usize>(&mut self) -> Result<[f32; N], PmxMatchError> {
        let mut values = [0.; N];
        for value in &mut values {
            *value = self.number()?;
        }
        Ok(values)
    }
    fn floats(&mut self, count: usize) -> Result<(), PmxMatchError> {
        for _ in 0..count {
            self.number()?;
        }
        Ok(())
    }
    fn count(
        &mut self,
        max: usize,
        minimum: usize,
        field: &'static str,
    ) -> Result<usize, PmxMatchError> {
        let count = usize::try_from(self.i32()?).map_err(|_| PmxMatchError::Invalid(field))?;
        if count > max {
            return Err(PmxMatchError::Limit(field));
        }
        let size = count.checked_mul(minimum).ok_or(PmxMatchError::Truncated)?;
        if size > self.bytes.len() - self.offset {
            return Err(PmxMatchError::Truncated);
        }
        Ok(count)
    }
    fn string(&mut self) -> Result<String, PmxMatchError> {
        let count = self.count(MAX_STRING_BYTES, 1, "text byte length")?;
        let encoding = self.encoding;
        let bytes = self.take(count)?;
        match encoding {
            0 => {
                if count % 2 != 0 {
                    return Err(PmxMatchError::Invalid("UTF-16LE byte length"));
                }
                let mut units = allocated(count / 2)?;
                units.extend(
                    bytes
                        .chunks_exact(2)
                        .map(|b| u16::from_le_bytes([b[0], b[1]])),
                );
                String::from_utf16(&units).map_err(|_| PmxMatchError::Invalid("UTF-16LE text"))
            }
            1 => {
                let value =
                    std::str::from_utf8(bytes).map_err(|_| PmxMatchError::Invalid("UTF-8 text"))?;
                let mut text = String::new();
                text.try_reserve_exact(bytes.len())
                    .map_err(|_| PmxMatchError::Allocation)?;
                text.push_str(value);
                Ok(text)
            }
            _ => Err(PmxMatchError::Invalid("text encoding")),
        }
    }
    fn names(&mut self) -> Result<(), PmxMatchError> {
        self.string()?;
        self.string()?;
        Ok(())
    }
    fn alias(
        &mut self,
        used: &mut BTreeSet<Vec<u8>>,
        field: &'static str,
    ) -> Result<Alias, PmxMatchError> {
        let text = self.string()?;
        self.string()?;
        let bytes = cp932(&text, 14, field)?;
        if !used.insert(bytes.clone()) {
            return Err(PmxMatchError::Invalid(
                "duplicate or ambiguous bone/morph alias",
            ));
        }
        Ok(Alias { bytes })
    }
    fn signed(&mut self, width: usize) -> Result<i32, PmxMatchError> {
        Ok(match width {
            1 => i32::from(self.byte()? as i8),
            2 => i32::from(i16::from_le_bytes(self.take(2)?.try_into().unwrap())),
            4 => self.i32()?,
            _ => return Err(PmxMatchError::Invalid("index width")),
        })
    }
    fn vertex(&mut self, bound: usize) -> Result<u32, PmxMatchError> {
        let value = match self.widths.vertex {
            1 => u32::from(self.byte()?),
            2 => u32::from(self.u16()?),
            4 => u32::from_le_bytes(self.take(4)?.try_into().unwrap()),
            _ => return Err(PmxMatchError::Invalid("vertex index width")),
        };
        if value as usize >= bound {
            return Err(PmxMatchError::Invalid("vertex index"));
        }
        Ok(value)
    }
    fn index(&mut self, width: usize, bound: usize, optional: bool) -> Result<i32, PmxMatchError> {
        let value = self.signed(width)?;
        check_index(value, bound, optional)?;
        Ok(value)
    }
}

fn check_index(value: i32, bound: usize, optional: bool) -> Result<(), PmxMatchError> {
    if (optional && value == -1) || (value >= 0 && (value as usize) < bound) {
        Ok(())
    } else {
        Err(PmxMatchError::Invalid("reference index"))
    }
}

fn inspect(bytes: &[u8]) -> Result<Inspection, PmxMatchError> {
    if bytes.len() as u64 > MAX_PMX_BYTES {
        return Err(PmxMatchError::TooLarge);
    }
    let mut r = Reader {
        bytes,
        offset: 0,
        encoding: 0,
        uv: 0,
        widths: Widths {
            vertex: 1,
            texture: 1,
            material: 1,
            bone: 1,
            morph: 1,
            rigid: 1,
        },
    };
    if r.take(4)? != b"PMX " || r.number()? != 2.0 {
        return Err(PmxMatchError::Invalid("PMX 2.0 header"));
    }
    if r.byte()? != 8 {
        return Err(PmxMatchError::Invalid("global header length"));
    }
    r.encoding = r.byte()?;
    r.uv = usize::from(r.byte()?);
    if r.encoding > 1 || r.uv > 4 {
        return Err(PmxMatchError::Invalid("encoding/additional UV"));
    }
    let mut widths = [0; 6];
    for width in &mut widths {
        *width = usize::from(r.byte()?);
        if ![1, 2, 4].contains(width) {
            return Err(PmxMatchError::Invalid("index width"));
        }
    }
    r.widths = Widths {
        vertex: widths[0],
        texture: widths[1],
        material: widths[2],
        bone: widths[3],
        morph: widths[4],
        rigid: widths[5],
    };
    let w = r.widths;
    let model_name = r.string()?;
    for _ in 0..3 {
        r.string()?;
    }
    let vertex_count = r.count(MAX_VERTICES, 37 + w.bone + r.uv * 16, "vertices")?;
    let mut vertices = allocated(vertex_count)?;
    let mut largest_skin_bone = None;
    for _ in 0..vertex_count {
        vertices.push(r.numbers()?);
        r.floats(5 + r.uv * 4)?;
        let deform = r.byte()?;
        let slots = match deform {
            0 => 1,
            1 | 3 => 2,
            2 => 4,
            _ => return Err(PmxMatchError::Invalid("PMX 2.0 deformation type")),
        };
        let mut bone_indices = [-1; 4];
        for index in &mut bone_indices[..slots] {
            *index = r.signed(w.bone)?;
            if *index < -1 {
                return Err(PmxMatchError::Invalid("skin bone index"));
            }
            if *index >= 0 {
                largest_skin_bone =
                    Some(largest_skin_bone.map_or(*index, |largest: i32| largest.max(*index)));
            }
        }
        let weights = match deform {
            0 => [1., 0., 0., 0.],
            1 | 3 => {
                let first = r.number()?;
                [first, 1. - first, 0., 0.]
            }
            2 => r.numbers()?,
            _ => unreachable!(),
        };
        if weights.iter().any(|v| !(0.0..=1.0).contains(v))
            || (weights.iter().map(|v| f64::from(*v)).sum::<f64>() - 1.).abs() > 1e-4
            || bone_indices
                .iter()
                .zip(weights)
                .any(|(index, weight)| *index < 0 && weight > 0.)
        {
            return Err(PmxMatchError::Invalid("skin weights"));
        }
        if deform == 3 {
            r.floats(9)?;
        }
        r.number()?;
    }
    let index_count = r.count(MAX_INDICES, w.vertex, "face indices")?;
    if index_count % 3 != 0 {
        return Err(PmxMatchError::Invalid("triangle index count"));
    }
    let mut indices = allocated(index_count)?;
    for _ in 0..index_count {
        indices.push(r.vertex(vertex_count)?);
    }
    let texture_count = r.count(MAX_RECORDS, 4, "textures")?;
    for _ in 0..texture_count {
        r.string()?;
    }
    let material_count = r.count(MAX_RECORDS, 84 + 2 * w.texture, "materials")?;
    let mut surface_total = 0_usize;
    for _ in 0..material_count {
        r.names()?;
        r.floats(11)?;
        r.byte()?;
        r.floats(5)?;
        r.index(w.texture, texture_count, true)?;
        r.index(w.texture, texture_count, true)?;
        if r.byte()? > 3 {
            return Err(PmxMatchError::Invalid("sphere texture mode"));
        }
        match r.byte()? {
            0 => {
                r.index(w.texture, texture_count, true)?;
            }
            1 => {
                if r.byte()? > 9 {
                    return Err(PmxMatchError::Invalid("shared toon index"));
                }
            }
            _ => return Err(PmxMatchError::Invalid("shared toon tag")),
        }
        r.string()?;
        let count = usize::try_from(r.i32()?)
            .map_err(|_| PmxMatchError::Invalid("material surface count"))?;
        if count % 3 != 0 {
            return Err(PmxMatchError::Invalid("material surface count"));
        }
        surface_total = surface_total
            .checked_add(count)
            .ok_or(PmxMatchError::Invalid("material surface count"))?;
        if surface_total > index_count {
            return Err(PmxMatchError::Invalid("material surface count"));
        }
    }
    if surface_total != index_count {
        return Err(PmxMatchError::Invalid("material surface coverage"));
    }
    let bone_count = r.count(MAX_RECORDS, 26 + 2 * w.bone, "bones")?;
    if bone_count == 0 {
        return Err(PmxMatchError::Invalid("missing model root"));
    }
    if let Some(index) = largest_skin_bone {
        check_index(index, bone_count, false)?;
    }
    let mut used = BTreeSet::new();
    let mut bones = allocated(bone_count)?;
    let mut unsupported = None;
    for _ in 0..bone_count {
        let name = r.alias(&mut used, "bone alias")?;
        let position = r.numbers()?;
        let parent = r.index(w.bone, bone_count, true)?;
        let layer = r.i32()?;
        if layer < 0 {
            return Err(PmxMatchError::Invalid("bone layer"));
        }
        let flags = r.u16()?;
        let tail = if flags & 1 != 0 {
            Tail::Bone(r.index(w.bone, bone_count, true)?)
        } else {
            Tail::Offset(r.numbers()?)
        };
        if flags & 0x0300 != 0 {
            r.index(w.bone, bone_count, false)?;
            r.number()?;
            unsupported = Some("bone grant");
        }
        if flags & 0x0400 != 0 {
            r.floats(3)?;
            unsupported = Some("fixed bone axis");
        }
        let axes = if flags & 0x0800 != 0 {
            Some([r.numbers()?, r.numbers()?])
        } else {
            None
        };
        if flags & 0x2000 != 0 {
            r.i32()?;
            unsupported = Some("external parent transform");
        }
        if flags & 0x0020 != 0 {
            r.index(w.bone, bone_count, false)?;
            if r.i32()? < 0 || r.number()? < 0. {
                return Err(PmxMatchError::Invalid("IK iterations/angle"));
            }
            let count = r.count(MAX_RECORDS, w.bone + 1, "IK links")?;
            for _ in 0..count {
                r.index(w.bone, bone_count, false)?;
                match r.byte()? {
                    0 => {}
                    1 => r.floats(6)?,
                    _ => return Err(PmxMatchError::Invalid("IK angle limit tag")),
                }
            }
            unsupported = Some("inverse kinematics");
        }
        if flags & 0x1000 != 0 {
            unsupported = Some("bone operation after physics");
        }
        if flags & !0x3f3f != 0 {
            unsupported = Some("unknown bone flags");
        }
        bones.push(Bone {
            name,
            position,
            parent,
            layer,
            flags,
            tail,
            axes,
        });
    }
    validate_hierarchy(&bones)?;
    let morph_count = r.count(MAX_RECORDS, 14, "morphs")?;
    let mut morphs = allocated(morph_count)?;
    let mut total_offsets = 0_usize;
    for _ in 0..morph_count {
        let name = r.alias(&mut used, "morph alias")?;
        if r.byte()? > 4 {
            return Err(PmxMatchError::Invalid("morph panel"));
        }
        let kind = r.byte()?;
        let minimum = match kind {
            0 => w.morph + 4,
            1 => w.vertex + 12,
            2 => w.bone + 28,
            3..=7 => w.vertex + 16,
            8 => w.material + 113,
            _ => return Err(PmxMatchError::Invalid("PMX 2.0 morph type")),
        };
        let count = r.count(MAX_VERTICES - total_offsets, minimum, "morph entries")?;
        total_offsets += count;
        let mut offsets = allocated(if kind == 1 { count } else { 0 })?;
        if kind != 1 {
            unsupported = Some("non-vertex morph semantics");
        }
        for _ in 0..count {
            match kind {
                0 => {
                    r.index(w.morph, morph_count, false)?;
                    r.number()?;
                }
                1 => {
                    let vertex = r.vertex(vertex_count)?;
                    offsets.push((vertex, r.numbers()?));
                }
                2 => {
                    r.index(w.bone, bone_count, false)?;
                    r.floats(7)?;
                }
                3..=7 => {
                    if kind > 3 && usize::from(kind - 3) > r.uv {
                        return Err(PmxMatchError::Invalid("morph additional UV channel"));
                    }
                    r.vertex(vertex_count)?;
                    r.floats(4)?;
                }
                8 => {
                    r.index(w.material, material_count, true)?;
                    if r.byte()? > 1 {
                        return Err(PmxMatchError::Invalid("material morph operation"));
                    }
                    r.floats(28)?;
                }
                _ => unreachable!(),
            }
        }
        offsets.sort_unstable_by_key(|entry| entry.0);
        if offsets.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(PmxMatchError::Invalid("duplicate vertex morph entry"));
        }
        morphs.push(Morph {
            name,
            kind,
            offsets,
        });
    }
    let display_count = r.count(MAX_RECORDS, 13, "display frames")?;
    for _ in 0..display_count {
        r.names()?;
        if r.byte()? > 1 {
            return Err(PmxMatchError::Invalid("display special tag"));
        }
        let count = r.count(MAX_RECORDS, 1 + w.bone.min(w.morph), "display entries")?;
        for _ in 0..count {
            match r.byte()? {
                0 => {
                    r.index(w.bone, bone_count, false)?;
                }
                1 => {
                    r.index(w.morph, morph_count, false)?;
                }
                _ => return Err(PmxMatchError::Invalid("display entry tag")),
            }
        }
    }
    let rigid_count = r.count(MAX_RECORDS, 69 + w.bone, "rigid bodies")?;
    for _ in 0..rigid_count {
        r.names()?;
        r.index(w.bone, bone_count, true)?;
        if r.byte()? > 15 {
            return Err(PmxMatchError::Invalid("rigid body group"));
        }
        r.u16()?;
        if r.byte()? > 2 {
            return Err(PmxMatchError::Invalid("rigid body shape"));
        }
        r.floats(14)?;
        match r.byte()? {
            0 => {}
            1 | 2 => unsupported = Some("dynamic rigid body physics"),
            _ => return Err(PmxMatchError::Invalid("rigid body mode")),
        }
    }
    let joint_count = r.count(MAX_RECORDS, 105 + 2 * w.rigid, "joints")?;
    for _ in 0..joint_count {
        r.names()?;
        if r.byte()? != 0 {
            return Err(PmxMatchError::Invalid("PMX 2.0 joint type"));
        }
        r.index(w.rigid, rigid_count, true)?;
        r.index(w.rigid, rigid_count, true)?;
        r.floats(24)?;
        unsupported = Some("joint physics");
    }
    if r.offset != bytes.len() {
        return Err(PmxMatchError::TrailingBytes);
    }
    Ok(Inspection {
        model_name,
        vertices,
        indices,
        bones,
        morphs,
        unsupported,
    })
}

fn validate_hierarchy(bones: &[Bone]) -> Result<(), PmxMatchError> {
    let mut states = allocated(bones.len())?;
    states.resize(bones.len(), 0_u8);
    for start in 0..bones.len() {
        let mut next = Some(start);
        while let Some(index) = next {
            match states[index] {
                0 => states[index] = 1,
                1 => return Err(PmxMatchError::Invalid("cyclic bone parents")),
                _ => break,
            }
            next = usize::try_from(bones[index].parent).ok();
        }
        let mut next = Some(start);
        while let Some(index) = next {
            if states[index] != 1 {
                break;
            }
            states[index] = 2;
            next = usize::try_from(bones[index].parent).ok();
        }
    }
    Ok(())
}

fn near<const N: usize>(a: [f32; N], b: [f32; N], tolerance: f64) -> bool {
    a.into_iter()
        .zip(b)
        .all(|(a, b)| (f64::from(a) - f64::from(b)).abs() <= tolerance)
}

fn default_axes(bone: &Bone) -> bool {
    bone.axes
        .is_none_or(|axes| axes == [[1., 0., 0.], [0., 0., 1.]])
}

fn compare(actual: &Inspection, expected: &Inspection) -> Result<(), PmxMatchError> {
    for model in [actual, expected] {
        if let Some(feature) = model.unsupported {
            return Err(PmxMatchError::Unsupported(feature));
        }
    }
    if actual.bones.len() != expected.bones.len() || actual.morphs.len() != expected.morphs.len() {
        return Err(PmxMatchError::NamesMismatch);
    }
    let by_name: BTreeMap<&[u8], &Bone> = actual
        .bones
        .iter()
        .map(|bone| (bone.name.bytes.as_slice(), bone))
        .collect();
    let parent_name = |model: &Inspection, index: i32| {
        usize::try_from(index)
            .ok()
            .map(|index| model.bones[index].name.bytes.clone())
    };
    for bone in &expected.bones {
        let other = by_name
            .get(bone.name.bytes.as_slice())
            .ok_or(PmxMatchError::NamesMismatch)?;
        if parent_name(actual, other.parent) != parent_name(expected, bone.parent) {
            return Err(PmxMatchError::RestMismatch("bone parent"));
        }
        if !near(other.position, bone.position, REST_TOLERANCE) {
            return Err(PmxMatchError::RestMismatch("absolute bone position"));
        }
        // Compare display axes with the generated reference; do not interpret
        // them as a bind rotation or reject our own matching rest orientation.
        let axes_match = if default_axes(other) && default_axes(bone) {
            true
        } else {
            matches!((other.axes, bone.axes), (Some(a), Some(b))
                if near(a[0], b[0], REST_TOLERANCE) && near(a[1], b[1], REST_TOLERANCE))
        };
        if !axes_match {
            return Err(PmxMatchError::Unsupported("non-default bone local axes"));
        }
        if other.flags & !0x0800 != bone.flags & !0x0800 || other.layer != bone.layer {
            return Err(PmxMatchError::RestMismatch("bone flags/layer"));
        }
        let tail_matches = match (&other.tail, &bone.tail) {
            (Tail::Bone(a), Tail::Bone(b)) => parent_name(actual, *a) == parent_name(expected, *b),
            (Tail::Offset(a), Tail::Offset(b)) => near(*a, *b, REST_TOLERANCE),
            _ => false,
        };
        if !tail_matches {
            return Err(PmxMatchError::RestMismatch("bone tail"));
        }
    }
    if !expected.morphs.is_empty() {
        if actual.vertices.len() != expected.vertices.len()
            || actual.indices != expected.indices
            || actual
                .vertices
                .iter()
                .zip(&expected.vertices)
                .any(|(a, b)| !near(*a, *b, MORPH_TOLERANCE))
        {
            return Err(PmxMatchError::MorphMismatch(
                "vertex correspondence/topology",
            ));
        }
    }
    let by_name: BTreeMap<&[u8], &Morph> = actual
        .morphs
        .iter()
        .map(|morph| (morph.name.bytes.as_slice(), morph))
        .collect();
    for morph in &expected.morphs {
        let other = by_name
            .get(morph.name.bytes.as_slice())
            .ok_or(PmxMatchError::NamesMismatch)?;
        if other.kind != morph.kind
            || other.offsets.len() != morph.offsets.len()
            || other
                .offsets
                .iter()
                .zip(&morph.offsets)
                .any(|((ai, a), (bi, b))| ai != bi || !near(*a, *b, MORPH_TOLERANCE))
        {
            return Err(PmxMatchError::MorphMismatch("type or vertex offsets"));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn handle_identity(file: &File) -> Result<(PathBuf, String), PmxMatchError> {
    use std::{ffi::c_void, os::windows::io::AsRawHandle};
    #[repr(C)]
    struct Information {
        attributes: u32,
        created: [u32; 2],
        accessed: [u32; 2],
        written: [u32; 2],
        volume: u32,
        size_high: u32,
        size_low: u32,
        links: u32,
        index_high: u32,
        index_low: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandle(handle: *mut c_void, information: *mut c_void) -> i32;
        fn GetFinalPathNameByHandleW(
            handle: *mut c_void,
            path: *mut u16,
            length: u32,
            flags: u32,
        ) -> u32;
    }
    let mut information = std::mem::MaybeUninit::<Information>::uninit();
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr().cast()) } == 0 {
        return Err(io_error(std::io::Error::last_os_error()));
    }
    let information = unsafe { information.assume_init() };
    let required =
        unsafe { GetFinalPathNameByHandleW(file.as_raw_handle(), std::ptr::null_mut(), 0, 0) };
    if required == 0 || required > 32_768 {
        return Err(io_error(std::io::Error::last_os_error()));
    }
    let mut path = allocated(required as usize)?;
    path.resize(required as usize, 0_u16);
    let written =
        unsafe { GetFinalPathNameByHandleW(file.as_raw_handle(), path.as_mut_ptr(), required, 0) };
    if written == 0 || written >= required {
        return Err(io_error(std::io::Error::last_os_error()));
    }
    use std::os::windows::ffi::OsStringExt;
    let canonical_path = PathBuf::from(std::ffi::OsString::from_wide(&path[..written as usize]));
    Ok((
        canonical_path,
        format!(
            "windows:{:08x}:{:08x}{:08x}",
            information.volume, information.index_high, information.index_low
        ),
    ))
}

#[cfg(unix)]
fn handle_identity(file: &File, path: &Path) -> Result<(PathBuf, String), PmxMatchError> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata().map_err(io_error)?;
    let canonical_path = path.canonicalize().map_err(io_error)?;
    let path_metadata = canonical_path.metadata().map_err(io_error)?;
    if metadata.dev() != path_metadata.dev() || metadata.ino() != path_metadata.ino() {
        return Err(PmxMatchError::ChangedDuringRead);
    }
    Ok((
        canonical_path,
        format!("unix:{}:{}", metadata.dev(), metadata.ino()),
    ))
}

/// The expected bytes must come from the generated model used to bind the VMD.
/// Matching is conservative: unsupported rig operations, ambiguous aliases and
/// unproven vertex correspondence fail the whole animation-only request.
pub fn match_existing(path: &Path, expected_pmx: &[u8]) -> Result<ExistingMatch, PmxMatchError> {
    let expected = inspect(expected_pmx)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Preserve a stable file while the handle is held: read sharing only.
        options.share_mode(1);
    }
    let mut file = options.open(path).map_err(io_error)?;
    let before = file.metadata().map_err(io_error)?;
    if !before.is_file() {
        return Err(PmxMatchError::Invalid("target is not a regular file"));
    }
    if before.len() > MAX_PMX_BYTES {
        return Err(PmxMatchError::TooLarge);
    }
    #[cfg(windows)]
    let (canonical_path, file_identity) = handle_identity(&file)?;
    #[cfg(unix)]
    let (canonical_path, file_identity) = handle_identity(&file, path)?;
    #[cfg(not(any(windows, unix)))]
    return Err(PmxMatchError::Unsupported(
        "stable file identity on this platform",
    ));
    #[cfg(any(windows, unix))]
    {
        let length = usize::try_from(before.len()).map_err(|_| PmxMatchError::TooLarge)?;
        let mut bytes = allocated(length)?;
        bytes.resize(length, 0_u8);
        file.read_exact(&mut bytes).map_err(io_error)?;
        let mut extra = [0];
        let after = file.metadata().map_err(io_error)?;
        if file.read(&mut extra).map_err(io_error)? != 0
            || before.len() != after.len()
            || before.modified().map_err(io_error)? != after.modified().map_err(io_error)?
        {
            return Err(PmxMatchError::ChangedDuringRead);
        }
        let actual = inspect(&bytes)?;
        compare(&actual, &expected)?;
        let model_name_cp932 = cp932(&actual.model_name, 19, "model name")?;
        Ok(ExistingMatch {
            canonical_path,
            file_identity,
            model_name_cp932,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct TestBone {
        name: String,
        position: [f32; 3],
        parent: i32,
        flags: u16,
        axes: [[f32; 3]; 2],
    }

    #[derive(Clone)]
    struct Fixture {
        encoding: u8,
        width: usize,
        model: String,
        bones: Vec<TestBone>,
        vertices: Vec<[f32; 3]>,
        faces: Vec<u32>,
        morph_name: String,
        morphs: Vec<u8>,
        delta: [f32; 3],
        morph_vertex: u32,
        rigid_mode: Option<u8>,
        joint: bool,
    }

    fn fixture() -> Fixture {
        Fixture {
            encoding: 0,
            width: 1,
            model: "Target".into(),
            bones: vec![
                TestBone {
                    name: "root".into(),
                    position: [0.; 3],
                    parent: -1,
                    flags: 0x1e,
                    axes: [[1., 0., 0.], [0., 0., 1.]],
                },
                TestBone {
                    name: "Bone".into(),
                    position: [0., 1., 0.],
                    parent: 0,
                    flags: 0x1e,
                    axes: [[1., 0., 0.], [0., 0., 1.]],
                },
            ],
            vertices: vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
            faces: vec![0, 1, 2],
            morph_name: "Smile".into(),
            morphs: vec![1],
            delta: [0., 0.25, 0.],
            morph_vertex: 1,
            rigid_mode: None,
            joint: false,
        }
    }

    fn integer(out: &mut Vec<u8>, value: i32) {
        out.extend(value.to_le_bytes());
    }
    fn floats(out: &mut Vec<u8>, values: &[f32]) {
        for value in values {
            out.extend(value.to_le_bytes());
        }
    }
    fn index(out: &mut Vec<u8>, value: i32, width: usize) {
        out.extend(&value.to_le_bytes()[..width]);
    }
    fn text(out: &mut Vec<u8>, value: &str, encoding: u8) {
        let bytes = if encoding == 0 {
            value
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>()
        } else {
            value.as_bytes().to_vec()
        };
        integer(out, bytes.len() as i32);
        out.extend(bytes);
    }

    // Independent wire fixtures: this helper neither uses the production PMX
    // writer nor calls the reader to determine section sizes or expectations.
    fn wire(f: &Fixture) -> Vec<u8> {
        let mut out = b"PMX ".to_vec();
        floats(&mut out, &[2.]);
        out.extend([8, f.encoding, 4]);
        out.extend([f.width as u8; 6]);
        for value in [&f.model, "", "", ""] {
            text(&mut out, value, f.encoding);
        }
        integer(&mut out, f.vertices.len() as i32);
        for position in &f.vertices {
            floats(&mut out, position);
            floats(&mut out, &[0., 0., 1., 0., 0.]);
            floats(&mut out, &[0.; 16]);
            out.push(0);
            index(&mut out, 0, f.width);
            floats(&mut out, &[1.]);
        }
        integer(&mut out, f.faces.len() as i32);
        for value in &f.faces {
            index(&mut out, *value as i32, f.width);
        }
        integer(&mut out, 0);
        integer(&mut out, 1);
        for value in ["Material", ""] {
            text(&mut out, value, f.encoding);
        }
        floats(&mut out, &[0.; 11]);
        out.push(0);
        floats(&mut out, &[0.; 5]);
        index(&mut out, -1, f.width);
        index(&mut out, -1, f.width);
        out.extend([0, 1, 0]);
        text(&mut out, "", f.encoding);
        integer(&mut out, f.faces.len() as i32);
        integer(&mut out, f.bones.len() as i32);
        for bone in &f.bones {
            for value in [&bone.name, ""] {
                text(&mut out, value, f.encoding);
            }
            floats(&mut out, &bone.position);
            index(&mut out, bone.parent, f.width);
            integer(&mut out, 0);
            out.extend(bone.flags.to_le_bytes());
            if bone.flags & 1 != 0 {
                index(&mut out, -1, f.width);
            } else {
                floats(&mut out, &[0., 1., 0.]);
            }
            if bone.flags & 0x300 != 0 {
                index(&mut out, 0, f.width);
                floats(&mut out, &[0.5]);
            }
            if bone.flags & 0x400 != 0 {
                floats(&mut out, &[1., 0., 0.]);
            }
            if bone.flags & 0x800 != 0 {
                floats(&mut out, &bone.axes[0]);
                floats(&mut out, &bone.axes[1]);
            }
            if bone.flags & 0x2000 != 0 {
                integer(&mut out, 7);
            }
            if bone.flags & 0x20 != 0 {
                index(&mut out, 0, f.width);
                integer(&mut out, 4);
                floats(&mut out, &[0.2]);
                integer(&mut out, 2);
                index(&mut out, 0, f.width);
                out.push(0);
                index(&mut out, 1, f.width);
                out.push(1);
                floats(&mut out, &[0.; 6]);
            }
        }
        integer(&mut out, f.morphs.len() as i32);
        for (number, kind) in f.morphs.iter().enumerate() {
            let name = if f.morphs.len() == 1 {
                f.morph_name.clone()
            } else {
                format!("m{number}")
            };
            for value in [&name, ""] {
                text(&mut out, value, f.encoding);
            }
            out.extend([1, *kind]);
            integer(&mut out, 1);
            match kind {
                0 => {
                    index(&mut out, 1, f.width);
                    floats(&mut out, &[0.5]);
                }
                1 => {
                    index(&mut out, f.morph_vertex as i32, f.width);
                    floats(&mut out, &f.delta);
                }
                2 => {
                    index(&mut out, 1, f.width);
                    floats(&mut out, &[0., 0., 0., 0., 0., 0., 1.]);
                }
                3..=7 => {
                    index(&mut out, 1, f.width);
                    floats(&mut out, &[0.; 4]);
                }
                8 => {
                    index(&mut out, -1, f.width);
                    out.push(1);
                    floats(&mut out, &[0.; 28]);
                }
                _ => {}
            }
        }
        integer(&mut out, 1);
        for value in ["Display", ""] {
            text(&mut out, value, f.encoding);
        }
        out.push(1);
        integer(&mut out, if f.morphs.is_empty() { 1 } else { 2 });
        out.push(0);
        index(&mut out, 0, f.width);
        if !f.morphs.is_empty() {
            out.push(1);
            index(&mut out, 0, f.width);
        }
        integer(&mut out, i32::from(f.rigid_mode.is_some()));
        if let Some(mode) = f.rigid_mode {
            for value in ["Body", ""] {
                text(&mut out, value, f.encoding);
            }
            index(&mut out, 1, f.width);
            out.push(0);
            out.extend(0_u16.to_le_bytes());
            out.push(0);
            floats(&mut out, &[0.; 14]);
            out.push(mode);
        }
        integer(&mut out, i32::from(f.joint));
        if f.joint {
            for value in ["Joint", ""] {
                text(&mut out, value, f.encoding);
            }
            out.push(0);
            index(&mut out, 0, f.width);
            index(&mut out, -1, f.width);
            floats(&mut out, &[0.; 24]);
        }
        out
    }

    fn compared(actual: &Fixture, expected: &Fixture) -> Result<(), PmxMatchError> {
        compare(&inspect(&wire(actual))?, &inspect(&wire(expected))?)
    }

    #[test]
    fn matching_display_axes_require_the_same_reference() {
        let mut expected = fixture();
        expected.bones[1].flags |= 0x800;
        expected.bones[1].axes = [[0., 1., 0.], [0., 0., 1.]];
        let mut actual = expected.clone();
        assert_eq!(compared(&actual, &expected), Ok(()));
        actual.bones[1].axes[0] = [1., 0., 0.];
        assert_eq!(compared(&actual, &expected),
            Err(PmxMatchError::Unsupported("non-default bone local axes")));
    }

    #[test]
    fn group_1_valid_encodings_widths_order_and_morph_correspondence() {
        let mut expected = fixture();
        expected.vertices = (0..256).map(|i| [i as f32, 0., 0.]).collect();
        expected.faces = vec![0, 1, 255];
        for width in [1, 2, 4] {
            for encoding in [0, 1] {
                let mut actual = expected.clone();
                actual.width = width;
                actual.encoding = encoding;
                // Bone indices may change; semantic parents still name root.
                actual.bones.swap(0, 1);
                actual.bones[0].parent = 1;
                actual.bones[1].parent = -1;
                actual.bones[0].flags |= 0x800;
                actual.rigid_mode = Some(0);
                assert_eq!(compared(&actual, &expected), Ok(()));
            }
        }
        // Float32-independent bounds just inside the fixed tolerances.
        let mut actual = expected.clone();
        actual.bones[1].position[0] += 0.00009;
        actual.delta[0] += 0.0000009;
        assert_eq!(compared(&actual, &expected), Ok(()));
        let mut no_morph = fixture();
        no_morph.morphs.clear();
        let mut other_geometry = no_morph.clone();
        other_geometry.vertices[0][0] = 7.;
        assert_eq!(compared(&other_geometry, &no_morph), Ok(()));
    }

    #[test]
    fn group_2_names_rest_parent_flags_and_morph_meaning_fail_closed() {
        let expected = fixture();
        let mut actual = expected.clone();
        actual.bones[1].name = "Other".into();
        assert_eq!(
            compared(&actual, &expected),
            Err(PmxMatchError::NamesMismatch)
        );
        let mut actual = expected.clone();
        actual.bones[1].position[0] = 0.0002;
        assert_eq!(
            compared(&actual, &expected),
            Err(PmxMatchError::RestMismatch("absolute bone position"))
        );
        let mut actual = expected.clone();
        actual.bones[1].parent = -1;
        assert_eq!(
            compared(&actual, &expected),
            Err(PmxMatchError::RestMismatch("bone parent"))
        );
        let mut actual = expected.clone();
        actual.bones[1].flags ^= 4;
        assert_eq!(
            compared(&actual, &expected),
            Err(PmxMatchError::RestMismatch("bone flags/layer"))
        );
        let mut actual = expected.clone();
        actual.bones[1].flags |= 0x800;
        actual.bones[1].axes[0] = [0., 1., 0.];
        assert_eq!(
            compared(&actual, &expected),
            Err(PmxMatchError::Unsupported("non-default bone local axes"))
        );
        let mut actual = expected.clone();
        actual.bones[1].flags |= 0x3f21;
        assert!(matches!(
            compared(&actual, &expected),
            Err(PmxMatchError::Unsupported(_))
        ));
        // Every conditional payload, both IK limit tags and all PMX 2.0 morph
        // layouts must be consumed before the explicit unsupported result.
        actual.morphs = (0..=8).collect();
        actual.rigid_mode = Some(2);
        actual.joint = true;
        let all = wire(&actual);
        assert_eq!(inspect(&all).unwrap().morphs.len(), 9);
        let mut extra = all;
        extra.push(0);
        assert!(matches!(inspect(&extra), Err(PmxMatchError::TrailingBytes)));
        for change in 0..4 {
            let mut actual = expected.clone();
            match change {
                0 => actual.delta[0] = 0.000002,
                1 => actual.morph_vertex = 2,
                2 => actual.vertices[1][0] = 1.0001,
                _ => actual.faces.swap(1, 2),
            }
            assert!(matches!(
                compared(&actual, &expected),
                Err(PmxMatchError::MorphMismatch(_))
            ));
        }
        let mut actual = expected.clone();
        actual.rigid_mode = Some(1);
        assert_eq!(
            compared(&actual, &expected),
            Err(PmxMatchError::Unsupported("dynamic rigid body physics"))
        );
    }

    #[test]
    fn group_3_malformed_names_counts_and_hardlinks_preserve_input() {
        let expected = fixture();
        let bytes = wire(&expected);
        for end in 0..bytes.len() {
            assert!(
                inspect(&bytes[..end]).is_err(),
                "accepted truncated prefix {end}"
            );
        }
        for width in [0, 3, 8] {
            let mut bad = bytes.clone();
            bad[11] = width;
            assert!(inspect(&bad).is_err());
        }
        for count in [i32::MAX, -1] {
            let mut bad = bytes.clone();
            bad[17..21].copy_from_slice(&count.to_le_bytes());
            assert!(inspect(&bad).is_err());
            let count_bytes = count.to_le_bytes();
            let mut r = Reader {
                bytes: &count_bytes,
                offset: 0,
                encoding: 0,
                uv: 0,
                widths: Widths {
                    vertex: 1,
                    texture: 1,
                    material: 1,
                    bone: 1,
                    morph: 1,
                    rigid: 1,
                },
            };
            assert!(r.count(MAX_VERTICES, 38, "vertices").is_err());
        }
        let mut bad = expected.clone();
        bad.bones[1].name = "root".into();
        assert!(matches!(
            inspect(&wire(&bad)),
            Err(PmxMatchError::Invalid(
                "duplicate or ambiguous bone/morph alias"
            ))
        ));
        let mut bad = expected.clone();
        bad.bones[0].parent = 1;
        assert!(matches!(
            inspect(&wire(&bad)),
            Err(PmxMatchError::Invalid("cyclic bone parents"))
        ));
        for name in ["", "123456789012345", "has\0nul", "😀"] {
            let mut bad = expected.clone();
            bad.bones[1].name = name.into();
            assert!(matches!(
                inspect(&wire(&bad)),
                Err(PmxMatchError::Name { .. })
            ));
        }
        let folder = std::env::current_dir()
            .unwrap()
            .join("target/m3-pmx-match-20261021/fixtures");
        std::fs::create_dir_all(&folder).unwrap();
        let original = folder.join(format!("model-{}.pmx", std::process::id()));
        let linked = folder.join(format!("hardlink-{}.pmx", std::process::id()));
        std::fs::write(&original, &bytes).unwrap();
        std::fs::hard_link(&original, &linked).unwrap();
        let a = match_existing(&original, &bytes).unwrap();
        let b = match_existing(&linked, &bytes).unwrap();
        assert_eq!(a.file_identity, b.file_identity);
        assert_eq!(a.model_name_cp932, b"Target");
        assert!(a.canonical_path.is_absolute());
        assert_eq!(std::fs::read(&original).unwrap(), bytes);
        assert_eq!(std::fs::read(&linked).unwrap(), bytes);
        let mut bad_model = expected;
        bad_model.model = "12345678901234567890".into();
        std::fs::write(&original, wire(&bad_model)).unwrap();
        assert!(matches!(
            match_existing(&original, &bytes),
            Err(PmxMatchError::Name {
                field: "model name",
                ..
            })
        ));
        std::fs::remove_file(&linked).unwrap();
        std::fs::remove_file(&original).unwrap();
    }
}
