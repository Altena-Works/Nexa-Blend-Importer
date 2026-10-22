use crate::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Language {
    #[default]
    Japanese,
    English,
}

impl Language {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "ja" => Some(Self::Japanese),
            "en" => Some(Self::English),
            _ => None,
        }
    }

    pub const fn code(self) -> &'static str {
        match self {
            Self::Japanese => "ja",
            Self::English => "en",
        }
    }

    pub const fn choose<'a>(self, japanese: &'a str, english: &'a str) -> &'a str {
        match self {
            Self::Japanese => japanese,
            Self::English => english,
        }
    }

    pub const fn gui_pending(self) -> &'static str {
        self.choose(
            "M0：要求と出力計画のみ実装済みです。\nblendの選択・対象検出・変換・出力はまだ実装されていません。\n\n完成後は、生成PMX → 対応モデルmotion VMD → camera VMDの順にMMD標準操作で手動読込みします。\n\n合成入力による出力計画：nexa-blend-importer.exe --plan --language ja",
            "M0 implements requests and output planning only.\nBlend selection, scene discovery, conversion, and output are not implemented yet.\n\nWhen implemented, use standard MMD operations to load generated PMX, matching model motion VMD, and camera VMD in that order.\n\nSynthetic output plan: nexa-blend-importer.exe --plan --language en",
        )
    }
}

impl Backend {
    pub fn label(self, language: Language) -> &'static str {
        match self {
            Self::ModelPmx => language.choose("モデルPMX変換", "model PMX conversion"),
            Self::ModelMotion => language.choose("モデルmotion変換", "model motion conversion"),
            Self::CameraPipeline => {
                language.choose("Importerのcamera統合", "Importer camera pipeline")
            }
            Self::AtomicPublication => {
                language.choose("出力の一括公開", "atomic output publication")
            }
        }
    }
}

impl ImportError {
    pub fn message(&self, language: Language) -> String {
        let en = language == Language::English;
        match self {
            Self::AllOff => language.choose("モデル・カメラ・アニメーションのいずれかを選択してください。", "Select at least one of model, camera, or animation.").into(),
            Self::InvalidInventory { field, target } => if en { format!("Invalid scene inventory field {field}: {target}") } else { format!("シーン一覧の項目が不正です（{field}）：{target}") },
            Self::DuplicateObjectId(id) => if en { format!("Duplicate source ID: {id}") } else { format!("入力IDが重複しています：{id}") },
            Self::MissingArmature { mesh, armature_id } => if en { format!("Mesh {mesh} references missing armature {armature_id}.") } else { format!("mesh「{mesh}」の参照armatureがありません：{armature_id}") },
            Self::MissingImage { mesh, image_id } => if en { format!("Mesh {mesh} references missing image {image_id}.") } else { format!("mesh「{mesh}」の参照画像がありません：{image_id}") },
            Self::MissingModels => language.choose("モデルが選択されていますが、render有効なmeshがありません。", "Model is selected, but no render-enabled mesh exists.").into(),
            Self::MissingCamera => language.choose("カメラが選択されていますが、sceneのactive cameraがありません。", "Camera is selected, but the scene has no active camera.").into(),
            Self::MissingAnimationGroups => language.choose("animation-onlyの対象model groupがありません。", "No model group exists for animation-only import.").into(),
            Self::TooManyGroups(count) => if en { format!("Model groups exceed the limit of {MAX_MODEL_GROUPS}: {count}") } else { format!("model group数が上限{MAX_MODEL_GROUPS}を超えています：{count}") },
            Self::UnknownMotion(target) => if en { format!("Motion has not been evaluated for {target}; no output can be planned.") } else { format!("「{target}」の動きが未評価のため、出力を計画できません。") },
            Self::InvalidRange => language.choose("有限の基準フレームと、有効な開始・終了フレーム／fpsが必要です。", "A finite reference frame and a valid start/end frame range and fps are required.").into(),
            Self::TooManyTicks(count) => if en { format!("The 30 fps range exceeds {MAX_OUTPUT_TICKS} ticks: {count}") } else { format!("30fpsの範囲が上限{MAX_OUTPUT_TICKS}ticksを超えています：{count}") },
            Self::InvalidUnits => language.choose("単位倍率とscene.unit_scaleは有限の正数でなければなりません。", "Unit multiplier and scene.unit_scale must be finite positive numbers.").into(),
            Self::InvalidRenderSize => language.choose("カメラのrender画像寸法は各辺1〜16384pxでなければなりません。", "Camera render dimensions must each be in 1..16384 pixels.").into(),
            Self::UnsupportedFeature { target, reason } => if en { format!("Unsupported selected information in {target}: {reason}. The entire import fails.") } else { format!("「{target}」の選択情報が未対応です：{reason}。取込み全体を失敗とします。") },
            Self::PmxTargetRequired(group) => if en { format!("Choose and verify an existing PMX for source model group {group}.") } else { format!("source model group「{group}」の既存PMXを選択し、照合してください。") },
            Self::UnknownPmxGroup(group) => if en { format!("PMX assignment refers to unknown group {group}.") } else { format!("PMX割当先のgroupがありません：{group}") },
            Self::DuplicatePmxGroup(group) => if en { format!("Multiple PMX targets are assigned to group {group}.") } else { format!("group「{group}」に複数のPMXが割り当てられています。") },
            Self::UnexpectedPmxTargets => language.choose("既存PMXの追加選択はanimation-onlyでのみ使用します。", "Existing PMX selection is only used for animation-only import.").into(),
            Self::InvalidPmxTarget(group) => if en { format!("Group {group} requires an absolute .pmx path and inspected file identity.") } else { format!("group「{group}」には絶対パスの.pmxと検査済みfile identityが必要です。") },
            Self::PmxNamesMismatch { group, target } => if en { format!("PMX name mismatch for {group}: {}", target.display()) } else { format!("「{group}」とPMXの名前が一致しません：{}", target.display()) },
            Self::PmxRestMismatch { group, target } => if en { format!("PMX rest-basis mismatch for {group}: {}", target.display()) } else { format!("「{group}」とPMXのrest basisが一致しません：{}", target.display()) },
            Self::PmxMatchUnverified { group, target } => if en { format!("PMX names and rest basis are not both verified for {group}: {}", target.display()) } else { format!("「{group}」とPMXの名前・rest basisの照合が完了していません：{}", target.display()) },
            Self::DuplicatePmxTarget(target) => if en { format!("The same PMX cannot be assigned to multiple groups: {}", target.display()) } else { format!("同じPMXを複数groupへ割り当てられません：{}", target.display()) },
            Self::TooManyFiles(count) => if en { format!("Planned output exceeds {MAX_OUTPUT_FILES} files: {count}") } else { format!("計画出力数が上限{MAX_OUTPUT_FILES}filesを超えています：{count}") },
            Self::UnsupportedBackend(backends) => {
                let labels = backends.iter().map(|backend| backend.label(language)).collect::<Vec<_>>().join(", ");
                if en { format!("M0 backend not implemented: {labels}. No conversion or publication was performed.") }
                else { format!("M0ではbackend未実装です：{labels}。変換・出力の公開は行っていません。") }
            }
        }
    }
}

impl PlanNotice {
    pub fn message(&self, language: Language) -> String {
        let en = language == Language::English;
        match self {
            Self::StaticModel { group_name } => if en { format!("{group_name}: static model; PMX only, no model motion VMD.") } else { format!("{group_name}：動きがないためPMXのみ。モデルmotion VMDは生成しません。") },
            Self::StaticAnimationGroup { group_name } => if en { format!("{group_name}: no motion; no VMD planned. Its existing PMX still requires matching.") } else { format!("{group_name}：動きがないためVMDの計画はありません。既存PMXの照合は必要です。") },
            Self::NameMappingPending => language.choose("名前対応表はM3の照合で必要性を確定します。M0は名前を変換しません。", "M3 name validation will determine whether a mapping table is needed. M0 performs no name conversion.").into(),
        }
    }
}

impl LoadStep {
    pub fn message(&self, language: Language) -> String {
        let en = language == Language::English;
        match self {
            Self::Model { group_name, pmx } => {
                if en {
                    format!("Load model {group_name}: {}", pmx.display())
                } else {
                    format!("モデル「{group_name}」を読込み：{}", pmx.display())
                }
            }
            Self::ModelMotion {
                group_name,
                target_pmx,
                vmd,
            } => {
                if en {
                    format!(
                        "Load {} onto model {group_name} ({})",
                        vmd.display(),
                        target_pmx.display()
                    )
                } else {
                    format!(
                        "モデル「{group_name}」（{}）へmotionを読込み：{}",
                        target_pmx.display(),
                        vmd.display()
                    )
                }
            }
            Self::Camera { vmd } => {
                if en {
                    format!("Load camera motion: {}", vmd.display())
                } else {
                    format!("カメラmotionを読込み：{}", vmd.display())
                }
            }
        }
    }
}

impl OutputPlan {
    pub fn describe(&self, language: Language) -> String {
        let mut lines = vec![format!(
            "{}: {}",
            language.choose(
                "出力計画（変換未実施）",
                "Output plan (conversion not performed)"
            ),
            self.scene_name
        )];
        for artifact in &self.artifacts {
            lines.push(format!("  {}", artifact.relative_path.display()));
        }
        for notice in &self.notices {
            lines.push(notice.message(language));
        }
        lines.push(
            language
                .choose(
                    "MMD標準操作での手動読込み順：",
                    "Manual loading order using standard MMD operations:",
                )
                .into(),
        );
        for (index, step) in self.load_steps.iter().enumerate() {
            lines.push(format!("  {}. {}", index + 1, step.message(language)));
        }
        lines.join("\n")
    }
}
