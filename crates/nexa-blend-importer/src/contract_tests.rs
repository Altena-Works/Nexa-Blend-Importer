use crate::*;

fn object(id: &str, name: &str) -> SourceObject {
    SourceObject {
        id: id.into(),
        name: name.into(),
    }
}

fn scene() -> SceneInventory {
    SceneInventory {
        scene_name: "テスト scene".into(),
        saved_frame: 17.0,
        frame_range: FrameRange {
            start: -10.0,
            end: 50.0,
        },
        frames_per_second: 30.0,
        unit_scale: 0.1,
        render_size: [1920, 1080],
        meshes: vec![
            MeshObject {
                object: object("mesh.body", "体"),
                render_enabled: true,
                armature_id: Some("rig".into()),
                motion: MotionState::Static,
                image_ids: vec!["image".into()],
            },
            MeshObject {
                object: object("mesh.clothes", "服"),
                render_enabled: true,
                armature_id: Some("rig".into()),
                motion: MotionState::Static,
                image_ids: vec!["image".into()],
            },
            MeshObject {
                object: object("mesh.floor", "B ../床"),
                render_enabled: true,
                armature_id: None,
                motion: MotionState::Static,
                image_ids: Vec::new(),
            },
            MeshObject {
                object: object("mesh.hidden", "hidden"),
                render_enabled: false,
                armature_id: Some("not-selected".into()),
                motion: MotionState::Unknown,
                image_ids: vec!["not-selected".into()],
            },
        ],
        armatures: vec![
            ArmatureObject {
                object: object("empty", "C 空armature"),
                motion: MotionState::Static,
            },
            ArmatureObject {
                object: object("rig", "A 人物"),
                motion: MotionState::Animated,
            },
        ],
        images: vec![object("image", "../../元画像.png")],
        active_camera: Some(object("camera", "Camera 日本語")),
        unsupported: Vec::new(),
    }
}

fn selected(mask: u8) -> Selection {
    Selection {
        model: mask & 1 != 0,
        camera: mask & 2 != 0,
        animation: mask & 4 != 0,
    }
}

fn bindings(scene: &SceneInventory) -> Vec<ExistingPmxMatch> {
    discover_model_groups(scene, selected(4))
        .unwrap()
        .into_iter()
        .enumerate()
        .map(|(index, group)| ExistingPmxMatch {
            group_id: group.id,
            target: std::env::temp_dir()
                .join("nexa-importer-contract-not-created")
                .join(format!("target_{index}.pmx")),
            file_identity: format!("file-{index}"),
            names: MatchStatus::Verified,
            rest_basis: MatchStatus::Verified,
        })
        .collect()
}

fn plan(
    scene: &SceneInventory,
    mask: u8,
    matches: &[ExistingPmxMatch],
) -> Result<OutputPlan, ImportError> {
    build_output_plan(
        &ImportRequest {
            selection: selected(mask),
            ..ImportRequest::default()
        },
        scene,
        matches,
    )
}

#[test]
fn m0_grouped_request_and_output_contract() {
    let scene = scene();
    let matches = bindings(&scene);
    // All seven combinations; render-disabled inputs do not create selected groups.
    for (mask, expected_files, expected_groups) in [
        (1, 3, 2),
        (2, 1, 0),
        (3, 4, 2),
        (4, 1, 3),
        (5, 4, 2),
        (6, 1, 0),
        (7, 5, 2),
    ] {
        let matching = if mask == 4 { matches.as_slice() } else { &[] };
        let result = plan(&scene, mask, matching).unwrap();
        assert_eq!(result.artifacts.len(), expected_files, "mask {mask}");
        assert_eq!(result.groups.len(), expected_groups, "mask {mask}");
        assert_eq!(result.reference_frame, 17.0);
        assert_eq!(result.scale_to_mmd, 1.25);
        if !result.groups.is_empty() {
            assert_eq!(result.groups[0].name, "A 人物");
            assert_eq!(result.groups[0].mesh_ids, ["mesh.body", "mesh.clothes"]);
            assert_eq!(result.groups[1].name, "B ../床");
        }
        let motions: Vec<_> = result
            .artifacts
            .iter()
            .filter_map(|artifact| match &artifact.kind {
                ArtifactKind::ModelMotion { group_id } => Some(group_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            motions,
            if mask == 4 || mask == 5 || mask == 7 {
                vec!["rig"]
            } else {
                Vec::new()
            }
        );
        for artifact in &result.artifacts {
            assert!(!artifact.relative_path.is_absolute());
            assert!(
                artifact
                    .relative_path
                    .components()
                    .all(|part| matches!(part, std::path::Component::Normal(_)))
            );
            assert!(!artifact.relative_path.to_string_lossy().contains('人'));
        }
        if mask & 2 != 0 {
            let sampling = result
                .artifacts
                .iter()
                .find_map(|artifact| match artifact.kind {
                    ArtifactKind::CameraVmd { sampling, .. } => Some(sampling),
                    _ => None,
                })
                .unwrap();
            assert_eq!(
                sampling,
                if mask & 4 == 0 {
                    CameraSampling::ReferenceFrame(17.0)
                } else {
                    CameraSampling::Range {
                        frames: scene.frame_range,
                        ticks: 61,
                        frames_per_second: 30,
                    }
                }
            );
            assert!(matches!(
                result.load_steps.last(),
                Some(LoadStep::Camera { .. })
            ));
        }
        let mut phase = 0;
        for step in &result.load_steps {
            let current = match step {
                LoadStep::Model { .. } => 0,
                LoadStep::ModelMotion { .. } => 1,
                LoadStep::Camera { .. } => 2,
            };
            assert!(current >= phase);
            phase = current;
        }
        assert!(matches!(
            execute(&result),
            Err(ImportError::UnsupportedBackend(_))
        ));
        let japanese = result.describe(Language::Japanese);
        let english = result.describe(Language::English);
        assert!(japanese.contains("テスト scene") && english.contains("テスト scene"));
        if mask == 7 {
            assert!(result.notices.contains(&PlanNotice::StaticModel {
                group_name: "B ../床".into()
            }));
            assert!(japanese.contains("B ../床") && english.contains("B ../床"));
        }
    }
    assert_eq!(plan(&scene, 0, &[]), Err(ImportError::AllOff));
    assert_eq!(
        plan(&scene, 1, &matches),
        Err(ImportError::UnexpectedPmxTargets)
    );
    let mut missing = scene.clone();
    missing.meshes.clear();
    assert_eq!(plan(&missing, 3, &[]), Err(ImportError::MissingModels));
    missing = scene.clone();
    missing.active_camera = None;
    assert_eq!(plan(&missing, 3, &[]), Err(ImportError::MissingCamera));
    missing.meshes.clear();
    missing.armatures.clear();
    assert_eq!(
        plan(&missing, 4, &[]),
        Err(ImportError::MissingAnimationGroups)
    );
    missing = scene.clone();
    missing.meshes[0].armature_id = Some("missing".into());
    assert!(matches!(
        plan(&missing, 1, &[]),
        Err(ImportError::MissingArmature { .. })
    ));
    missing = scene.clone();
    missing.images.clear();
    assert!(matches!(
        plan(&missing, 1, &[]),
        Err(ImportError::MissingImage { .. })
    ));
    missing = scene.clone();
    missing.meshes[0].object.id = "rig".into();
    assert_eq!(
        plan(&missing, 1, &[]),
        Err(ImportError::DuplicateObjectId("rig".into()))
    );
    missing = scene.clone();
    missing.armatures[1].motion = MotionState::Unknown;
    assert_eq!(
        plan(&missing, 5, &[]),
        Err(ImportError::UnknownMotion("A 人物".into()))
    );
    assert!(plan(&missing, 1, &[]).is_ok());
    let mut static_scene = scene.clone();
    static_scene.armatures[1].motion = MotionState::Static;
    let static_plan = plan(&static_scene, 5, &[]).unwrap();
    assert!(
        !static_plan
            .artifacts
            .iter()
            .any(|artifact| matches!(artifact.kind, ArtifactKind::ModelMotion { .. }))
    );
    assert_eq!(static_plan.artifacts.len(), 3);
    // PMX assignment is one-to-one and requires BOTH name and rest-basis evidence.
    assert_eq!(
        plan(&scene, 4, &[]),
        Err(ImportError::PmxTargetRequired("A 人物".into()))
    );
    let mut bad_matches = matches.clone();
    bad_matches[0].names = MatchStatus::Unchecked;
    assert!(matches!(
        plan(&scene, 4, &bad_matches),
        Err(ImportError::PmxMatchUnverified { .. })
    ));
    bad_matches[0].names = MatchStatus::Mismatch;
    assert!(matches!(
        plan(&scene, 4, &bad_matches),
        Err(ImportError::PmxNamesMismatch { .. })
    ));
    bad_matches = matches.clone();
    bad_matches[0].rest_basis = MatchStatus::Mismatch;
    assert!(matches!(
        plan(&scene, 4, &bad_matches),
        Err(ImportError::PmxRestMismatch { .. })
    ));
    bad_matches = matches.clone();
    bad_matches[1].file_identity = bad_matches[0].file_identity.clone();
    assert!(matches!(
        plan(&scene, 4, &bad_matches),
        Err(ImportError::DuplicatePmxTarget(_))
    ));
    bad_matches = matches.clone();
    bad_matches[1].target = bad_matches[0].target.clone();
    assert!(matches!(
        plan(&scene, 4, &bad_matches),
        Err(ImportError::DuplicatePmxTarget(_))
    ));
    bad_matches = matches.clone();
    bad_matches.push(bad_matches[0].clone());
    assert!(matches!(
        plan(&scene, 4, &bad_matches),
        Err(ImportError::DuplicatePmxGroup(_))
    ));
    bad_matches = matches.clone();
    bad_matches[0].group_id = "not-a-group".into();
    assert!(matches!(
        plan(&scene, 4, &bad_matches),
        Err(ImportError::UnknownPmxGroup(_))
    ));
    bad_matches = matches.clone();
    bad_matches[0].target = PathBuf::from("relative.pmx");
    assert!(matches!(
        plan(&scene, 4, &bad_matches),
        Err(ImportError::InvalidPmxTarget(_))
    ));
    let mut unsupported = scene.clone();
    unsupported.unsupported.push(UnsupportedFeature {
        component: Component::ModelAnimation,
        target: "A 人物".into(),
        reason: "scale/shear".into(),
    });
    assert!(matches!(
        plan(&unsupported, 7, &[]),
        Err(ImportError::UnsupportedFeature { .. })
    ));
    assert!(plan(&unsupported, 3, &[]).is_ok());
    assert!(plan(&unsupported, 6, &[]).is_ok());
    let mut invalid = ImportRequest {
        selection: selected(2),
        reference_frame: Some(f64::NAN),
        ..ImportRequest::default()
    };
    assert_eq!(
        build_output_plan(&invalid, &scene, &[]),
        Err(ImportError::InvalidRange)
    );
    invalid = ImportRequest {
        selection: selected(6),
        frame_range: Some(FrameRange {
            start: 0.0,
            end: 108_001.0,
        }),
        ..ImportRequest::default()
    };
    assert_eq!(
        build_output_plan(&invalid, &scene, &[]),
        Err(ImportError::TooManyTicks(108_002))
    );
    invalid.frame_range = Some(FrameRange {
        start: 0.0,
        end: 108_000.0,
    });
    assert!(build_output_plan(&invalid, &scene, &[]).is_ok());
    invalid.units_per_meter = f64::INFINITY;
    assert_eq!(
        build_output_plan(&invalid, &scene, &[]),
        Err(ImportError::InvalidUnits)
    );
    let error = ImportError::PmxTargetRequired("日本語 model".into());
    assert!(error.message(Language::Japanese).contains("日本語 model"));
    assert!(error.message(Language::English).contains("日本語 model"));
    assert_ne!(
        error.message(Language::Japanese),
        error.message(Language::English)
    );
    assert_eq!(Language::parse("ja"), Some(Language::Japanese));
    assert_eq!(Language::parse("en"), Some(Language::English));
    assert_eq!(Language::parse("fr"), None);
}
