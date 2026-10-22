//! Blender scene extraction, selected PMX/VMD generation, and transactional publication.

pub mod coordinates;
pub mod packet;
pub mod transaction;
pub mod names;
pub mod pmx;
pub mod motion;
pub mod existing_pmx;
pub mod source;
pub mod pipeline;
#[cfg(windows)]
pub mod gui;
mod planner;
mod text;

pub use nexa_blend_camera as camera;
pub use planner::{build_output_plan, discover_model_groups, execute};
pub use text::Language;

use std::{fmt, path::PathBuf};

pub const PRODUCT_NAME: &str = "Nexa Blend Importer";
pub const DEFAULT_UNITS_PER_METER: f64 = 12.5;
pub const MAX_MODEL_GROUPS: usize = 256;
pub const MAX_OUTPUT_FILES: usize = 4096;
pub const MAX_OUTPUT_TICKS: u64 = 108_001;
pub const OUTPUT_FPS: u32 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub model: bool,
    pub camera: bool,
    pub animation: bool,
}

impl Default for Selection {
    fn default() -> Self {
        Self {
            model: true,
            camera: true,
            animation: true,
        }
    }
}

impl Selection {
    pub const fn animation_only(self) -> bool {
        self.animation && !self.model && !self.camera
    }

    pub fn validate(self) -> Result<(), ImportError> {
        if self.model || self.camera || self.animation {
            Ok(())
        } else {
            Err(ImportError::AllOff)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameRange {
    pub start: f64,
    pub end: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImportRequest {
    pub selection: Selection,
    pub reference_frame: Option<f64>,
    pub frame_range: Option<FrameRange>,
    pub units_per_meter: f64,
}

impl Default for ImportRequest {
    fn default() -> Self {
        Self {
            selection: Selection::default(),
            reference_frame: None,
            frame_range: None,
            units_per_meter: DEFAULT_UNITS_PER_METER,
        }
    }
}

/// Inventory IDs identify source objects; names are display data and never paths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceObject {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotionState {
    Static,
    Animated,
    /// M1 must evaluate the selected range before a motion can be planned.
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeshObject {
    pub object: SourceObject,
    pub render_enabled: bool,
    pub armature_id: Option<String>,
    /// Includes evaluated object movement and shape-key changes.
    pub motion: MotionState,
    pub image_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArmatureObject {
    pub object: SourceObject,
    /// Includes evaluated bone and armature-object movement.
    pub motion: MotionState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Component {
    Model,
    ModelAnimation,
    Camera,
    CameraAnimation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsupportedFeature {
    pub component: Component,
    pub target: String,
    /// Source/backend diagnostic, preserved verbatim in both languages.
    pub reason: String,
}

/// Synthetic at M0. No API in this crate reads a .blend or verifies a PMX file.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneInventory {
    pub scene_name: String,
    pub saved_frame: f64,
    pub frame_range: FrameRange,
    pub frames_per_second: f64,
    pub unit_scale: f64,
    pub render_size: [u32; 2],
    pub meshes: Vec<MeshObject>,
    pub armatures: Vec<ArmatureObject>,
    pub images: Vec<SourceObject>,
    /// The scene's active camera, not an arbitrary camera from the scene.
    pub active_camera: Option<SourceObject>,
    pub unsupported: Vec<UnsupportedFeature>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupSource {
    Armature,
    IndependentMesh,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelGroup {
    pub id: String,
    pub name: String,
    pub source: GroupSource,
    pub mesh_ids: Vec<String>,
    pub motion: MotionState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchStatus {
    Verified,
    Mismatch,
    Unchecked,
}

/// M1/M3 must populate this after reading the chosen existing PMX.
/// M0 only checks this policy evidence; it does not prove names or rest bases.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExistingPmxMatch {
    pub group_id: String,
    pub target: PathBuf,
    /// Stable file identity from the inspector; aliases/hardlinks must share it.
    pub file_identity: String,
    pub names: MatchStatus,
    pub rest_basis: MatchStatus,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CameraSampling {
    ReferenceFrame(f64),
    Range {
        frames: FrameRange,
        ticks: u64,
        frames_per_second: u32,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum ArtifactKind {
    ModelPmx {
        group_id: String,
    },
    ModelMotion {
        group_id: String,
    },
    CameraVmd {
        camera_id: String,
        sampling: CameraSampling,
    },
    TexturePng {
        image_id: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlannedArtifact {
    /// Generated relative name only; contains no source names.
    pub relative_path: PathBuf,
    pub kind: ArtifactKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanNotice {
    StaticModel {
        group_name: String,
    },
    /// Existing-PMX matching is still required for a static animation-only group.
    StaticAnimationGroup {
        group_name: String,
    },
    /// M3 adds a name map when shortening is necessary; no name mapping occurs at M0.
    NameMappingPending,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadStep {
    Model {
        group_name: String,
        pmx: PathBuf,
    },
    ModelMotion {
        group_name: String,
        target_pmx: PathBuf,
        vmd: PathBuf,
    },
    Camera {
        vmd: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    ModelPmx,
    ModelMotion,
    CameraPipeline,
    AtomicPublication,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OutputPlan {
    pub scene_name: String,
    pub selection: Selection,
    pub reference_frame: f64,
    pub frame_range: FrameRange,
    pub scale_to_mmd: f64,
    pub render_size: [u32; 2],
    pub groups: Vec<ModelGroup>,
    pub existing_pmx: Vec<ExistingPmxMatch>,
    pub artifacts: Vec<PlannedArtifact>,
    pub notices: Vec<PlanNotice>,
    /// Manual standard MMD loading order; paths are relative for generated PMX.
    pub load_steps: Vec<LoadStep>,
    pub required_backends: Vec<Backend>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportError {
    AllOff,
    InvalidInventory { field: &'static str, target: String },
    DuplicateObjectId(String),
    MissingArmature { mesh: String, armature_id: String },
    MissingImage { mesh: String, image_id: String },
    MissingModels,
    MissingCamera,
    MissingAnimationGroups,
    TooManyGroups(usize),
    UnknownMotion(String),
    InvalidRange,
    TooManyTicks(u64),
    InvalidUnits,
    InvalidRenderSize,
    UnsupportedFeature { target: String, reason: String },
    PmxTargetRequired(String),
    UnknownPmxGroup(String),
    DuplicatePmxGroup(String),
    UnexpectedPmxTargets,
    InvalidPmxTarget(String),
    PmxNamesMismatch { group: String, target: PathBuf },
    PmxRestMismatch { group: String, target: PathBuf },
    PmxMatchUnverified { group: String, target: PathBuf },
    DuplicatePmxTarget(PathBuf),
    TooManyFiles(usize),
    UnsupportedBackend(Vec<Backend>),
}

impl ImportError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::AllOff => "all_off",
            Self::InvalidInventory { .. } => "invalid_inventory",
            Self::DuplicateObjectId(_) => "duplicate_object_id",
            Self::MissingArmature { .. } => "missing_armature",
            Self::MissingImage { .. } => "missing_image",
            Self::MissingModels => "missing_models",
            Self::MissingCamera => "missing_camera",
            Self::MissingAnimationGroups => "missing_animation_groups",
            Self::TooManyGroups(_) => "too_many_groups",
            Self::UnknownMotion(_) => "unknown_motion",
            Self::InvalidRange => "invalid_range",
            Self::TooManyTicks(_) => "too_many_ticks",
            Self::InvalidUnits => "invalid_units",
            Self::InvalidRenderSize => "invalid_render_size",
            Self::UnsupportedFeature { .. } => "unsupported_feature",
            Self::PmxTargetRequired(_) => "pmx_target_required",
            Self::UnknownPmxGroup(_) => "unknown_pmx_group",
            Self::DuplicatePmxGroup(_) => "duplicate_pmx_group",
            Self::UnexpectedPmxTargets => "unexpected_pmx_targets",
            Self::InvalidPmxTarget(_) => "invalid_pmx_target",
            Self::PmxNamesMismatch { .. } => "pmx_names_mismatch",
            Self::PmxRestMismatch { .. } => "pmx_rest_mismatch",
            Self::PmxMatchUnverified { .. } => "pmx_match_unverified",
            Self::DuplicatePmxTarget(_) => "duplicate_pmx_target",
            Self::TooManyFiles(_) => "too_many_files",
            Self::UnsupportedBackend(_) => "unsupported_backend",
        }
    }
}

impl fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message(Language::English))
    }
}

impl std::error::Error for ImportError {}

#[cfg(test)]
mod contract_tests;
