//! PMX 2.0 output from validated rest geometry. MMD9.32 requires UTF-16LE text.
//! PMX bone rest transforms are translations at global heads. Display local axes
//! do not replace this rotational basis; motion must solve evaluated world skin.

use crate::{
    coordinates::{CoordinateError, CoordinateTransform},
    names::{self, NameAlias, NameError, NameInput, NameKind},
    packet::{BoneWeight, Matrix, ValidatedPacket, MAX_GROUP_BONES},
};
use std::fmt;

const IDENTITY: [[f64; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];
const MAX_PMX_BYTES: usize = 2 * 1024 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct BoneBinding {
    /// None only for the generated root; other indices address packet.bones.
    pub source_index: Option<usize>,
    pub pmx_index: u32,
    pub parent_pmx: Option<u32>,
    pub name: NameAlias,
    pub rest_head: [f32; 3],
}

#[derive(Clone, Debug)]
pub struct MorphBinding {
    pub mesh_index: usize,
    pub source_index: usize,
    pub pmx_index: u32,
    pub name: NameAlias,
}

#[derive(Clone, Debug)]
pub struct ModelBinding {
    pub group_index: usize,
    /// PMX primary model name and VMD model header share these exact bytes.
    pub model_name: NameAlias,
    pub root: BoneBinding,
    /// Source order is already topological. PMX index = source index + 1.
    pub bones: Vec<BoneBinding>,
    /// Stable mesh order, then source morph order.
    pub morphs: Vec<MorphBinding>,
    pub scale_to_mmd: f64,
}

pub struct ModelOutput {
    pub pmx: Vec<u8>,
    pub binding: ModelBinding,
    pub name_map_tsv: String,
}

#[derive(Debug)]
pub enum PmxError {
    GroupIndex,
    MissingGeometry,
    Limit(&'static str),
    Invalid(&'static str),
    Allocation,
    Names(NameError),
    Coordinates {
        target: String,
        error: CoordinateError,
    },
}
impl fmt::Display for PmxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GroupIndex => f.write_str("PMX group index is out of range"),
            Self::MissingGeometry => f.write_str("PMX model has no triangle geometry"),
            Self::Limit(field) => write!(f, "PMX limit: {field}"),
            Self::Invalid(field) => write!(f, "invalid PMX input: {field}"),
            Self::Allocation => f.write_str("PMX allocation failed"),
            Self::Names(error) => write!(f, "PMX names: {error}"),
            Self::Coordinates { target, error } => write!(f, "PMX coordinates ({target}): {error}"),
        }
    }
}
impl std::error::Error for PmxError {}
impl From<NameError> for PmxError {
    fn from(error: NameError) -> Self {
        Self::Names(error)
    }
}

fn reserved<T>(count: usize) -> Result<Vec<T>, PmxError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| PmxError::Allocation)?;
    Ok(values)
}

fn coordinate<T>(target: &str, result: Result<T, CoordinateError>) -> Result<T, PmxError> {
    result.map_err(|error| PmxError::Coordinates {
        target: target.to_owned(),
        error,
    })
}

fn object_matrix(matrix: &Matrix, translation: bool) -> [[f64; 4]; 4] {
    let mut rows =
        std::array::from_fn(|row| std::array::from_fn(|column| matrix[row * 4 + column]));
    // Packet validation already checked the affine row, allowing rounding noise.
    rows[3] = [0.0, 0.0, 0.0, 1.0];
    if !translation {
        for row in &mut rows[..3] {
            row[3] = 0.0;
        }
    }
    rows
}

/// Also works for animation-only armatures without geometry. The generated root
/// has head zero and an identity rotational rest basis, just like all PMX bones.
pub fn build_binding(
    validated: &ValidatedPacket<'_>,
    group_index: usize,
    units_per_meter: f64,
) -> Result<ModelBinding, PmxError> {
    let scene = validated.packet();
    let group = scene.groups.get(group_index).ok_or(PmxError::GroupIndex)?;
    let bone_count = group
        .bones
        .len()
        .checked_add(1)
        .filter(|n| *n <= MAX_GROUP_BONES)
        .ok_or(PmxError::Limit("bones including generated root"))?;
    let global = coordinate(
        "scene units",
        CoordinateTransform::new(units_per_meter, scene.unit_scale, IDENTITY),
    )?;
    let morph_count = group
        .meshes
        .iter()
        .try_fold(0_usize, |total, mesh| total.checked_add(mesh.morphs.len()))
        .filter(|n| *n <= i32::MAX as usize)
        .ok_or(PmxError::Limit("morph count"))?;
    let mut morph_ids = reserved(morph_count)?;
    for (mesh_index, mesh) in group.meshes.iter().enumerate() {
        for morph in &mesh.morphs {
            morph_ids.push(format!("mesh[{mesh_index}]/{}", morph.source_id));
        }
    }
    let mut inputs = reserved(
        bone_count
            .checked_add(morph_count)
            .and_then(|n| n.checked_add(1))
            .ok_or(PmxError::Limit("names"))?,
    )?;
    inputs.push(NameInput {
        kind: NameKind::Model,
        source_id: &group.source_id,
        original: &group.name,
    });
    inputs.push(NameInput {
        kind: NameKind::Root,
        source_id: &group.source_id,
        original: "NexaRoot",
    });
    for bone in &group.bones {
        inputs.push(NameInput {
            kind: NameKind::Bone,
            source_id: &bone.source_id,
            original: &bone.name,
        });
    }
    let mut id = morph_ids.iter();
    for mesh in &group.meshes {
        for morph in &mesh.morphs {
            inputs.push(NameInput {
                kind: NameKind::Morph,
                source_id: id.next().unwrap(),
                original: &morph.name,
            });
        }
    }
    let mut names = names::build_name_map(&inputs)?.into_iter();
    let model_name = names.next().unwrap();
    let root = BoneBinding {
        source_index: None,
        pmx_index: 0,
        parent_pmx: None,
        name: names.next().unwrap(),
        rest_head: [0.0; 3],
    };
    let mut bones = reserved(group.bones.len())?;
    for (source_index, bone) in group.bones.iter().enumerate() {
        // A singular source rest basis cannot support the corresponding motion.
        coordinate(
            &bone.name,
            CoordinateTransform::new(1.0, 1.0, object_matrix(&bone.rest_world, true)),
        )?;
        bones.push(BoneBinding {
            source_index: Some(source_index),
            pmx_index: (source_index + 1) as u32,
            parent_pmx: Some(bone.parent.map_or(0, |parent| parent + 1)),
            name: names.next().unwrap(),
            rest_head: coordinate(&bone.name, global.point(bone.head_world))?,
        });
    }
    let mut morphs = reserved(morph_count)?;
    for (mesh_index, mesh) in group.meshes.iter().enumerate() {
        for source_index in 0..mesh.morphs.len() {
            morphs.push(MorphBinding {
                mesh_index,
                source_index,
                pmx_index: morphs.len() as u32,
                name: names.next().unwrap(),
            });
        }
    }
    Ok(ModelBinding {
        group_index,
        model_name,
        root,
        bones,
        morphs,
        scale_to_mmd: units_per_meter * scene.unit_scale,
    })
}

pub fn build_model(
    validated: &ValidatedPacket<'_>,
    group_index: usize,
    units_per_meter: f64,
) -> Result<ModelOutput, PmxError> {
    build_model_impl(validated, group_index, units_per_meter, false)
}

/// Comparison input for animation-only existing-PMX matching. May contain no
/// geometry; it is not a successfully converted/publishable model artifact.
pub fn build_match_reference(
    validated: &ValidatedPacket<'_>,
    group_index: usize,
    units_per_meter: f64,
) -> Result<ModelOutput, PmxError> {
    build_model_impl(validated, group_index, units_per_meter, true)
}

fn build_model_impl(
    validated: &ValidatedPacket<'_>,
    group_index: usize,
    units_per_meter: f64,
    match_reference: bool,
) -> Result<ModelOutput, PmxError> {
    let binding = build_binding(validated, group_index, units_per_meter)?;
    let scene = validated.packet();
    let group = &scene.groups[group_index];
    if !match_reference {
        for image in &scene.images {
            validate_png(image)?;
        }
    }
    let vertex_count = group
        .meshes
        .iter()
        .try_fold(0_usize, |total, mesh| {
            total.checked_add(mesh.vertices.len())
        })
        .ok_or(PmxError::Limit("vertices"))?;
    let triangle_count = group
        .meshes
        .iter()
        .try_fold(0_usize, |total, mesh| {
            total.checked_add(mesh.triangles.len())
        })
        .ok_or(PmxError::Limit("triangles"))?;
    if !match_reference && (vertex_count == 0 || triangle_count == 0) {
        return Err(PmxError::MissingGeometry);
    }
    let index_count = triangle_count
        .checked_mul(3)
        .ok_or(PmxError::Limit("surface indices"))?;
    let material_count = group
        .meshes
        .iter()
        .try_fold(0_usize, |total, mesh| {
            total.checked_add(mesh.materials.len())
        })
        .ok_or(PmxError::Limit("materials"))?;
    let mut output = Writer(Vec::new());
    output.bytes(b"PMX ")?;
    output.floats([2.0])?;
    output.bytes(&[8, 0, 0, 4, 4, 4, 4, 4, 4])?; // UTF16LE, no extra UV, 32-bit indices.
    output.text(&binding.model_name.alias)?;
    output.text(&group.name)?;
    let comment = "Nexa Blend Importer: defined Principled Base Color/Alpha diffuse conversion; not PBR shader equivalence.";
    output.text(comment)?;
    output.text(comment)?;
    output.count(vertex_count)?;
    let mut transforms = reserved(group.meshes.len())?;
    let mut vertex_bases = reserved(group.meshes.len())?;
    let mut vertex_base = 0_u32;
    for mesh in &group.meshes {
        let transform = coordinate(
            &mesh.name,
            CoordinateTransform::new(
                units_per_meter,
                scene.unit_scale,
                object_matrix(&mesh.world_from_mesh, true),
            ),
        )?;
        vertex_bases.push(vertex_base);
        for vertex in &mesh.vertices {
            output.floats(coordinate(&mesh.name, transform.point(vertex.position))?)?;
            output.floats(coordinate(&mesh.name, transform.normal(vertex.normal))?)?;
            output.floats(coordinate(&mesh.name, transform.uv(vertex.uv))?)?;
            write_weights(&mut output, &vertex.weights)?;
            output.floats([0.0])?; // Edge disabled, including per-vertex scale.
        }
        vertex_base = vertex_base
            .checked_add(mesh.vertices.len() as u32)
            .ok_or(PmxError::Limit("vertex index"))?;
        transforms.push(transform);
    }
    // PMX assigns consecutive surface ranges to each material. Reorder triangles
    // within each mesh by material, preserving source order inside each range.
    output.count(index_count)?;
    let mut material_counts = reserved(group.meshes.len())?;
    for (mesh_index, mesh) in group.meshes.iter().enumerate() {
        // Counting sort is linear in faces + materials, including large packets.
        let mut counts = reserved(mesh.materials.len())?;
        counts.resize(mesh.materials.len(), 0_usize);
        for triangle in &mesh.triangles {
            counts[triangle.material as usize] += 1;
        }
        let mut next = reserved(counts.len())?;
        let mut total = 0_usize;
        for count in &counts {
            next.push(total);
            total += count;
        }
        let mut order = reserved(mesh.triangles.len())?;
        order.resize(mesh.triangles.len(), 0_usize);
        for (index, triangle) in mesh.triangles.iter().enumerate() {
            let slot = &mut next[triangle.material as usize];
            order[*slot] = index;
            *slot += 1;
        }
        for triangle_index in order {
            for index in transforms[mesh_index].triangle(mesh.triangles[triangle_index].vertices) {
                output.u32(
                    index
                        .checked_add(vertex_bases[mesh_index])
                        .ok_or(PmxError::Limit("vertex index"))?,
                )?;
            }
        }
        material_counts.push(counts);
    }
    output.count(scene.images.len())?;
    for index in 0..scene.images.len() {
        output.text(&format!("textures/texture_{:04}.png", index + 1))?;
    }
    output.count(material_count)?;
    for (mesh_index, mesh) in group.meshes.iter().enumerate() {
        for (material_index, material) in mesh.materials.iter().enumerate() {
            output.text(&material.name)?;
            output.text(&material.name)?;
            output.floats(material.diffuse.map(|value| value as f32))?;
            output.floats([0.0; 7])?; // Specular RGB, power, ambient RGB.
            output.byte(0)?; // No edge/culling/shadow features are inferred.
            output.floats([0.0; 5])?; // Edge color RGBA and size.
            output.i32(material.image.map_or(-1, |index| index as i32))?;
            output.i32(-1)?; // No sphere texture.
            output.byte(0)?; // Sphere mode OFF.
            output.byte(0)?; // Individual toon texture, absent.
            output.i32(-1)?;
            output.text("Defined diffuse conversion of Principled Base Color/Alpha")?;
            let count = material_counts[mesh_index][material_index];
            output.count(
                count
                    .checked_mul(3)
                    .ok_or(PmxError::Limit("material indices"))?,
            )?;
        }
    }
    output.count(binding.bones.len() + 1)?;
    write_bone(&mut output, &binding.root, [0.0, 1.0, 0.0], None)?;
    let global = coordinate(
        "scene units",
        CoordinateTransform::new(units_per_meter, scene.unit_scale, IDENTITY),
    )?;
    for (bone, bound) in group.bones.iter().zip(&binding.bones) {
        let delta = std::array::from_fn(|axis| bone.tail_world[axis] - bone.head_world[axis]);
        let tail = coordinate(&bone.name, global.point(delta))?;
        let axes = display_axes(&bone.rest_world)?;
        write_bone(&mut output, bound, tail, Some(axes))?;
    }
    output.count(binding.morphs.len())?;
    for bound in &binding.morphs {
        let mesh = &group.meshes[bound.mesh_index];
        let morph = &mesh.morphs[bound.source_index];
        let linear = coordinate(
            &mesh.name,
            CoordinateTransform::new(
                units_per_meter,
                scene.unit_scale,
                object_matrix(&mesh.world_from_mesh, false),
            ),
        )?;
        output.text(&bound.name.alias)?;
        output.text(&bound.name.original)?;
        output.bytes(&[4, 1])?; // Other expression panel, vertex morph.
        output.count(morph.offsets.len())?;
        for offset in &morph.offsets {
            output.u32(
                offset
                    .vertex
                    .checked_add(vertex_bases[bound.mesh_index])
                    .ok_or(PmxError::Limit("morph vertex"))?,
            )?;
            output.floats(coordinate(&morph.name, linear.point(offset.delta))?)?;
        }
    }
    output.count(3)?;
    write_display(&mut output, "Root", "Root", true, 0, &[0])?;
    let morph_indices: Vec<u32> = binding.morphs.iter().map(|morph| morph.pmx_index).collect();
    write_display(&mut output, "表情", "Exp", true, 1, &morph_indices)?;
    let bone_indices: Vec<u32> = binding.bones.iter().map(|bone| bone.pmx_index).collect();
    write_display(&mut output, "操作", "Bones", false, 0, &bone_indices)?;
    output.count(0)?; // No inferred physics rigid bodies.
    output.count(0)?; // No joints. PMX2.0 has no soft-body section.
    let name_map_tsv = names::mapping_tsv(
        std::iter::once(&binding.model_name)
            .chain(std::iter::once(&binding.root.name))
            .chain(binding.bones.iter().map(|bone| &bone.name))
            .chain(binding.morphs.iter().map(|morph| &morph.name)),
    );
    Ok(ModelOutput {
        pmx: output.0,
        binding,
        name_map_tsv,
    })
}

fn write_weights(output: &mut Writer, weights: &[BoneWeight]) -> Result<(), PmxError> {
    if weights.is_empty() {
        output.byte(0)?;
        return output.i32(0);
    }
    if weights.len() > 4 {
        return Err(PmxError::Limit("positive bone weights"));
    }
    let sum: f64 = weights.iter().map(|weight| weight.weight).sum();
    if !sum.is_finite() || sum <= 0.0 {
        return Err(PmxError::Invalid("weight sum"));
    }
    let mut bones = [0_i32; 4];
    let mut normalized = [0.0_f32; 4];
    for (index, weight) in weights.iter().enumerate() {
        bones[index] =
            i32::try_from(weight.bone + 1).map_err(|_| PmxError::Limit("weight bone index"))?;
        normalized[index] = (weight.weight / sum) as f32;
        if !normalized[index].is_finite() || normalized[index] <= 0.0 {
            return Err(PmxError::Invalid("weight lost during float32 conversion"));
        }
    }
    match weights.len() {
        1 => {
            output.byte(0)?;
            output.i32(bones[0])
        }
        2 => {
            if normalized[0] >= 1.0 {
                return Err(PmxError::Invalid(
                    "BDEF2 weight lost during float32 conversion",
                ));
            }
            output.byte(1)?;
            output.i32(bones[0])?;
            output.i32(bones[1])?;
            output.floats([normalized[0]])
        }
        _ => {
            output.byte(2)?;
            for bone in bones {
                output.i32(bone)?;
            }
            output.floats(normalized)
        }
    }
}

// Integrity only: Blender decodes the source pixels and re-encodes PNG. Rust does
// not claim to decompress IDAT or prove that the compressed pixel stream decodes.
fn validate_png(image: &crate::packet::ImagePacket) -> Result<(), PmxError> {
    const TABLE: [u32; 256] = {
        let mut table = [0; 256];
        let mut index = 0;
        while index < 256 {
            let mut value = index as u32;
            let mut bit = 0;
            while bit < 8 {
                value = if value & 1 != 0 {
                    (value >> 1) ^ 0xEDB8_8320
                } else {
                    value >> 1
                };
                bit += 1;
            }
            table[index] = value;
            index += 1;
        }
        table
    };
    let invalid = || PmxError::Invalid("PNG chunk integrity");
    let png = &image.png;
    if png.get(..8) != Some(b"\x89PNG\r\n\x1a\n".as_slice()) {
        return Err(invalid());
    }
    let mut offset = 8_usize;
    let mut ihdr = false;
    let mut idat = false;
    let mut idat_closed = false;
    let mut compressed_bytes = 0_usize;
    let mut palette = false;
    let mut color = 0_u8;
    while offset < png.len() {
        let header_end = offset.checked_add(8).ok_or_else(invalid)?;
        let header = png.get(offset..header_end).ok_or_else(invalid)?;
        let length = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
        let kind: &[u8] = &header[4..];
        if !kind.iter().all(u8::is_ascii_alphabetic) || !kind[2].is_ascii_uppercase() {
            return Err(invalid());
        }
        let body_end = header_end.checked_add(length).ok_or_else(invalid)?;
        let end = body_end.checked_add(4).ok_or_else(invalid)?;
        let body = png.get(header_end..body_end).ok_or_else(invalid)?;
        let stored_crc = u32::from_be_bytes(
            png.get(body_end..end)
                .ok_or_else(invalid)?
                .try_into()
                .unwrap(),
        );
        let mut crc = !0_u32;
        for byte in &png[offset + 4..body_end] {
            crc = TABLE[((crc ^ u32::from(*byte)) & 255) as usize] ^ (crc >> 8);
        }
        if !crc != stored_crc || (!ihdr && kind != b"IHDR") {
            return Err(invalid());
        }
        match kind {
            b"IHDR" => {
                if ihdr || length != 13 {
                    return Err(invalid());
                }
                if u32::from_be_bytes(body[..4].try_into().unwrap()) != image.width
                    || u32::from_be_bytes(body[4..8].try_into().unwrap()) != image.height
                {
                    return Err(invalid());
                }
                color = body[9];
                let depth_valid = match color {
                    0 => matches!(body[8], 1 | 2 | 4 | 8 | 16),
                    2 | 4 | 6 => matches!(body[8], 8 | 16),
                    3 => matches!(body[8], 1 | 2 | 4 | 8),
                    _ => false,
                };
                if !depth_valid || body[10] != 0 || body[11] != 0 || body[12] > 1 {
                    return Err(invalid());
                }
                ihdr = true;
            }
            b"PLTE" => {
                if palette
                    || idat
                    || matches!(color, 0 | 4)
                    || length == 0
                    || length > 768
                    || length % 3 != 0
                {
                    return Err(invalid());
                }
                palette = true;
            }
            b"IDAT" => {
                if idat_closed || (color == 3 && !palette) {
                    return Err(invalid());
                }
                idat = true;
                compressed_bytes = compressed_bytes.checked_add(length).ok_or_else(invalid)?;
            }
            b"IEND" => {
                return if length == 0 && idat && compressed_bytes > 0 && end == png.len() {
                    Ok(())
                } else {
                    Err(invalid())
                };
            }
            _ => {
                if kind[0].is_ascii_uppercase() {
                    return Err(invalid());
                }
            }
        }
        if idat && kind != b"IDAT" {
            idat_closed = true;
        }
        offset = end;
    }
    Err(invalid())
}

// These axes control MMD's local editing display, not the bone's bind rotation.
fn display_axes(rest: &Matrix) -> Result<[[f32; 3]; 2], PmxError> {
    fn normalize(value: [f64; 3]) -> Result<[f64; 3], PmxError> {
        let largest = value.iter().map(|v| v.abs()).fold(0.0_f64, f64::max);
        if !largest.is_finite() || largest == 0.0 {
            return Err(PmxError::Invalid("bone rest axes"));
        }
        let value = value.map(|v| v / largest);
        let length = value.iter().map(|v| v * v).sum::<f64>().sqrt();
        Ok(value.map(|v| v / length))
    }
    let x = normalize([rest[0], rest[8], rest[4]])?;
    let y = normalize([rest[1], rest[9], rest[5]])?;
    // Axis reflection C changes handedness; cross restores a right-handed local
    // editing frame while keeping the converted Blender bone-length direction Y.
    let z = normalize([
        x[1] * y[2] - x[2] * y[1],
        x[2] * y[0] - x[0] * y[2],
        x[0] * y[1] - x[1] * y[0],
    ])?;
    Ok([x.map(|v| v as f32), z.map(|v| v as f32)])
}

fn write_bone(
    output: &mut Writer,
    bone: &BoneBinding,
    tail: [f32; 3],
    axes: Option<[[f32; 3]; 2]>,
) -> Result<(), PmxError> {
    output.text(&bone.name.alias)?;
    output.text(&bone.name.original)?;
    output.floats(bone.rest_head)?;
    output.i32(bone.parent_pmx.map_or(-1, |parent| parent as i32))?;
    output.i32(0)?;
    output.bytes(&(0x001E_u16 | if axes.is_some() { 0x0800 } else { 0 }).to_le_bytes())?;
    output.floats(tail)?;
    if let Some([x, z]) = axes {
        output.floats(x)?;
        output.floats(z)?;
    }
    Ok(())
}

fn write_display(
    output: &mut Writer,
    name: &str,
    english: &str,
    special: bool,
    kind: u8,
    indices: &[u32],
) -> Result<(), PmxError> {
    output.text(name)?;
    output.text(english)?;
    output.byte(u8::from(special))?;
    output.count(indices.len())?;
    for index in indices {
        output.byte(kind)?;
        output.u32(*index)?;
    }
    Ok(())
}

struct Writer(Vec<u8>);
impl Writer {
    fn bytes(&mut self, bytes: &[u8]) -> Result<(), PmxError> {
        self.0
            .len()
            .checked_add(bytes.len())
            .filter(|size| *size <= MAX_PMX_BYTES)
            .ok_or(PmxError::Limit("output bytes"))?;
        self.0
            .try_reserve(bytes.len())
            .map_err(|_| PmxError::Allocation)?;
        self.0.extend_from_slice(bytes);
        Ok(())
    }
    fn byte(&mut self, value: u8) -> Result<(), PmxError> {
        self.bytes(&[value])
    }
    fn i32(&mut self, value: i32) -> Result<(), PmxError> {
        self.bytes(&value.to_le_bytes())
    }
    fn u32(&mut self, value: u32) -> Result<(), PmxError> {
        self.bytes(&value.to_le_bytes())
    }
    fn count(&mut self, count: usize) -> Result<(), PmxError> {
        self.i32(i32::try_from(count).map_err(|_| PmxError::Limit("signed count"))?)
    }
    fn floats<const N: usize>(&mut self, values: [f32; N]) -> Result<(), PmxError> {
        for value in values {
            if !value.is_finite() {
                return Err(PmxError::Invalid("nonfinite output"));
            }
            self.bytes(&value.to_le_bytes())?;
        }
        Ok(())
    }
    fn text(&mut self, text: &str) -> Result<(), PmxError> {
        let bytes = text
            .encode_utf16()
            .count()
            .checked_mul(2)
            .ok_or(PmxError::Limit("text bytes"))?;
        self.count(bytes)?;
        for unit in text.encode_utf16() {
            self.bytes(&unit.to_le_bytes())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::*;

    const FLAT_IDENTITY: Matrix = [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    const MESH_MATRIX: Matrix = [
        0.0, -3.0, 0.0, 10.0, 2.0, 0.0, 0.0, -4.0, 0.0, 0.0, 4.0, 7.0, 0.0, 0.0, 0.0, 1.0,
    ];

    fn png() -> Vec<u8> {
        // A complete 1x1 RGBA PNG with a stored-DEFLATE zlib stream. The test
        // reference CRC is bitwise; production uses a table and shares no helper.
        fn chunk(output: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
            output.extend_from_slice(&(body.len() as u32).to_be_bytes());
            output.extend_from_slice(kind);
            output.extend_from_slice(body);
            let mut crc = !0_u32;
            for byte in kind.iter().chain(body) {
                crc ^= u32::from(*byte);
                for _ in 0..8 {
                    crc = (crc >> 1) ^ (0xEDB8_8320 & 0_u32.wrapping_sub(crc & 1));
                }
            }
            output.extend_from_slice(&(!crc).to_be_bytes());
        }
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        chunk(
            &mut bytes,
            b"IHDR",
            &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0],
        );
        assert_eq!(&bytes[29..33], &[0x1F, 0x15, 0xC4, 0x89]);
        chunk(
            &mut bytes,
            b"IDAT",
            &[
                0x78, 0x01, 0x01, 0x05, 0x00, 0xFA, 0xFF, 0, 0, 0, 0, 0, 0, 5, 0, 1,
            ],
        );
        chunk(&mut bytes, b"IEND", &[]);
        bytes
    }

    fn fixture() -> ScenePacket {
        let vertex = |position, normal, uv, weights| Vertex {
            position,
            normal,
            uv,
            weights,
        };
        let weights = |entries: &[(u32, f64)]| {
            entries
                .iter()
                .map(|(bone, weight)| BoneWeight {
                    bone: *bone,
                    weight: *weight,
                })
                .collect()
        };
        let mut bones = Vec::new();
        for (index, (name, parent, head)) in [
            ("親骨", None, [1.0, 2.0, 3.0]),
            ("長い日本語の名前0123456789", Some(0), [1.0, 3.0, 3.0]),
            ("bone_1", Some(1), [2.0, 3.0, 3.0]),
            ("😀", Some(0), [3.0, 2.0, 3.0]),
        ]
        .into_iter()
        .enumerate()
        {
            let mut rest = FLAT_IDENTITY;
            rest[3] = head[0];
            rest[7] = head[1];
            rest[11] = head[2];
            let mut tail = [head[0], head[1] + 1.0, head[2]];
            if index == 0 {
                rest[0] = 0.0;
                rest[1] = -1.0;
                rest[4] = 1.0;
                rest[5] = 0.0;
                tail = [head[0] - 1.0, head[1], head[2]];
            }
            bones.push(BonePacket {
                source_id: format!("b{index}"),
                name: name.into(),
                parent,
                rest_world: rest,
                head_world: head,
                tail_world: tail,
            });
        }
        ScenePacket {
            scene_name: "Fixture".into(),
            reference_frame: 1.0,
            frame_start: 1.0,
            frame_end: 1.0,
            frames_per_second: 30.0,
            unit_scale: 0.02,
            render_size: [1, 1],
            cameras: vec![],
            images: (0..2)
                .map(|index| ImagePacket {
                    source_id: format!("image{index}"),
                    name: "pixels".into(),
                    width: 1,
                    height: 1,
                    png: png(),
                })
                .collect(),
            groups: vec![ModelPacket {
                source_id: "group".into(),
                name: "日本😀".into(),
                reference_object_world: FLAT_IDENTITY,
                bones,
                poses: vec![],
                meshes: vec![MeshPacket {
                    source_id: "mesh".into(),
                    name: "Seam mesh".into(),
                    world_from_mesh: MESH_MATRIX,
                    vertices: vec![
                        vertex([1.0, 2.0, 3.0], [1.0; 3], [0.25, 0.2], vec![]),
                        vertex([0.0; 3], [0.0, 0.0, 1.0], [0.0; 2], weights(&[(0, 2.0)])),
                        vertex(
                            [1.0, 0.0, 0.0],
                            [0.0, 1.0, 0.0],
                            [1.0, 0.0],
                            weights(&[(0, 2.0), (1, 6.0)]),
                        ),
                        vertex(
                            [1.0, 2.0, 3.0],
                            [1.0; 3],
                            [-0.25, 1.5],
                            weights(&[(0, 3.0), (1, 1.0), (2, 2.0), (3, 4.0)]),
                        ),
                    ],
                    triangles: vec![
                        Triangle {
                            vertices: [0, 1, 2],
                            material: 1,
                        },
                        Triangle {
                            vertices: [3, 2, 1],
                            material: 0,
                        },
                    ],
                    materials: vec![
                        MaterialPacket {
                            name: "Diffuse".into(),
                            diffuse: [0.2, 0.3, 0.4, 0.5],
                            image: Some(1),
                        },
                        MaterialPacket {
                            name: "Plain".into(),
                            diffuse: [1.0; 4],
                            image: None,
                        },
                    ],
                    morphs: vec![VertexMorph {
                        source_id: "smile".into(),
                        name: "笑い😀".into(),
                        offsets: vec![
                            MorphOffset {
                                vertex: 0,
                                delta: [1.0, 2.0, 3.0],
                            },
                            MorphOffset {
                                vertex: 3,
                                delta: [0.0, 1.0, 0.0],
                            },
                        ],
                    }],
                }],
            }],
        }
    }

    // Independent forward reader. It does not call writer helpers, reconstruct
    // a packet or use an inverse coordinate transform as the expected result.
    struct Read<'a> {
        bytes: &'a [u8],
        offset: usize,
    }
    impl Read<'_> {
        fn take(&mut self, count: usize) -> &[u8] {
            let end = self.offset.checked_add(count).unwrap();
            let value = self
                .bytes
                .get(self.offset..end)
                .expect("PMX section truncated");
            self.offset = end;
            value
        }
        fn byte(&mut self) -> u8 {
            self.take(1)[0]
        }
        fn u16(&mut self) -> u16 {
            u16::from_le_bytes(self.take(2).try_into().unwrap())
        }
        fn i32(&mut self) -> i32 {
            i32::from_le_bytes(self.take(4).try_into().unwrap())
        }
        fn u32(&mut self) -> u32 {
            u32::from_le_bytes(self.take(4).try_into().unwrap())
        }
        fn count(&mut self) -> usize {
            usize::try_from(self.i32()).expect("negative PMX count")
        }
        fn floats<const N: usize>(&mut self) -> [f32; N] {
            std::array::from_fn(|_| f32::from_le_bytes(self.take(4).try_into().unwrap()))
        }
        fn text(&mut self) -> String {
            let length = self.count();
            assert_eq!(length % 2, 0, "UTF16 byte count");
            let units: Vec<u16> = self
                .take(length)
                .chunks_exact(2)
                .map(|bytes| u16::from_le_bytes(bytes.try_into().unwrap()))
                .collect();
            String::from_utf16(&units).expect("valid UTF16")
        }
    }
    #[derive(Debug, PartialEq)]
    struct Deform {
        kind: u8,
        bones: [i32; 4],
        weights: [f32; 4],
    }
    struct ReadVertex {
        position: [f32; 3],
        normal: [f32; 3],
        uv: [f32; 2],
        deform: Deform,
    }
    struct ReadMaterial {
        diffuse: [f32; 4],
        texture: i32,
        count: usize,
    }
    struct ReadBone {
        name: String,
        original: String,
        head: [f32; 3],
        parent: i32,
        tail: [f32; 3],
        axes: Option<[[f32; 3]; 2]>,
    }
    struct ReadMorph {
        name: String,
        original: String,
        offsets: Vec<(u32, [f32; 3])>,
    }
    struct ReadModel {
        name: String,
        original: String,
        vertices: Vec<ReadVertex>,
        indices: Vec<u32>,
        textures: Vec<String>,
        materials: Vec<ReadMaterial>,
        bones: Vec<ReadBone>,
        morphs: Vec<ReadMorph>,
        displays: Vec<(String, u8, Vec<(u8, u32)>)>,
    }

    fn read_model(bytes: &[u8]) -> ReadModel {
        let mut reader = Read { bytes, offset: 0 };
        assert_eq!(reader.take(4), b"PMX ");
        assert_eq!(reader.floats::<1>(), [2.0]);
        assert_eq!(reader.take(9), &[8, 0, 0, 4, 4, 4, 4, 4, 4]);
        let name = reader.text();
        let original = reader.text();
        assert!(reader.text().contains("diffuse conversion"));
        reader.text();
        let vertices = (0..reader.count())
            .map(|_| {
                let position = reader.floats();
                let normal = reader.floats();
                let uv = reader.floats();
                let kind = reader.byte();
                let mut bones = [0; 4];
                let mut weights = [0.0; 4];
                match kind {
                    0 => {
                        bones[0] = reader.i32();
                        weights[0] = 1.0;
                    }
                    1 => {
                        bones[0] = reader.i32();
                        bones[1] = reader.i32();
                        weights[0] = reader.floats::<1>()[0];
                        weights[1] = 1.0 - weights[0];
                    }
                    2 => {
                        for bone in &mut bones {
                            *bone = reader.i32();
                        }
                        weights = reader.floats();
                    }
                    _ => panic!("unexpected deform {kind}"),
                }
                assert_eq!(reader.floats::<1>(), [0.0]);
                ReadVertex {
                    position,
                    normal,
                    uv,
                    deform: Deform {
                        kind,
                        bones,
                        weights,
                    },
                }
            })
            .collect();
        let indices = (0..reader.count()).map(|_| reader.u32()).collect();
        let textures = (0..reader.count()).map(|_| reader.text()).collect();
        let materials = (0..reader.count())
            .map(|_| {
                assert!(!reader.text().is_empty());
                reader.text();
                let diffuse = reader.floats();
                assert_eq!(reader.floats::<7>(), [0.0; 7]);
                assert_eq!(reader.byte(), 0);
                assert_eq!(reader.floats::<5>(), [0.0; 5]);
                let texture = reader.i32();
                assert_eq!(reader.i32(), -1);
                assert_eq!(reader.byte(), 0);
                assert_eq!(reader.byte(), 0);
                assert_eq!(reader.i32(), -1);
                reader.text();
                ReadMaterial {
                    diffuse,
                    texture,
                    count: reader.count(),
                }
            })
            .collect();
        let bones = (0..reader.count())
            .map(|_| {
                let name = reader.text();
                let original = reader.text();
                let head = reader.floats();
                let parent = reader.i32();
                assert_eq!(reader.i32(), 0);
                let flags = reader.u16();
                assert_eq!(flags & !0x0800, 0x001E);
                let tail = reader.floats();
                let axes = if flags & 0x0800 != 0 {
                    Some([reader.floats(), reader.floats()])
                } else {
                    None
                };
                ReadBone {
                    name,
                    original,
                    head,
                    parent,
                    tail,
                    axes,
                }
            })
            .collect();
        let morphs = (0..reader.count())
            .map(|_| {
                let name = reader.text();
                let original = reader.text();
                assert_eq!(reader.take(2), &[4, 1]);
                let offsets = (0..reader.count())
                    .map(|_| (reader.u32(), reader.floats()))
                    .collect();
                ReadMorph {
                    name,
                    original,
                    offsets,
                }
            })
            .collect();
        let displays = (0..reader.count())
            .map(|_| {
                let name = reader.text();
                reader.text();
                let special = reader.byte();
                let entries = (0..reader.count())
                    .map(|_| (reader.byte(), reader.u32()))
                    .collect();
                (name, special, entries)
            })
            .collect();
        assert_eq!(reader.count(), 0);
        assert_eq!(reader.count(), 0);
        assert_eq!(reader.offset, bytes.len(), "all PMX2.0 sections consumed");
        ReadModel {
            name,
            original,
            vertices,
            indices,
            textures,
            materials,
            bones,
            morphs,
            displays,
        }
    }

    fn near(actual: [f32; 3], expected: [f64; 3]) {
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert!(
                (f64::from(actual) - expected).abs() < 1e-7,
                "{actual} != {expected}"
            );
        }
    }

    #[test]
    fn mesh_material_readback_checks_seams_winding_normals_uv_texture_and_png_integrity() {
        let scene = fixture();
        let output = build_model(&scene.validate().unwrap(), 0, 12.5).unwrap();
        let read = read_model(&output.pmx);
        assert_eq!(read.name, output.binding.model_name.alias);
        assert_eq!(read.original, "日本😀");
        assert_eq!(
            names::encode_cp932(&read.name).unwrap(),
            output.binding.model_name.cp932
        );
        assert_eq!(read.vertices.len(), 4);
        assert_eq!(read.vertices[0].position, [1.0, 4.75, -0.5]);
        assert_eq!(read.vertices[3].position, read.vertices[0].position);
        assert_eq!(read.vertices[0].uv, [0.25, 0.8]);
        assert_eq!(read.vertices[3].uv, [-0.25, -0.5]);
        let length = 61.0_f64.sqrt();
        near(
            read.vertices[0].normal,
            [-4.0 / length, 3.0 / length, 6.0 / length],
        );
        assert_eq!(read.indices, [3, 1, 2, 0, 2, 1]);
        assert_eq!(
            read.textures,
            ["textures/texture_0001.png", "textures/texture_0002.png"]
        );
        assert_eq!(read.materials.len(), 2);
        assert_eq!(read.materials[0].diffuse, [0.2, 0.3, 0.4, 0.5]);
        assert_eq!((read.materials[0].texture, read.materials[0].count), (1, 3));
        assert_eq!(
            (read.materials[1].texture, read.materials[1].count),
            (-1, 3)
        );
        assert_eq!(read.displays[0], ("Root".into(), 1, vec![(0, 0)]));
        assert_eq!(read.displays[1], ("表情".into(), 1, vec![(1, 0)]));
        assert_eq!(read.displays[2].2, [(0, 1), (0, 2), (0, 3), (0, 4)]);
        let mut damaged = fixture();
        *damaged.images[0].png.last_mut().unwrap() ^= 1;
        assert!(matches!(
            build_model(&damaged.validate().unwrap(), 0, 12.5),
            Err(PmxError::Invalid("PNG chunk integrity"))
        ));
        damaged.images[0].png = png();
        damaged.images[0].png.truncate(33);
        assert!(matches!(
            build_model(&damaged.validate().unwrap(), 0, 12.5),
            Err(PmxError::Invalid("PNG chunk integrity"))
        ));
    }

    #[test]
    fn rig_weight_readback_checks_root_topology_global_heads_axes_bdef_and_bone_limit() {
        let scene = fixture();
        let output = build_model(&scene.validate().unwrap(), 0, 12.5).unwrap();
        let read = read_model(&output.pmx);
        assert_eq!(output.binding.root.pmx_index, 0);
        assert_eq!(output.binding.root.source_index, None);
        assert_eq!(output.binding.scale_to_mmd, 0.25);
        assert_eq!(read.bones.len(), 5);
        assert_eq!(read.bones[0].parent, -1);
        assert_eq!(read.bones[0].head, [0.0; 3]);
        assert_eq!(read.bones[1].head, [0.25, 0.75, 0.5]);
        assert_eq!(read.bones[1].tail, [-0.25, 0.0, 0.0]);
        assert_eq!(
            read.bones[1].axes,
            Some([[0.0, 0.0, 1.0], [0.0, -1.0, 0.0]])
        );
        assert_eq!(
            read.bones
                .iter()
                .map(|bone| bone.parent)
                .collect::<Vec<_>>(),
            [-1, 0, 1, 2, 1]
        );
        for (index, bound) in output.binding.bones.iter().enumerate() {
            assert_eq!(bound.source_index, Some(index));
            assert_eq!(bound.pmx_index, (index + 1) as u32);
            assert_eq!(read.bones[index + 1].name, bound.name.alias);
            assert_eq!(
                read.bones[index + 1].original,
                scene.groups[0].bones[index].name
            );
        }
        assert_eq!(
            read.vertices[0].deform,
            Deform {
                kind: 0,
                bones: [0; 4],
                weights: [1.0, 0.0, 0.0, 0.0]
            }
        );
        assert_eq!(
            read.vertices[1].deform,
            Deform {
                kind: 0,
                bones: [1, 0, 0, 0],
                weights: [1.0, 0.0, 0.0, 0.0]
            }
        );
        assert_eq!(
            read.vertices[2].deform,
            Deform {
                kind: 1,
                bones: [1, 2, 0, 0],
                weights: [0.25, 0.75, 0.0, 0.0]
            }
        );
        assert_eq!(
            read.vertices[3].deform,
            Deform {
                kind: 2,
                bones: [1, 2, 3, 4],
                weights: [0.3, 0.1, 0.2, 0.4]
            }
        );
        let mut empty = fixture();
        empty.groups[0].meshes.clear();
        empty.images.clear();
        assert!(matches!(
            build_model(&empty.validate().unwrap(), 0, 12.5),
            Err(PmxError::MissingGeometry)
        ));
        let reference = build_match_reference(&empty.validate().unwrap(), 0, 12.5).unwrap();
        let read = read_model(&reference.pmx);
        assert!(read.vertices.is_empty() && read.materials.is_empty());
        assert_eq!(read.bones.len(), 5);
        assert!(read.displays[1].2.is_empty());
        let template = empty.groups[0].bones[0].clone();
        empty.groups[0].bones = (0..MAX_GROUP_BONES)
            .map(|index| BonePacket {
                source_id: format!("bone{index}"),
                parent: None,
                ..template.clone()
            })
            .collect();
        assert!(matches!(
            build_binding(&empty.validate().unwrap(), 0, 12.5),
            Err(PmxError::Limit("bones including generated root"))
        ));
    }

    #[test]
    fn morph_readback_checks_relative_linear_deltas_seam_indices_mesh_bases_and_shared_names() {
        let mut scene = fixture();
        let mut second = scene.groups[0].meshes[0].clone();
        second.source_id = "second".into();
        second.world_from_mesh = [
            -1.0, 0.0, 0.0, 50.0, 0.0, 1.0, 0.0, 60.0, 0.0, 0.0, 1.0, 70.0, 0.0, 0.0, 0.0, 1.0,
        ];
        second.morphs[0].offsets = vec![MorphOffset {
            vertex: 1,
            delta: [1.0, 0.0, 0.0],
        }];
        scene.groups[0].meshes.push(second);
        let output = build_model(&scene.validate().unwrap(), 0, 12.5).unwrap();
        let read = read_model(&output.pmx);
        assert_eq!(read.morphs.len(), 2);
        assert_eq!(
            read.morphs[0].offsets,
            [(0, [-1.5, 3.0, 0.5]), (3, [-0.75, 0.0, 0.0])]
        );
        assert_eq!(read.morphs[1].offsets, [(5, [-0.25, 0.0, 0.0])]);
        assert_eq!(&read.indices[6..], &[7, 6, 5, 4, 5, 6]); // Negative mesh determinant cancels C reflection.
        assert_ne!(read.morphs[0].name, read.morphs[1].name);
        for (index, bound) in output.binding.morphs.iter().enumerate() {
            assert_eq!(
                (bound.mesh_index, bound.source_index, bound.pmx_index),
                (index, 0, index as u32)
            );
            assert_eq!(read.morphs[index].name, bound.name.alias);
            assert_eq!(read.morphs[index].original, "笑い😀");
            assert_eq!(
                names::encode_cp932(&bound.name.alias).unwrap(),
                bound.name.cp932
            );
            assert_eq!(bound.name.vmd_field()[14], 0);
        }
        assert!(output.name_map_tsv.contains("笑い😀"));
        assert_eq!(read.displays[1].2, [(1, 0), (1, 1)]);
    }
}
