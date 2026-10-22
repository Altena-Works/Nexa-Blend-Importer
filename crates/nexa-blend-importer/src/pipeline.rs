//! Selected components share one evaluated scene and one new-folder publication.
use crate::{camera, existing_pmx, motion, packet::{self, ScenePacket, ValidatedPacket}, pmx,
    source::{self, SourceOptions}, transaction::{ArtifactName, OutputTransaction}};
use std::{collections::BTreeSet, path::PathBuf, sync::atomic::{AtomicBool, Ordering}};

#[derive(Clone, Debug)]
pub struct Options {
    pub source: SourceOptions,
    pub output_parent: PathBuf,
    pub allow_fov_approximation: bool,
}

pub struct ImportResult {
    pub directory: PathBuf,
    /// These generated paths are in the required standard MMD loading order.
    pub models: Vec<PathBuf>,
    pub motions: Vec<(PathBuf, PathBuf)>,
    pub camera: Option<PathBuf>,
    pub notices: Vec<ImportNotice>,
}

pub enum ImportNotice { NoModelMotion(String), DiffuseMaterials }
impl ImportNotice {
    pub fn text(&self, language: crate::Language) -> String {
        match self {
            Self::NoModelMotion(name) => format!("{name}: {}", language.choose(
                "基準姿勢からのモデル動作がないため、モーションVMDは生成しませんでした。",
                "No rest-relative model motion; no model-motion VMD was generated.")),
            Self::DiffuseMaterials => language.choose(
                "材質は拡散色と画像へ変換します。PBRシェーダーの見た目は再現しません。",
                "Materials use diffuse color and images; PBR shader appearance is not reproduced.").into(),
        }
    }
}

fn cancelled(flag: &AtomicBool) -> Result<(), String> {
    if flag.load(Ordering::Acquire) { Err("conversion cancelled".into()) } else { Ok(()) }
}

pub fn prepare(options: &Options, flag: &AtomicBool) -> Result<ScenePacket, String> {
    source::import_scene(&options.source, flag).map_err(|e| e.to_string())
}

fn matrix(flat: packet::Matrix) -> [[f64; 4]; 4] {
    std::array::from_fn(|r| std::array::from_fn(|c| flat[4*r+c]))
}

/// Reuses the qualified camera conversion; unrelated legacy carrier metadata is not published.
fn camera_bytes(packet: &ScenePacket, units: f64, approximate: bool) -> Result<Vec<u8>, String> {
    if packet.cameras.is_empty() { return Err("the selected scene has no camera samples".into()); }
    let mut keys = Vec::new();
    keys.try_reserve_exact(packet.cameras.len()).map_err(|_| "camera allocation failed")?;
    let mut previous = None;
    for sample in &packet.cameras {
        let frame = sample.source_frame.floor();
        let carrier = camera::CameraSample {
            blender_version: [0, 0, 0], scene_name: packet.scene_name.clone(), view_layer_name: String::new(),
            camera_name: sample.name.clone(), frame: frame as i32, subframe: sample.source_frame-frame,
            evaluated_frame: sample.source_frame, frame_start: packet.frame_start.floor() as i32,
            frame_end: packet.frame_end.ceil() as i32, fps: 30, fps_base: 30.0/packet.frames_per_second,
            frame_map_old: 1, frame_map_new: 1,
            kind: match sample.projection { packet::Projection::Perspective => camera::CameraKind::Perspective,
                packet::Projection::Orthographic => camera::CameraKind::Orthographic },
            sensor_fit: match sample.sensor_fit { packet::SensorFit::Auto => camera::SensorFit::Auto,
                packet::SensorFit::Horizontal => camera::SensorFit::Horizontal,
                packet::SensorFit::Vertical => camera::SensorFit::Vertical },
            lens_mm: sample.lens_mm, sensor_width_mm: sample.sensor_mm[0], sensor_height_mm: sample.sensor_mm[1],
            shift_x: sample.shift[0], shift_y: sample.shift[1], ortho_scale: sample.ortho_scale,
            clip_start: sample.clip[0], clip_end: sample.clip[1], unit_scale: packet.unit_scale,
            resolution: packet.render_size, resolution_percentage: 100, pixel_aspect: sample.pixel_aspect,
            matrix_world: matrix(sample.world_from_camera), matrix_projection: matrix(sample.projection_matrix),
        };
        let converted = camera::convert::convert_sample(&carrier, sample.tick,
            camera::convert::ConvertOptions { units_per_meter: units, viewport: packet.render_size,
                allow_fov_approximation: approximate }, previous)
            .map_err(|e| format!("camera {} at tick {}: {e}", sample.name, sample.tick))?;
        previous = Some(converted.rotation);
        keys.push(converted.key);
    }
    let mut bytes = Vec::new();
    camera::vmd::write_camera_vmd(&mut bytes, &keys).map_err(|e| e.to_string())?;
    Ok(bytes)
}

fn needs_motion(keys: &motion::ModelMotion) -> bool {
    keys.bone_keys.iter().any(|key| key.translation.iter().any(|v| v.abs() > 1e-6)
        || key.rotation[..3].iter().any(|v| v.abs() > 1e-7) || (key.rotation[3].abs()-1.0).abs() > 1e-7)
        || keys.morph_keys.iter().any(|key| key.weight.abs() > 1e-6)
}

/// Existing PMX selections are ordered by the stable model-group order from the extractor.
/// This function publishes only after every selected component has been generated successfully.
pub fn publish(packet: &ScenePacket, options: &Options, targets: &[PathBuf], flag: &AtomicBool)
    -> Result<ImportResult, String>
{
    let selection = options.source.request.selection;
    selection.validate().map_err(|e| e.to_string())?;
    let validated: ValidatedPacket<'_> = packet.validate().map_err(|e| e.to_string())?;
    let model_work = selection.model || selection.animation_only();
    if model_work && packet.groups.is_empty() { return Err("the selected scene has no model groups".into()); }
    if selection.animation_only() && targets.len() != packet.groups.len() {
        return Err("one existing PMX is required for each source model group".into());
    }
    if !selection.animation_only() && !targets.is_empty() { return Err("existing PMX targets are only used for animation-only import".into()); }
    cancelled(flag)?;
    let mut transaction = OutputTransaction::begin(&options.output_parent).map_err(|e| e.to_string())?;
    let mut models = Vec::new(); let mut motions = Vec::new(); let mut notices = Vec::new();
    let mut identities = BTreeSet::new(); let mut name_map = String::new();
    if model_work {
        for (index, group) in packet.groups.iter().enumerate() {
            cancelled(flag)?;
            let ordinal = u16::try_from(index+1).map_err(|_| "too many model groups")?;
            let model = if selection.model { pmx::build_model(&validated, index, options.source.request.units_per_meter) }
                else { pmx::build_match_reference(&validated, index, options.source.request.units_per_meter) }
                .map_err(|e| format!("model {}: {e}", group.name))?;
            let generated_pmx = ArtifactName::ModelPmx(ordinal).relative_path().map_err(|e| e.to_string())?;
            let mut model_name = model.binding.model_name.cp932.clone();
            let target = if selection.animation_only() {
                let matched = existing_pmx::match_existing(&targets[index], &model.pmx)
                    .map_err(|e| format!("model {}: {e}", group.name))?;
                if !identities.insert(matched.file_identity) { return Err("the same existing PMX was assigned to multiple groups".into()); }
                model_name = matched.model_name_cp932;
                matched.canonical_path
            } else { generated_pmx.clone() };
            if selection.model {
                transaction.write_artifact(ArtifactName::ModelPmx(ordinal), &model.pmx).map_err(|e| e.to_string())?;
                models.push(generated_pmx);
            }
            if selection.animation {
                if group.poses.is_empty() { return Err(format!("model {} has no evaluated poses", group.name)); }
                let keys = motion::solve_model_motion(&validated, index, options.source.request.units_per_meter, &model.binding)
                    .map_err(|e| format!("motion {}: {e}", group.name))?;
                if needs_motion(&keys) {
                    let bytes = motion::write_model_vmd(&keys, &model_name).map_err(|e| e.to_string())?;
                    transaction.write_artifact(ArtifactName::ModelMotion(ordinal), &bytes).map_err(|e| e.to_string())?;
                    motions.push((target, ArtifactName::ModelMotion(ordinal).relative_path().map_err(|e| e.to_string())?));
                } else { notices.push(ImportNotice::NoModelMotion(group.name.clone())); }
            }
            name_map.push_str(&format!("# model_{ordinal:04}\n"));
            name_map.push_str(&model.name_map_tsv);
            if !name_map.ends_with('\n') { name_map.push('\n'); }
        }
    }
    if selection.model {
        for (index, image) in packet.images.iter().enumerate() {
            cancelled(flag)?;
            transaction.write_artifact(ArtifactName::TexturePng(u16::try_from(index+1).map_err(|_| "too many images")?), &image.png)
                .map_err(|e| e.to_string())?;
        }
        notices.push(ImportNotice::DiffuseMaterials);
    }
    let camera = if selection.camera {
        cancelled(flag)?;
        let bytes = camera_bytes(packet, options.source.request.units_per_meter, options.allow_fov_approximation)?;
        transaction.write_artifact(ArtifactName::CameraVmd, &bytes).map_err(|e| e.to_string())?;
        Some(PathBuf::from("camera.vmd"))
    } else { None };
    if !name_map.is_empty() { transaction.write_artifact(ArtifactName::NameMap, name_map.as_bytes()).map_err(|e| e.to_string())?; }
    cancelled(flag)?;
    let directory = transaction.publish().map_err(|e| e.to_string())?;
    Ok(ImportResult { directory, models, motions, camera, notices })
}

pub fn run(options: &Options, targets: &[PathBuf], flag: &AtomicBool) -> Result<ImportResult, String> {
    let packet = prepare(options, flag)?;
    publish(&packet, options, targets, flag)
}
