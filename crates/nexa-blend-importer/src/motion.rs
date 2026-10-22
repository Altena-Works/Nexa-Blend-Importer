//! Model-only VMD keys from evaluated world skin matrices and the shared PMX binding.

use crate::{
    names::{self, NameAlias, NameError, NameKind},
    packet::{Matrix, ModelPacket, ValidatedPacket},
    pmx::ModelBinding,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

const IDENTITY: Matrix = [
    1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
];
const MAX_VMD_BYTES: u64 = 2 * 1024 * 1024 * 1024;
// Float32 Blender matrices can accumulate small roundoff; this is not a scale bake.
const RIGID_TOLERANCE: f64 = 1e-6;

#[derive(Clone, Debug, PartialEq)]
pub struct BoneKey {
    pub name: [u8; 15],
    pub tick: u32,
    pub translation: [f32; 3],
    /// Normalized [x,y,z,w], with consecutive signs chosen continuously per bone.
    pub rotation: [f32; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct MorphKey {
    pub name: [u8; 15],
    pub tick: u32,
    pub weight: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelMotion {
    pub bone_keys: Vec<BoneKey>,
    pub morph_keys: Vec<MorphKey>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MotionError {
    InvalidGroup,
    InvalidUnits,
    Binding(&'static str),
    Name(NameError),
    ModelName,
    Transform {
        tick: u32,
        bone: Option<usize>,
        reason: &'static str,
    },
    Morph {
        tick: u32,
        reason: &'static str,
    },
    Float32Range(&'static str),
    Keys(&'static str),
    Limit(&'static str),
    Allocation,
}

impl fmt::Display for MotionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidGroup => f.write_str("model motion group is absent"),
            Self::InvalidUnits => f.write_str("motion units must have a positive finite product"),
            Self::Binding(reason) => write!(f, "model motion binding: {reason}"),
            Self::Name(error) => write!(f, "model motion name: {error}"),
            Self::ModelName => {
                f.write_str("VMD model name must be 1..19 explicit CP932 bytes without NUL")
            }
            Self::Transform { tick, bone, reason } => write!(
                f,
                "model motion transform at tick {tick}, bone {bone:?}: {reason}"
            ),
            Self::Morph { tick, reason } => write!(f, "model morph at tick {tick}: {reason}"),
            Self::Float32Range(field) => {
                write!(f, "model motion {field} exceeds finite float32 range")
            }
            Self::Limit(field) => write!(f, "model motion limit: {field}"),
            Self::Keys(reason) => write!(f, "invalid model motion keys: {reason}"),
            Self::Allocation => f.write_str("model motion allocation failed"),
        }
    }
}
impl std::error::Error for MotionError {}
impl From<NameError> for MotionError {
    fn from(value: NameError) -> Self {
        Self::Name(value)
    }
}

fn binding_check(condition: bool, reason: &'static str) -> Result<(), MotionError> {
    if condition {
        Ok(())
    } else {
        Err(MotionError::Binding(reason))
    }
}

fn alias(name: &NameAlias, kind: NameKind) -> Result<[u8; 15], MotionError> {
    binding_check(
        name.kind == kind
            && (1..=14).contains(&name.cp932.len())
            && !name.cp932.iter().any(|v| matches!(*v, 0 | b'\n' | b'\r')),
        "invalid VMD alias",
    )?;
    binding_check(
        names::encode_cp932(&name.alias)? == name.cp932,
        "alias encoding differs",
    )?;
    Ok(name.vmd_field())
}

fn counts(ticks: usize, bones: usize, morphs: usize) -> Result<(usize, usize, usize), MotionError> {
    let bone_keys = ticks
        .checked_mul(bones)
        .ok_or(MotionError::Limit("bone key count"))?;
    let morph_keys = ticks
        .checked_mul(morphs)
        .ok_or(MotionError::Limit("morph key count"))?;
    if bone_keys > u32::MAX as usize || morph_keys > u32::MAX as usize {
        return Err(MotionError::Limit("VMD u32 key count"));
    }
    let bytes = (bone_keys as u64)
        .checked_mul(111)
        .and_then(|n| {
            (morph_keys as u64)
                .checked_mul(23)
                .and_then(|m| n.checked_add(m))
        })
        .and_then(|n| n.checked_add(74))
        .filter(|n| *n <= MAX_VMD_BYTES)
        .ok_or(MotionError::Limit("VMD bytes"))?;
    Ok((
        bone_keys,
        morph_keys,
        usize::try_from(bytes).map_err(|_| MotionError::Limit("VMD bytes"))?,
    ))
}

fn reserve<T>(count: usize) -> Result<Vec<T>, MotionError> {
    let mut result = Vec::new();
    result
        .try_reserve_exact(count)
        .map_err(|_| MotionError::Allocation)?;
    Ok(result)
}

fn to_f32<const N: usize>(values: [f64; N], field: &'static str) -> Result<[f32; N], MotionError> {
    if values
        .iter()
        .any(|v| !v.is_finite() || v.abs() > f64::from(f32::MAX))
    {
        return Err(MotionError::Float32Range(field));
    }
    Ok(values.map(|v| v as f32))
}

fn multiply(a: &Matrix, b: &Matrix) -> Matrix {
    std::array::from_fn(|i| (0..4).map(|k| a[4 * (i / 4) + k] * b[4 * k + i % 4]).sum())
}

fn affine_inverse(m: &Matrix) -> Option<Matrix> {
    let largest = (0..3)
        .flat_map(|r| (0..3).map(move |c| m[4 * r + c].abs()))
        .fold(0., f64::max);
    if !largest.is_finite() || largest == 0. {
        return None;
    }
    let a: [[f64; 3]; 3] = std::array::from_fn(|r| std::array::from_fn(|c| m[4 * r + c] / largest));
    let cofactor = [
        [
            a[1][1] * a[2][2] - a[1][2] * a[2][1],
            a[1][2] * a[2][0] - a[1][0] * a[2][2],
            a[1][0] * a[2][1] - a[1][1] * a[2][0],
        ],
        [
            a[0][2] * a[2][1] - a[0][1] * a[2][2],
            a[0][0] * a[2][2] - a[0][2] * a[2][0],
            a[0][1] * a[2][0] - a[0][0] * a[2][1],
        ],
        [
            a[0][1] * a[1][2] - a[0][2] * a[1][1],
            a[0][2] * a[1][0] - a[0][0] * a[1][2],
            a[0][0] * a[1][1] - a[0][1] * a[1][0],
        ],
    ];
    let determinant = (0..3).map(|c| a[0][c] * cofactor[0][c]).sum::<f64>();
    if determinant == 0. || !determinant.is_finite() {
        return None;
    }
    let mut inverse = IDENTITY;
    for r in 0..3 {
        for c in 0..3 {
            inverse[4 * r + c] = (cofactor[c][r] / determinant) / largest;
        }
        inverse[4 * r + 3] = -(0..3)
            .map(|c| inverse[4 * r + c] * m[4 * c + 3])
            .sum::<f64>();
    }
    inverse.iter().all(|v| v.is_finite()).then_some(inverse)
}

fn rigid_inverse(m: &Matrix) -> Matrix {
    let mut result = IDENTITY;
    for r in 0..3 {
        for c in 0..3 {
            result[4 * r + c] = m[4 * c + r];
        }
        result[4 * r + 3] = -(0..3).map(|c| m[4 * c + r] * m[4 * c + 3]).sum::<f64>();
    }
    result
}

/// Conjugate by k*C, C(x,y,z)=(x,z,y); the scale affects translation once.
fn mapped(m: &Matrix, scale: f64) -> Matrix {
    let axes = [0, 2, 1];
    let mut result = IDENTITY;
    for r in 0..3 {
        for c in 0..3 {
            result[4 * r + c] = m[4 * axes[r] + axes[c]];
        }
        result[4 * r + 3] = m[4 * axes[r] + 3] * scale;
    }
    result
}

fn proper_rigid(m: &Matrix, tick: u32, bone: Option<usize>) -> Result<(), MotionError> {
    let error = |reason| MotionError::Transform { tick, bone, reason };
    if m.iter().any(|v| !v.is_finite()) {
        return Err(error("non-finite matrix"));
    }
    if m[12..15].iter().any(|v| v.abs() > 1e-10) || (m[15] - 1.).abs() > 1e-10 {
        return Err(error("non-affine matrix"));
    }
    for c in 0..3 {
        for d in c..3 {
            let dot = (0..3).map(|r| m[4 * r + c] * m[4 * r + d]).sum::<f64>();
            if (dot - if c == d { 1. } else { 0. }).abs() > RIGID_TOLERANCE {
                return Err(error("residual scale or shear"));
            }
        }
    }
    let determinant = m[0] * (m[5] * m[10] - m[6] * m[9]) - m[1] * (m[4] * m[10] - m[6] * m[8])
        + m[2] * (m[4] * m[9] - m[5] * m[8]);
    if determinant <= 0. || (determinant - 1.).abs() > 4. * RIGID_TOLERANCE {
        return Err(error("reflection or improper rotation"));
    }
    Ok(())
}

fn quaternion(m: &Matrix) -> [f64; 4] {
    let trace = m[0] + m[5] + m[10];
    let mut q = if trace > 0. {
        let s = (trace + 1.).sqrt() * 2.;
        [
            (m[9] - m[6]) / s,
            (m[2] - m[8]) / s,
            (m[4] - m[1]) / s,
            s / 4.,
        ]
    } else if m[0] > m[5] && m[0] > m[10] {
        let s = (1. + m[0] - m[5] - m[10]).sqrt() * 2.;
        [
            s / 4.,
            (m[1] + m[4]) / s,
            (m[2] + m[8]) / s,
            (m[9] - m[6]) / s,
        ]
    } else if m[5] > m[10] {
        let s = (1. + m[5] - m[0] - m[10]).sqrt() * 2.;
        [
            (m[1] + m[4]) / s,
            s / 4.,
            (m[6] + m[9]) / s,
            (m[2] - m[8]) / s,
        ]
    } else {
        let s = (1. + m[10] - m[0] - m[5]).sqrt() * 2.;
        [
            (m[2] + m[8]) / s,
            (m[6] + m[9]) / s,
            s / 4.,
            (m[4] - m[1]) / s,
        ]
    };
    let length = q.iter().map(|v| v * v).sum::<f64>().sqrt();
    for v in &mut q {
        *v /= length;
    }
    q
}

fn validate_binding(
    group: &ModelPacket,
    group_index: usize,
    scale: f64,
    binding: &ModelBinding,
) -> Result<(Vec<[u8; 15]>, Vec<[u8; 15]>, Vec<usize>), MotionError> {
    binding_check(binding.group_index == group_index, "wrong group")?;
    binding_check(
        binding.scale_to_mmd.is_finite()
            && (binding.scale_to_mmd - scale).abs() <= scale * f64::EPSILON * 16.,
        "different units",
    )?;
    binding_check(
        binding.root.source_index.is_none()
            && binding.root.pmx_index == 0
            && binding.root.parent_pmx.is_none()
            && binding.root.rest_head == [0.; 3],
        "generated root",
    )?;
    binding_check(binding.bones.len() == group.bones.len(), "bone count")?;
    let mut bone_names = reserve(group.bones.len() + 1)?;
    let mut unique = BTreeSet::new();
    bone_names.push(alias(&binding.root.name, NameKind::Root)?);
    unique.insert(binding.root.name.cp932.as_slice());
    for (i, (bound, source)) in binding.bones.iter().zip(&group.bones).enumerate() {
        binding_check(
            bound.source_index == Some(i)
                && bound.pmx_index as usize == i + 1
                && bound.parent_pmx == Some(source.parent.map_or(0, |p| p + 1)),
            "bone hierarchy/order",
        )?;
        let expected = to_f32(
            [
                source.head_world[0] * scale,
                source.head_world[2] * scale,
                source.head_world[1] * scale,
            ],
            "rest head",
        )?;
        binding_check(
            bound
                .rest_head
                .iter()
                .zip(expected)
                .all(|(a, b)| a.is_finite() && (f64::from(*a) - f64::from(b)).abs() <= 1e-4),
            "bone rest differs",
        )?;
        binding_check(
            bound.name.original == source.name && unique.insert(bound.name.cp932.as_slice()),
            "bone alias correspondence/collision",
        )?;
        bone_names.push(alias(&bound.name, NameKind::Bone)?);
    }
    let mut offsets = reserve(group.meshes.len() + 1)?;
    offsets.push(0_usize);
    for mesh in &group.meshes {
        offsets.push(
            offsets
                .last()
                .unwrap()
                .checked_add(mesh.morphs.len())
                .ok_or(MotionError::Limit("morph count"))?,
        );
    }
    binding_check(
        binding.morphs.len() == *offsets.last().unwrap(),
        "morph count",
    )?;
    let mut morph_names = reserve(binding.morphs.len())?;
    unique.clear();
    for (mesh_index, mesh) in group.meshes.iter().enumerate() {
        for (source_index, source) in mesh.morphs.iter().enumerate() {
            let flat = offsets[mesh_index] + source_index;
            let bound = &binding.morphs[flat];
            binding_check(
                bound.mesh_index == mesh_index
                    && bound.source_index == source_index
                    && bound.pmx_index as usize == flat,
                "morph mesh/source order",
            )?;
            binding_check(
                bound.name.original == source.name && unique.insert(bound.name.cp932.as_slice()),
                "morph alias correspondence/collision",
            )?;
            morph_names.push(alias(&bound.name, NameKind::Morph)?);
        }
    }
    Ok((bone_names, morph_names, offsets))
}

/// Keys are tick-major, with generated root then source bones, and stable mesh/morph order.
/// Static samples still yield keys; deciding whether to publish motion belongs to the planner.
pub fn solve_model_motion(
    packet: &ValidatedPacket<'_>,
    group_index: usize,
    units_per_meter: f64,
    binding: &ModelBinding,
) -> Result<ModelMotion, MotionError> {
    let scene = packet.packet();
    let group = scene
        .groups
        .get(group_index)
        .ok_or(MotionError::InvalidGroup)?;
    let scale = units_per_meter * scene.unit_scale;
    if !units_per_meter.is_finite() || units_per_meter <= 0. || !scale.is_finite() || scale <= 0. {
        return Err(MotionError::InvalidUnits);
    }
    let (bone_count, morph_count, _) = counts(
        group.poses.len(),
        group.bones.len() + 1,
        binding.morphs.len(),
    )?;
    let (bone_names, morph_names, offsets) = validate_binding(group, group_index, scale, binding)?;
    let reference_inverse =
        affine_inverse(&group.reference_object_world).ok_or(MotionError::Transform {
            tick: 0,
            bone: None,
            reason: "singular reference object matrix",
        })?;
    let mut result = ModelMotion {
        bone_keys: reserve(bone_count)?,
        morph_keys: reserve(morph_count)?,
    };
    let mut previous = reserve::<[f64; 4]>(bone_names.len())?;
    previous.resize(bone_names.len(), [0., 0., 0., 1.]);
    let mut frames = reserve::<Matrix>(bone_names.len())?;
    frames.resize(bone_names.len(), IDENTITY);
    let mut morph_values = reserve::<f64>(morph_names.len())?;
    morph_values.resize(morph_names.len(), 0.);
    for (sample_index, pose) in group.poses.iter().enumerate() {
        let root_skin = mapped(&multiply(&pose.object_world, &reference_inverse), scale);
        proper_rigid(&root_skin, pose.tick, None)?;
        frames[0] = root_skin;
        for (source_index, skin) in pose.bone_skin_world.iter().enumerate() {
            let skin = mapped(skin, scale);
            proper_rigid(&skin, pose.tick, Some(source_index))?;
            let rest = binding.bones[source_index].rest_head.map(f64::from);
            let mut frame = skin;
            for r in 0..3 {
                frame[4 * r + 3] += (0..3).map(|c| skin[4 * r + c] * rest[c]).sum::<f64>();
            }
            frames[source_index + 1] = frame;
        }
        for (pmx_index, bound) in std::iter::once(&binding.root)
            .chain(&binding.bones)
            .enumerate()
        {
            let rest = bound.rest_head.map(f64::from);
            let (local, parent_rest) = if let Some(parent) = bound.parent_pmx {
                let parent = parent as usize;
                let parent_rest = if parent == 0 {
                    [0.; 3]
                } else {
                    binding.bones[parent - 1].rest_head.map(f64::from)
                };
                (
                    multiply(&rigid_inverse(&frames[parent]), &frames[pmx_index]),
                    parent_rest,
                )
            } else {
                (frames[pmx_index], [0.; 3])
            };
            proper_rigid(&local, pose.tick, pmx_index.checked_sub(1))?;
            let delta = std::array::from_fn(|r| local[4 * r + 3] - (rest[r] - parent_rest[r]));
            let mut q = quaternion(&local);
            let dot = q
                .iter()
                .zip(previous[pmx_index])
                .map(|(a, b)| a * b)
                .sum::<f64>();
            if dot < 0.
                || (sample_index == 0
                    && q[3] == 0.
                    && q[..3].iter().find(|v| **v != 0.).is_some_and(|v| *v < 0.))
            {
                q = q.map(|v| -v);
            }
            previous[pmx_index] = q;
            result.bone_keys.push(BoneKey {
                name: bone_names[pmx_index],
                tick: pose.tick,
                translation: to_f32(delta, "translation")?,
                rotation: to_f32(q, "quaternion")?,
            });
        }
        if pose.morph_weights.len() != morph_names.len() {
            return Err(MotionError::Morph {
                tick: pose.tick,
                reason: "missing evaluated morph weight",
            });
        }
        for weight in &pose.morph_weights {
            morph_values[offsets[weight.mesh as usize] + weight.morph as usize] = weight.weight;
        }
        for (name, weight) in morph_names.iter().zip(&morph_values) {
            result.morph_keys.push(MorphKey {
                name: *name,
                tick: pose.tick,
                weight: to_f32([*weight], "morph weight")?[0],
            });
        }
    }
    Ok(result)
}

/// Explicit model header bytes avoid fallback or truncation of an unencodable/long name.
pub fn generate_model_vmd(
    packet: &ValidatedPacket<'_>,
    group_index: usize,
    units_per_meter: f64,
    binding: &ModelBinding,
    model_name_cp932: &[u8],
) -> Result<Vec<u8>, MotionError> {
    model_header(model_name_cp932)?;
    let keys = solve_model_motion(packet, group_index, units_per_meter, binding)?;
    write_model_vmd(&keys, model_name_cp932)
}

fn model_header(model_name_cp932: &[u8]) -> Result<[u8; 20], MotionError> {
    if !(1..=19).contains(&model_name_cp932.len()) || model_name_cp932.contains(&0) {
        return Err(MotionError::ModelName);
    }
    names::decode_cp932(model_name_cp932)?;
    let mut model = [0_u8; 20];
    model[..model_name_cp932.len()].copy_from_slice(model_name_cp932);
    Ok(model)
}

fn key_name(field: &[u8; 15]) -> Result<(), MotionError> {
    let end = field
        .iter()
        .position(|b| *b == 0)
        .ok_or(MotionError::Keys("unterminated name"))?;
    if end == 0
        || field[end..].iter().any(|b| *b != 0)
        || field[..end].iter().any(|b| *b < 0x20 || *b == 0x7f)
    {
        return Err(MotionError::Keys("invalid name"));
    }
    names::decode_cp932(&field[..end])?;
    Ok(())
}

/// Serializes already-solved keys without cloning or recalculating the pose arrays.
pub fn write_model_vmd(
    keys: &ModelMotion,
    model_name_cp932: &[u8],
) -> Result<Vec<u8>, MotionError> {
    let model = model_header(model_name_cp932)?;
    let (_, _, size) = counts(1, keys.bone_keys.len(), keys.morph_keys.len())?;
    let mut bones = BTreeMap::<[u8; 15], (u32, [f32; 4])>::new();
    for key in &keys.bone_keys {
        if key.tick as usize >= crate::packet::MAX_TICKS
            || key
                .translation
                .iter()
                .chain(&key.rotation)
                .any(|v| !v.is_finite())
            || (key
                .rotation
                .iter()
                .map(|v| f64::from(*v).powi(2))
                .sum::<f64>()
                - 1.)
                .abs()
                > 1e-6
        {
            return Err(MotionError::Keys(
                "tick, translation or normalized rotation",
            ));
        }
        if let Some((previous_tick, previous_q)) = bones.get(&key.name) {
            if key.tick <= *previous_tick
                || key
                    .rotation
                    .iter()
                    .zip(previous_q)
                    .map(|(a, b)| a * b)
                    .sum::<f32>()
                    < -1e-6
            {
                return Err(MotionError::Keys(
                    "bone tick order or quaternion sign continuity",
                ));
            }
        } else {
            key_name(&key.name)?;
        }
        bones.insert(key.name, (key.tick, key.rotation));
    }
    let mut morphs = BTreeMap::<[u8; 15], u32>::new();
    for key in &keys.morph_keys {
        if key.tick as usize >= crate::packet::MAX_TICKS || !key.weight.is_finite() {
            return Err(MotionError::Keys("morph tick or weight"));
        }
        if let Some(previous) = morphs.get(&key.name) {
            if key.tick <= *previous {
                return Err(MotionError::Keys("morph tick order"));
            }
        } else {
            key_name(&key.name)?;
        }
        morphs.insert(key.name, key.tick);
    }
    let mut bytes = reserve(size)?;
    let mut header = [0_u8; 30];
    header[..25].copy_from_slice(b"Vocaloid Motion Data 0002");
    bytes.extend(header);
    bytes.extend(model);
    bytes.extend((keys.bone_keys.len() as u32).to_le_bytes());
    for key in &keys.bone_keys {
        bytes.extend(key.name);
        bytes.extend(key.tick.to_le_bytes());
        for value in key.translation.into_iter().chain(key.rotation) {
            bytes.extend(value.to_le_bytes());
        }
        // Plane-major x1,y1,x2,y2 for X/Y/Z/rotation; x1=y1 and x2=y2 are linear.
        for _ in 0..4 {
            bytes.extend([
                20, 20, 20, 20, 20, 20, 20, 20, 107, 107, 107, 107, 107, 107, 107, 107,
            ]);
        }
    }
    bytes.extend((keys.morph_keys.len() as u32).to_le_bytes());
    for key in &keys.morph_keys {
        bytes.extend(key.name);
        bytes.extend(key.tick.to_le_bytes());
        bytes.extend(key.weight.to_le_bytes());
    }
    // Camera, light, self-shadow and model-display/IK counts.
    for _ in 0..4 {
        bytes.extend(0_u32.to_le_bytes());
    }
    debug_assert_eq!(bytes.len(), size);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::*;

    fn translate(p: [f64; 3]) -> Matrix {
        let mut m = IDENTITY;
        for r in 0..3 {
            m[4 * r + 3] = p[r];
        }
        m
    }
    fn rotate_z(angle: f64) -> Matrix {
        let (s, c) = angle.sin_cos();
        [c, -s, 0., 0., s, c, 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.]
    }
    fn about(rest: [f64; 3], delta: [f64; 3], rotation: Matrix) -> Matrix {
        let mut m = rotation;
        for r in 0..3 {
            m[4 * r + 3] =
                rest[r] + delta[r] - (0..3).map(|c| rotation[4 * r + c] * rest[c]).sum::<f64>();
        }
        m
    }
    fn fixture() -> ScenePacket {
        let bones = vec![
            BonePacket {
                source_id: "parent".into(),
                name: "親".into(),
                parent: None,
                rest_world: multiply(&translate([2., 3., 4.]), &rotate_z(0.37)),
                head_world: [2., 3., 4.],
                tail_world: [2., 4., 4.],
            },
            BonePacket {
                source_id: "child".into(),
                name: "Child".into(),
                parent: Some(0),
                rest_world: multiply(&translate([5., 6., 7.]), &rotate_z(-0.61)),
                head_world: [5., 6., 7.],
                tail_world: [5., 7., 7.],
            },
        ];
        let meshes = (0..2)
            .map(|index| MeshPacket {
                source_id: format!("mesh{index}"),
                name: format!("Mesh{index}"),
                world_from_mesh: IDENTITY,
                vertices: vec![
                    Vertex {
                        position: [0.; 3],
                        normal: [0., 0., 1.],
                        uv: [0.; 2],
                        weights: vec![]
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
                    source_id: format!("shape{index}"),
                    name: "Smile".into(),
                    offsets: vec![MorphOffset {
                        vertex: 0,
                        delta: [0., 0.1, 0.],
                    }],
                }],
            })
            .collect();
        let reference = translate([10., -3., 7.]);
        let root_skin = [
            0., -1., 0., 4., 1., 0., 0., 5., 0., 0., 1., 6., 0., 0., 0., 1.,
        ];
        let parent_local = about(
            [2., 3., 4.],
            [1., 2., 3.],
            [
                0., 0., 1., 0., 0., 1., 0., 0., -1., 0., 0., 0., 0., 0., 0., 1.,
            ],
        );
        let child_local = about(
            [5., 6., 7.],
            [-2., 1., 4.],
            [
                0., -1., 0., 0., 1., 0., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
            ],
        );
        let parent_skin = multiply(&root_skin, &parent_local);
        let child_skin = multiply(&parent_skin, &child_local);
        ScenePacket {
            scene_name: "Scene".into(),
            reference_frame: 1.,
            frame_start: 1.,
            frame_end: 4.,
            frames_per_second: 30.,
            unit_scale: 1.,
            render_size: [512, 512],
            images: vec![],
            cameras: vec![],
            groups: vec![ModelPacket {
                source_id: "rig".into(),
                name: "Model".into(),
                reference_object_world: reference,
                meshes,
                bones,
                poses: vec![
                    PoseSample {
                        tick: 0,
                        source_frame: 1.,
                        object_world: reference,
                        bone_skin_world: vec![IDENTITY; 2],
                        morph_weights: vec![
                            MorphWeight {
                                mesh: 1,
                                morph: 0,
                                weight: -0.25,
                            },
                            MorphWeight {
                                mesh: 0,
                                morph: 0,
                                weight: 0.5,
                            },
                        ],
                    },
                    PoseSample {
                        tick: 1,
                        source_frame: 2.,
                        object_world: multiply(&root_skin, &reference),
                        bone_skin_world: vec![parent_skin, child_skin],
                        morph_weights: vec![
                            MorphWeight {
                                mesh: 0,
                                morph: 0,
                                weight: 0.75,
                            },
                            MorphWeight {
                                mesh: 1,
                                morph: 0,
                                weight: 0.125,
                            },
                        ],
                    },
                ],
            }],
        }
    }
    fn near<const N: usize>(actual: [f32; N], expected: [f64; N]) {
        assert!(
            actual
                .iter()
                .zip(expected)
                .all(|(a, e)| (f64::from(*a) - e).abs() <= 1e-6),
            "{actual:?} != {expected:?}"
        );
    }

    #[test]
    fn key_scene_rest_parent_root_mesh_morphs_and_quaternion_continuity() {
        let scene = fixture();
        let checked = scene.validate().unwrap();
        let binding = crate::pmx::build_binding(&checked, 0, 2.).unwrap();
        let keys = solve_model_motion(&checked, 0, 2., &binding).unwrap();
        assert_eq!((keys.bone_keys.len(), keys.morph_keys.len()), (6, 4));
        for key in &keys.bone_keys[..3] {
            near(key.translation, [0.; 3]);
            near(key.rotation, [0., 0., 0., 1.]);
        }
        let h = std::f64::consts::FRAC_1_SQRT_2;
        for (key, (delta, q)) in keys.bone_keys[3..].iter().zip([
            ([8., 12., 10.], [0., -h, 0., h]),
            ([2., 6., 4.], [0., 0., -h, h]),
            ([-4., 8., 2.], [0., -h, 0., h]),
        ]) {
            near(key.translation, delta);
            near(key.rotation, q);
        }
        assert_eq!(
            keys.morph_keys.iter().map(|k| k.weight).collect::<Vec<_>>(),
            [0.5, -0.25, 0.75, 0.125]
        );
        assert_ne!(keys.morph_keys[0].name, keys.morph_keys[1].name);
        assert_eq!(keys.morph_keys[0].name, binding.morphs[0].name.vmd_field());
        assert_eq!(keys.morph_keys[1].name, binding.morphs[1].name.vmd_field());

        let mut turning = fixture();
        let base = turning.groups[0].poses[0].clone();
        let reference = turning.groups[0].reference_object_world;
        turning.groups[0].poses = [170., 190., 270.]
            .into_iter()
            .enumerate()
            .map(|(index, angle)| {
                let skin = rotate_z(f64::to_radians(angle));
                PoseSample {
                    tick: index as u32,
                    source_frame: index as f64 + 1.,
                    object_world: multiply(&skin, &reference),
                    bone_skin_world: vec![skin; 2],
                    ..base.clone()
                }
            })
            .collect();
        let checked = turning.validate().unwrap();
        let binding = crate::pmx::build_binding(&checked, 0, 2.).unwrap();
        let keys = solve_model_motion(&checked, 0, 2., &binding).unwrap();
        let root_keys: Vec<_> = keys.bone_keys.iter().step_by(3).collect();
        for (key, degrees) in root_keys.iter().zip([170_f64, 190., 270.]) {
            let half = degrees.to_radians() / 2.;
            near(key.rotation, [0., -half.sin(), 0., half.cos()]);
            assert!(
                (key.rotation
                    .iter()
                    .map(|v| f64::from(*v).powi(2))
                    .sum::<f64>()
                    - 1.)
                    .abs()
                    < 1e-6
            );
        }
        for pair in root_keys.windows(2) {
            assert!(
                pair[0]
                    .rotation
                    .iter()
                    .zip(pair[1].rotation)
                    .map(|(a, b)| a * b)
                    .sum::<f32>()
                    > 0.
            );
        }
    }

    #[test]
    fn wire_readback_has_shared_names_linear_keys_and_only_model_tracks() {
        let scene = fixture();
        let checked = scene.validate().unwrap();
        let model = crate::pmx::build_model(&checked, 0, 2.).unwrap();
        let bytes = generate_model_vmd(
            &checked,
            0,
            2.,
            &model.binding,
            &model.binding.model_name.cp932,
        )
        .unwrap();
        let solved = solve_model_motion(&checked, 0, 2., &model.binding).unwrap();
        assert_eq!(
            write_model_vmd(&solved, &model.binding.model_name.cp932).unwrap(),
            bytes
        );
        assert_eq!(bytes.len(), 832);
        assert_eq!(&bytes[..25], b"Vocaloid Motion Data 0002");
        assert!(bytes[25..30].iter().all(|v| *v == 0));
        assert_eq!(&bytes[30..35], b"Model");
        assert!(bytes[35..50].iter().all(|v| *v == 0));
        let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let float = |at: usize| f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        assert_eq!(word(50), 6);
        let mut cursor = 54;
        let bone_names: Vec<_> = std::iter::once(&model.binding.root)
            .chain(&model.binding.bones)
            .map(|b| b.name.vmd_field())
            .collect();
        for index in 0..6 {
            assert_eq!(&bytes[cursor..cursor + 15], &bone_names[index % 3]);
            assert_eq!(word(cursor + 15), (index / 3) as u32);
            assert!(bytes[cursor..cursor + 15].contains(&0));
            if index == 3 {
                near(
                    std::array::from_fn(|i| float(cursor + 19 + 4 * i)),
                    [8., 12., 10.],
                );
            }
            let interpolation = &bytes[cursor + 47..cursor + 111];
            for block in interpolation.chunks_exact(16) {
                for channel in 0..4 {
                    assert_eq!(
                        [
                            block[channel],
                            block[4 + channel],
                            block[8 + channel],
                            block[12 + channel]
                        ],
                        [20, 20, 107, 107]
                    );
                }
            }
            // Nexa's existing loader view also sees x1=y1, x2=y2, with physics marker off.
            for channel in 0..4 {
                let control: [u8; 4] =
                    std::array::from_fn(|plane| interpolation[16 * plane + 4 * channel]);
                assert_eq!(control[0], control[1]);
                assert_eq!(control[2], control[3]);
            }
            assert_ne!([interpolation[2], interpolation[3]], [0x63, 0x0f]);
            cursor += 111;
        }
        assert_eq!(word(cursor), 4);
        cursor += 4;
        for (index, expected) in [0.5, -0.25, 0.75, 0.125].into_iter().enumerate() {
            assert_eq!(
                &bytes[cursor..cursor + 15],
                &model.binding.morphs[index % 2].name.vmd_field()
            );
            assert_eq!(word(cursor + 15), (index / 2) as u32);
            assert_eq!(float(cursor + 19), expected);
            cursor += 23;
        }
        assert_eq!(&bytes[cursor..], &[0; 16]);
    }

    #[test]
    fn fail_stop_rejects_unsupported_transforms_names_maps_and_limits() {
        for matrix in [
            [
                1.1, 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
            ],
            [
                1., 0.25, 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
            ],
            [
                -1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
            ],
        ] {
            let mut scene = fixture();
            scene.groups[0].poses[0].bone_skin_world[0] = matrix;
            let checked = scene.validate().unwrap();
            let binding = crate::pmx::build_binding(&checked, 0, 2.).unwrap();
            assert!(matches!(
                solve_model_motion(&checked, 0, 2., &binding),
                Err(MotionError::Transform { .. })
            ));
        }
        let scene = fixture();
        let checked = scene.validate().unwrap();
        let binding = crate::pmx::build_binding(&checked, 0, 2.).unwrap();
        for name in [
            &b""[..],
            &b"12345678901234567890"[..],
            &b"Bad\0Name"[..],
            &b"\x81"[..],
        ] {
            assert!(generate_model_vmd(&checked, 0, 2., &binding, name).is_err());
        }
        let mut bad_binding = binding.clone();
        bad_binding.root.name.cp932 = vec![b'A'; 15];
        assert!(matches!(
            solve_model_motion(&checked, 0, 2., &bad_binding),
            Err(MotionError::Binding(_))
        ));
        let mut bad_binding = binding.clone();
        bad_binding.morphs.swap(0, 1);
        assert!(matches!(
            solve_model_motion(&checked, 0, 2., &bad_binding),
            Err(MotionError::Binding(_))
        ));
        assert!(matches!(
            solve_model_motion(&checked, 0, 1., &binding),
            Err(MotionError::Binding(_))
        ));
        assert!(matches!(
            solve_model_motion(&checked, 0, f64::MAX, &binding),
            Err(MotionError::Binding(_)) | Err(MotionError::Float32Range(_))
        ));
        assert!(matches!(
            counts(usize::MAX, 2, 0),
            Err(MotionError::Limit(_))
        ));
        assert!(matches!(
            counts(108_001, 65_535, 0),
            Err(MotionError::Limit(_))
        ));
        let mut missing = fixture();
        missing.groups[0].poses[0].morph_weights.pop();
        let checked = missing.validate().unwrap();
        let binding = crate::pmx::build_binding(&checked, 0, 2.).unwrap();
        assert!(matches!(
            solve_model_motion(&checked, 0, 2., &binding),
            Err(MotionError::Morph { .. })
        ));
        let mut overflow = fixture();
        overflow.groups[0].poses[0].morph_weights[0].weight = f64::MAX;
        let checked = overflow.validate().unwrap();
        let binding = crate::pmx::build_binding(&checked, 0, 2.).unwrap();
        assert!(matches!(
            solve_model_motion(&checked, 0, 2., &binding),
            Err(MotionError::Float32Range(_))
        ));
        let mut object_scale = fixture();
        object_scale.groups[0].poses[0].object_world[0] = 2.;
        let checked = object_scale.validate().unwrap();
        let binding = crate::pmx::build_binding(&checked, 0, 2.).unwrap();
        assert!(matches!(
            solve_model_motion(&checked, 0, 2., &binding),
            Err(MotionError::Transform { bone: None, .. })
        ));
        let scene = fixture();
        let checked = scene.validate().unwrap();
        let binding = crate::pmx::build_binding(&checked, 0, 2.).unwrap();
        let solved = solve_model_motion(&checked, 0, 2., &binding).unwrap();
        let mut malformed = solved.clone();
        malformed.bone_keys[0].rotation = [0., 0., 0., 2.];
        assert!(matches!(
            write_model_vmd(&malformed, b"Model"),
            Err(MotionError::Keys(_))
        ));
        let mut malformed = solved.clone();
        malformed.bone_keys[0].name = [b'A'; 15];
        assert!(matches!(
            write_model_vmd(&malformed, b"Model"),
            Err(MotionError::Keys(_))
        ));
        let mut malformed = solved.clone();
        malformed.bone_keys.push(solved.bone_keys[0].clone());
        assert!(matches!(
            write_model_vmd(&malformed, b"Model"),
            Err(MotionError::Keys(_))
        ));
        let mut malformed = solved;
        malformed.morph_keys[0].weight = f32::NAN;
        assert!(matches!(
            write_model_vmd(&malformed, b"Model"),
            Err(MotionError::Keys(_))
        ));
    }
}
