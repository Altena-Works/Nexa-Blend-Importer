use crate::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component as PathComponent, PathBuf},
};

fn valid_object(object: &SourceObject, ids: &mut BTreeSet<String>) -> Result<(), ImportError> {
    if object.id.is_empty() || object.name.is_empty() {
        return Err(ImportError::InvalidInventory {
            field: "object.id/name",
            target: object.name.clone(),
        });
    }
    if !ids.insert(object.id.clone()) {
        return Err(ImportError::DuplicateObjectId(object.id.clone()));
    }
    Ok(())
}

fn validate_inventory(scene: &SceneInventory) -> Result<(), ImportError> {
    if scene.scene_name.is_empty() {
        return Err(ImportError::InvalidInventory {
            field: "scene_name",
            target: String::new(),
        });
    }
    let mut ids = BTreeSet::new();
    for armature in &scene.armatures {
        valid_object(&armature.object, &mut ids)?;
    }
    for mesh in &scene.meshes {
        valid_object(&mesh.object, &mut ids)?;
    }
    if let Some(camera) = &scene.active_camera {
        valid_object(camera, &mut ids)?;
    }
    let mut image_ids = BTreeSet::new();
    for image in &scene.images {
        valid_object(image, &mut image_ids)?;
    }
    Ok(())
}

fn merged_motion(a: MotionState, b: MotionState) -> MotionState {
    match (a, b) {
        (MotionState::Unknown, _) | (_, MotionState::Unknown) => MotionState::Unknown,
        (MotionState::Animated, _) | (_, MotionState::Animated) => MotionState::Animated,
        _ => MotionState::Static,
    }
}

/// Model mode groups render-enabled meshes. Animation-only also includes empty armatures.
pub fn discover_model_groups(
    scene: &SceneInventory,
    selection: Selection,
) -> Result<Vec<ModelGroup>, ImportError> {
    selection.validate()?;
    validate_inventory(scene)?;
    if !selection.model && !selection.animation_only() {
        return Ok(Vec::new());
    }
    let armatures: BTreeMap<_, _> = scene
        .armatures
        .iter()
        .map(|armature| (armature.object.id.as_str(), armature))
        .collect();
    let mut groups = BTreeMap::<String, ModelGroup>::new();
    if selection.animation_only() {
        for armature in &scene.armatures {
            groups.insert(
                armature.object.id.clone(),
                ModelGroup {
                    id: armature.object.id.clone(),
                    name: armature.object.name.clone(),
                    source: GroupSource::Armature,
                    mesh_ids: Vec::new(),
                    motion: armature.motion,
                },
            );
        }
    }
    for mesh in scene.meshes.iter().filter(|mesh| mesh.render_enabled) {
        if let Some(id) = &mesh.armature_id {
            let armature =
                armatures
                    .get(id.as_str())
                    .ok_or_else(|| ImportError::MissingArmature {
                        mesh: mesh.object.name.clone(),
                        armature_id: id.clone(),
                    })?;
            let group = groups.entry(id.clone()).or_insert_with(|| ModelGroup {
                id: id.clone(),
                name: armature.object.name.clone(),
                source: GroupSource::Armature,
                mesh_ids: Vec::new(),
                motion: armature.motion,
            });
            group.mesh_ids.push(mesh.object.id.clone());
            group.motion = merged_motion(group.motion, mesh.motion);
        } else {
            groups.insert(
                mesh.object.id.clone(),
                ModelGroup {
                    id: mesh.object.id.clone(),
                    name: mesh.object.name.clone(),
                    source: GroupSource::IndependentMesh,
                    mesh_ids: vec![mesh.object.id.clone()],
                    motion: mesh.motion,
                },
            );
        }
    }
    let mut groups: Vec<_> = groups.into_values().collect();
    groups.sort_by(|a, b| (&a.name, &a.id).cmp(&(&b.name, &b.id)));
    for group in &mut groups {
        group.mesh_ids.sort();
    }
    if groups.len() > MAX_MODEL_GROUPS {
        return Err(ImportError::TooManyGroups(groups.len()));
    }
    Ok(groups)
}

fn range_ticks(range: FrameRange, fps: f64) -> Result<u64, ImportError> {
    if !range.start.is_finite()
        || !range.end.is_finite()
        || range.end < range.start
        || !fps.is_finite()
        || fps <= 0.0
    {
        return Err(ImportError::InvalidRange);
    }
    // Include both endpoints, placing a final key at the requested end when off-grid.
    let steps = ((range.end - range.start) / fps * f64::from(OUTPUT_FPS)).ceil();
    if !steps.is_finite() || steps >= u64::MAX as f64 {
        return Err(ImportError::TooManyTicks(u64::MAX));
    }
    let ticks = (steps as u64)
        .checked_add(1)
        .ok_or(ImportError::TooManyTicks(u64::MAX))?;
    if ticks > MAX_OUTPUT_TICKS {
        return Err(ImportError::TooManyTicks(ticks));
    }
    Ok(ticks)
}

fn validate_pmx_matches(
    groups: &[ModelGroup],
    matches: &[ExistingPmxMatch],
) -> Result<(), ImportError> {
    let mut bound_groups = BTreeSet::new();
    let mut identities = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for entry in matches {
        let group = groups
            .iter()
            .find(|group| group.id == entry.group_id)
            .ok_or_else(|| ImportError::UnknownPmxGroup(entry.group_id.clone()))?;
        if !bound_groups.insert(entry.group_id.as_str()) {
            return Err(ImportError::DuplicatePmxGroup(group.name.clone()));
        }
        if !entry.target.is_absolute()
            || entry.file_identity.is_empty()
            || !entry
                .target
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("pmx"))
            || entry
                .target
                .components()
                .any(|part| matches!(part, PathComponent::ParentDir))
        {
            return Err(ImportError::InvalidPmxTarget(group.name.clone()));
        }
        // The future PMX inspector supplies canonical file identity; lexical checks catch common aliases too.
        let mut path_key = entry
            .target
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        if cfg!(windows) {
            path_key = path_key.to_lowercase();
        }
        if !identities.insert(entry.file_identity.as_str()) || !paths.insert(path_key) {
            return Err(ImportError::DuplicatePmxTarget(entry.target.clone()));
        }
        if entry.names == MatchStatus::Mismatch {
            return Err(ImportError::PmxNamesMismatch {
                group: group.name.clone(),
                target: entry.target.clone(),
            });
        }
        if entry.rest_basis == MatchStatus::Mismatch {
            return Err(ImportError::PmxRestMismatch {
                group: group.name.clone(),
                target: entry.target.clone(),
            });
        }
        if entry.names != MatchStatus::Verified || entry.rest_basis != MatchStatus::Verified {
            return Err(ImportError::PmxMatchUnverified {
                group: group.name.clone(),
                target: entry.target.clone(),
            });
        }
    }
    for group in groups {
        if !bound_groups.contains(group.id.as_str()) {
            return Err(ImportError::PmxTargetRequired(group.name.clone()));
        }
    }
    Ok(())
}

fn feature_selected(component: Component, selection: Selection) -> bool {
    match component {
        Component::Model => selection.model,
        Component::ModelAnimation => {
            selection.animation && (selection.model || selection.animation_only())
        }
        Component::Camera => selection.camera,
        Component::CameraAnimation => selection.camera && selection.animation,
    }
}

/// Pure planning only. Does not inspect input files, create staging, or publish output.
pub fn build_output_plan(
    request: &ImportRequest,
    scene: &SceneInventory,
    matches: &[ExistingPmxMatch],
) -> Result<OutputPlan, ImportError> {
    request.selection.validate()?;
    let groups = discover_model_groups(scene, request.selection)?;
    let selection = request.selection;
    if selection.model && !scene.meshes.iter().any(|mesh| mesh.render_enabled) {
        return Err(ImportError::MissingModels);
    }
    if selection.camera && scene.active_camera.is_none() {
        return Err(ImportError::MissingCamera);
    }
    if selection.animation_only() && groups.is_empty() {
        return Err(ImportError::MissingAnimationGroups);
    }
    let reference_frame = request.reference_frame.unwrap_or(scene.saved_frame);
    let frame_range = request.frame_range.unwrap_or(scene.frame_range);
    if !reference_frame.is_finite() {
        return Err(ImportError::InvalidRange);
    }
    let ticks = if selection.animation {
        Some(range_ticks(frame_range, scene.frames_per_second)?)
    } else {
        None
    };
    let scale_to_mmd = request.units_per_meter * scene.unit_scale;
    if !request.units_per_meter.is_finite()
        || request.units_per_meter <= 0.0
        || !scene.unit_scale.is_finite()
        || scene.unit_scale <= 0.0
        || !scale_to_mmd.is_finite()
        || scale_to_mmd <= 0.0
    {
        return Err(ImportError::InvalidUnits);
    }
    if selection.camera
        && scene
            .render_size
            .iter()
            .any(|size| !(1..=16384).contains(size))
    {
        return Err(ImportError::InvalidRenderSize);
    }
    if let Some(feature) = scene
        .unsupported
        .iter()
        .find(|feature| feature_selected(feature.component, selection))
    {
        return Err(ImportError::UnsupportedFeature {
            target: feature.target.clone(),
            reason: feature.reason.clone(),
        });
    }
    if selection.animation_only() {
        validate_pmx_matches(&groups, matches)?;
    } else if !matches.is_empty() {
        return Err(ImportError::UnexpectedPmxTargets);
    }
    let mut plan = OutputPlan {
        scene_name: scene.scene_name.clone(),
        selection,
        reference_frame,
        frame_range,
        scale_to_mmd,
        render_size: scene.render_size,
        groups,
        existing_pmx: matches.to_vec(),
        artifacts: Vec::new(),
        notices: Vec::new(),
        load_steps: Vec::new(),
        required_backends: Vec::new(),
    };
    let mut motion_steps = Vec::new();
    for (index, group) in plan.groups.iter().enumerate() {
        let pmx = PathBuf::from(format!("model_{:04}.pmx", index + 1));
        if selection.model {
            plan.artifacts.push(PlannedArtifact {
                relative_path: pmx.clone(),
                kind: ArtifactKind::ModelPmx {
                    group_id: group.id.clone(),
                },
            });
            plan.load_steps.push(LoadStep::Model {
                group_name: group.name.clone(),
                pmx: pmx.clone(),
            });
        }
        if selection.animation {
            match group.motion {
                MotionState::Unknown => return Err(ImportError::UnknownMotion(group.name.clone())),
                MotionState::Static => plan.notices.push(if selection.model {
                    PlanNotice::StaticModel {
                        group_name: group.name.clone(),
                    }
                } else {
                    PlanNotice::StaticAnimationGroup {
                        group_name: group.name.clone(),
                    }
                }),
                MotionState::Animated => {
                    let vmd = PathBuf::from(format!("model_{:04}.motion.vmd", index + 1));
                    plan.artifacts.push(PlannedArtifact {
                        relative_path: vmd.clone(),
                        kind: ArtifactKind::ModelMotion {
                            group_id: group.id.clone(),
                        },
                    });
                    let target_pmx = if selection.animation_only() {
                        matches
                            .iter()
                            .find(|entry| entry.group_id == group.id)
                            .expect("validated group binding")
                            .target
                            .clone()
                    } else {
                        pmx
                    };
                    motion_steps.push(LoadStep::ModelMotion {
                        group_name: group.name.clone(),
                        target_pmx,
                        vmd,
                    });
                }
            }
        }
    }
    plan.load_steps.extend(motion_steps);
    if selection.model {
        let used_meshes: BTreeSet<_> = plan
            .groups
            .iter()
            .flat_map(|group| group.mesh_ids.iter().map(String::as_str))
            .collect();
        let mut image_ids = BTreeSet::new();
        for mesh in scene
            .meshes
            .iter()
            .filter(|mesh| used_meshes.contains(mesh.object.id.as_str()))
        {
            for image_id in &mesh.image_ids {
                if !scene.images.iter().any(|image| image.id == *image_id) {
                    return Err(ImportError::MissingImage {
                        mesh: mesh.object.name.clone(),
                        image_id: image_id.clone(),
                    });
                }
                image_ids.insert(image_id.as_str());
            }
        }
        let mut images: Vec<_> = scene
            .images
            .iter()
            .filter(|image| image_ids.contains(image.id.as_str()))
            .collect();
        images.sort_by(|a, b| (&a.name, &a.id).cmp(&(&b.name, &b.id)));
        for (index, image) in images.into_iter().enumerate() {
            plan.artifacts.push(PlannedArtifact {
                relative_path: PathBuf::from(format!("textures/texture_{:04}.png", index + 1)),
                kind: ArtifactKind::TexturePng {
                    image_id: image.id.clone(),
                },
            });
        }
    }
    if selection.camera {
        let camera = scene
            .active_camera
            .as_ref()
            .expect("validated active camera");
        let sampling = match ticks {
            Some(ticks) => CameraSampling::Range {
                frames: frame_range,
                ticks,
                frames_per_second: OUTPUT_FPS,
            },
            None => CameraSampling::ReferenceFrame(reference_frame),
        };
        plan.artifacts.push(PlannedArtifact {
            relative_path: PathBuf::from("camera.vmd"),
            kind: ArtifactKind::CameraVmd {
                camera_id: camera.id.clone(),
                sampling,
            },
        });
        plan.load_steps.push(LoadStep::Camera {
            vmd: PathBuf::from("camera.vmd"),
        });
    }
    if plan.artifacts.len() > MAX_OUTPUT_FILES {
        return Err(ImportError::TooManyFiles(plan.artifacts.len()));
    }
    if selection.animation && !plan.groups.is_empty() {
        plan.notices.push(PlanNotice::NameMappingPending);
    }
    if selection.model {
        plan.required_backends.push(Backend::ModelPmx);
    }
    if plan
        .artifacts
        .iter()
        .any(|artifact| matches!(artifact.kind, ArtifactKind::ModelMotion { .. }))
    {
        plan.required_backends.push(Backend::ModelMotion);
    }
    if selection.camera {
        plan.required_backends.push(Backend::CameraPipeline);
    }
    plan.required_backends.push(Backend::AtomicPublication);
    Ok(plan)
}

/// M0 has no conversion/publication path. Every plan stops here without I/O.
pub fn execute(plan: &OutputPlan) -> Result<(), ImportError> {
    Err(ImportError::UnsupportedBackend(
        plan.required_backends.clone(),
    ))
}
