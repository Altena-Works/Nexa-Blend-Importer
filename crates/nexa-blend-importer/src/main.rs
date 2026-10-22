use nexa_blend_importer::*;
use std::{env, process::ExitCode};
mod cli;

fn help(language: Language) -> &'static str {
    language.choose(
        "Nexa Blend Importer 0.3.0\n  --gui [--language ja|en]  blend選択と3要素の取り込み画面\n  --import FILE.blend [--blender blender.exe] [--output-parent DIR]\n    [--selection model,camera,animation] [--scene NAME]\n    [--reference-frame N] [--start N --end N] [--units 12.5]\n    [--allow-fov-approximation] [--target-pmx FILE.pmx ...]\n  --plan [--selection ...]  開発用の合成入力計画（変換しません）\n\nBlenderを自動検出します。既存PMXはanimation-only時にgroup順で指定します。\n生成PMX→モデルmotion VMD→camera VMDはMMDの標準操作で読み込みます。",
        "Nexa Blend Importer 0.3.0\n  --gui [--language ja|en]  blend picker and component-selection window\n  --import FILE.blend [--blender blender.exe] [--output-parent DIR]\n    [--selection model,camera,animation] [--scene NAME]\n    [--reference-frame N] [--start N --end N] [--units 12.5]\n    [--allow-fov-approximation] [--target-pmx FILE.pmx ...]\n  --plan [--selection ...]  Developer synthetic plan only; no conversion\n\nInstalled Blender is detected automatically. Animation-only PMX targets follow group order.\nLoad generated PMX, model-motion VMD, then camera VMD through MMD's standard operations.",
    )
}

fn synthetic_scene() -> SceneInventory {
    SceneInventory {
        scene_name: "合成scene / synthetic scene".into(),
        saved_frame: 12.0,
        frame_range: FrameRange {
            start: 1.0,
            end: 31.0,
        },
        frames_per_second: 30.0,
        unit_scale: 1.0,
        render_size: [1920, 1080],
        meshes: vec![
            MeshObject {
                object: SourceObject {
                    id: "mesh.rigged".into(),
                    name: "モデル mesh".into(),
                },
                render_enabled: true,
                armature_id: Some("armature.main".into()),
                motion: MotionState::Static,
                image_ids: vec!["image.diffuse".into()],
            },
            MeshObject {
                object: SourceObject {
                    id: "mesh.static".into(),
                    name: "静的モデル / static model".into(),
                },
                render_enabled: true,
                armature_id: None,
                motion: MotionState::Static,
                image_ids: Vec::new(),
            },
        ],
        armatures: vec![ArmatureObject {
            object: SourceObject {
                id: "armature.main".into(),
                name: "可動モデル / animated model".into(),
            },
            motion: MotionState::Animated,
        }],
        images: vec![SourceObject {
            id: "image.diffuse".into(),
            name: "元画像 / source image".into(),
        }],
        active_camera: Some(SourceObject {
            id: "camera.active".into(),
            name: "カメラ / camera".into(),
        }),
        unsupported: Vec::new(),
    }
}

fn synthetic_matches(
    scene: &SceneInventory,
    selection: Selection,
) -> Result<Vec<ExistingPmxMatch>, ImportError> {
    if !selection.animation_only() {
        return Ok(Vec::new());
    }
    let base = env::temp_dir().join("nexa-importer-synthetic-not-read");
    Ok(discover_model_groups(scene, selection)?
        .into_iter()
        .enumerate()
        .map(|(index, group)| ExistingPmxMatch {
            group_id: group.id,
            target: base.join(format!("target_{:04}.pmx", index + 1)),
            file_identity: format!("synthetic-identity-{index}"),
            names: MatchStatus::Verified,
            rest_basis: MatchStatus::Verified,
        })
        .collect())
}

fn selection(value: &str) -> Option<Selection> {
    let mut result = Selection {
        model: false,
        camera: false,
        animation: false,
    };
    if value == "none" {
        return Some(result);
    }
    for item in value.split(',') {
        match item {
            "model" if !result.model => result.model = true,
            "camera" if !result.camera => result.camera = true,
            "animation" if !result.animation => result.animation = true,
            _ => return None,
        }
    }
    Some(result)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Help,
    Plan,
    Gui,
}

struct Arguments {
    language: Language,
    mode: Mode,
    selection: Selection,
}

fn arguments(args: &[String], language: Language) -> Result<Arguments, String> {
    let mut parsed = Arguments {
        language,
        mode: Mode::Help,
        selection: Selection::default(),
    };
    let mut explicit_mode = false;
    let mut explicit_selection = false;
    let mut index = 0;
    while index < args.len() {
        let value = &args[index];
        match value.as_str() {
            "--language" => {
                index += 1;
                parsed.language = args
                    .get(index)
                    .and_then(|value| Language::parse(value))
                    .ok_or_else(|| {
                        language
                            .choose(
                                "--languageにはjaまたはenを指定してください。",
                                "--language requires ja or en.",
                            )
                            .to_string()
                    })?;
            }
            "--selection" => {
                index += 1;
                parsed.selection = args
                    .get(index)
                    .and_then(|value| selection(value))
                    .ok_or_else(|| {
                        language
                            .choose("--selectionの値が不正です。", "Invalid --selection value.")
                            .to_string()
                    })?;
                if explicit_selection {
                    return Err(language
                        .choose(
                            "--selectionの重複指定はできません。",
                            "--selection may only be supplied once.",
                        )
                        .into());
                }
                explicit_selection = true;
            }
            "--help" | "-h" | "--plan" | "--gui" => {
                if explicit_mode {
                    return Err(language
                        .choose(
                            "--help、--plan、--guiのいずれか1つを指定してください。",
                            "Choose one of --help, --plan, or --gui.",
                        )
                        .into());
                }
                parsed.mode = match value.as_str() {
                    "--plan" => Mode::Plan,
                    "--gui" => Mode::Gui,
                    _ => Mode::Help,
                };
                explicit_mode = true;
            }
            _ => {
                return Err(format!(
                    "{} {value}",
                    language.choose("不明な引数：", "Unknown argument:")
                ));
            }
        }
        index += 1;
    }
    if explicit_selection && parsed.mode != Mode::Plan {
        return Err(language
            .choose(
                "--selectionは--planと一緒に指定してください。",
                "--selection requires --plan.",
            )
            .into());
    }
    Ok(parsed)
}

#[cfg(not(windows))]
fn gui_pending(language: Language) {
    eprintln!("{}", language.gui_pending());
}

fn run(args: &[String]) -> ExitCode {
    // Resolve a valid language before reporting an earlier malformed argument.
    let language = args
        .windows(2)
        .filter(|pair| pair[0] == "--language")
        .filter_map(|pair| Language::parse(&pair[1]))
        .next_back()
        .unwrap_or_default();
    if args.iter().any(|arg| arg == "--import") {
        return cli::run(args, language);
    }
    let parsed = match arguments(args, language) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("{PRODUCT_NAME}: {error}");
            return ExitCode::from(2);
        }
    };
    match parsed.mode {
        Mode::Help => {
            println!("{}", help(parsed.language));
            ExitCode::SUCCESS
        }
        Mode::Gui => {
            #[cfg(windows)]
            { match nexa_blend_importer::gui::run(parsed.language) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => { nexa_blend_importer::gui::error_message(parsed.language, &error); ExitCode::FAILURE }
            } }
            #[cfg(not(windows))]
            { gui_pending(parsed.language); ExitCode::FAILURE }
        }
        Mode::Plan => {
            println!("{}", parsed.language.choose(
                "合成入力の計画です。blend・PMXの読込み、変換、ファイル出力は行いません。",
                "Synthetic planning only. No blend/PMX reading, conversion, or file output is performed.",
            ));
            if parsed.selection.animation_only() {
                println!("{}", parsed.language.choose(
                "PMXの名前・rest basis・file identityは合成証跡です。実PMXの照合は未実装です。",
                "PMX names, rest basis, and file identity use synthetic evidence. Real PMX matching is not implemented.",
            ));
            }
            let scene = synthetic_scene();
            let result = synthetic_matches(&scene, parsed.selection).and_then(|matches| {
                build_output_plan(
                    &ImportRequest {
                        selection: parsed.selection,
                        ..ImportRequest::default()
                    },
                    &scene,
                    &matches,
                )
            });
            match result {
                Ok(plan) => {
                    println!("{}", plan.describe(parsed.language));
                    if let Err(error) = execute(&plan) {
                        println!("[{}] {}", error.code(), error.message(parsed.language));
                    }
                    // Success means only that the explicitly requested synthetic plan was built.
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("[{}] {}", error.code(), error.message(parsed.language));
                    ExitCode::FAILURE
                }
            }
        }
    }
}

fn main() -> ExitCode {
    run(&env::args().skip(1).collect::<Vec<_>>())
}
