use crate::{
    decode, import as import_camera, read as read_camera, CameraKind, CameraSample, DecodeError,
    ImportError, Request, SensorFit,
};
use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        OnceLock,
    },
};

const SCENE: &str = "NexaScene";
const LAYER: &str = "NexaLayer";
const PERSPECTIVE: &str = "Nexaカメラ";
const ORTHOGRAPHIC: &str = "NexaOrtho";

static FAILED: AtomicBool = AtomicBool::new(false);
static FIXTURE: OnceLock<Result<Fixture, String>> = OnceLock::new();

struct CaseGuard;

impl Drop for CaseGuard {
    fn drop(&mut self) {
        if std::thread::panicking() {
            FAILED.store(true, Ordering::SeqCst);
        }
    }
}

fn begin_case() -> CaseGuard {
    assert!(
        !FAILED.load(Ordering::SeqCst),
        "an earlier case failed; no further fixture or Blender execution"
    );
    CaseGuard
}

fn near(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        actual.is_finite() && (actual - expected).abs() <= tolerance,
        "expected {expected} +/- {tolerance}, got {actual}"
    );
}

fn push_string(bytes: &mut Vec<u8>, value: &str) {
    bytes.extend_from_slice(&(value.len() as u32).to_le_bytes());
    bytes.extend_from_slice(value.as_bytes());
}

pub(super) fn known_packet() -> (Vec<u8>, usize, usize) {
    known_packet_with_names(["KnownScene", "KnownLayer", "Knownカメラ"])
}

fn known_packet_with_names(names: [&str; 3]) -> (Vec<u8>, usize, usize) {
    let mut bytes = b"NEXACAM1".to_vec();
    for value in [4_u32, 5, 4] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in names {
        push_string(&mut bytes, value);
    }
    bytes.extend_from_slice(&1_i32.to_le_bytes());
    bytes.extend_from_slice(&0.5_f64.to_le_bytes());
    bytes.extend_from_slice(&1.5_f64.to_le_bytes());
    bytes.extend_from_slice(&1_i32.to_le_bytes());
    bytes.extend_from_slice(&3_i32.to_le_bytes());
    bytes.extend_from_slice(&24_u32.to_le_bytes());
    bytes.extend_from_slice(&1.001_f64.to_le_bytes());
    bytes.extend_from_slice(&100_u32.to_le_bytes());
    bytes.extend_from_slice(&100_u32.to_le_bytes());
    let kind_offset = bytes.len();
    bytes.extend_from_slice(&[0, 1]);
    let lens_offset = bytes.len();
    for value in [55.0_f64, 36.0, 24.0, 0.015, 0.0, 6.0, 0.1, 1000.0, 1.0] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in [800_u32, 600, 100] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in [1.0_f64, 1.0] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let world: [[f64; 4]; 4] = [
        [1.0, 0.0, 0.0, 3.5],
        [0.0, 1.0, 0.0, 2.0],
        [0.0, 0.0, 1.0, 3.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    let projection: [[f64; 4]; 4] = [
        [110.0 / 36.0, 0.0, 0.03, 0.0],
        [0.0, (110.0 / 36.0) * (4.0 / 3.0), 0.0, 0.0],
        [0.0, 0.0, -1000.1 / 999.9, -200.0 / 999.9],
        [0.0, 0.0, -1.0, 0.0],
    ];
    for matrix in [world, projection] {
        for row in matrix {
            for value in row {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
    }
    (bytes, kind_offset, lens_offset)
}

#[test]
fn a_codec_checks_known_packet_and_rejects_malformed_or_unbounded_data() {
    let _guard = begin_case();
    let (packet, kind_offset, lens_offset) = known_packet();
    let sample = decode(&packet).expect("manually specified packet must decode");
    assert_eq!(sample.blender_version, [4, 5, 4]);
    assert_eq!(sample.scene_name, "KnownScene");
    assert_eq!(sample.view_layer_name, "KnownLayer");
    assert_eq!(sample.camera_name, "Knownカメラ");
    assert_eq!(sample.frame, 1);
    near(sample.subframe, 0.5, 0.0);
    near(sample.evaluated_frame, 1.5, 0.0);
    assert_eq!((sample.frame_start, sample.frame_end), (1, 3));
    assert_eq!(sample.fps, 24);
    near(sample.fps_base, 1.001, 0.0);
    assert_eq!((sample.frame_map_old, sample.frame_map_new), (100, 100));
    assert!(matches!(sample.kind, CameraKind::Perspective));
    assert!(matches!(sample.sensor_fit, SensorFit::Horizontal));
    near(sample.lens_mm, 55.0, 0.0);
    near(sample.sensor_width_mm, 36.0, 0.0);
    near(sample.sensor_height_mm, 24.0, 0.0);
    near(sample.shift_x, 0.015, 0.0);
    near(sample.shift_y, 0.0, 0.0);
    near(sample.ortho_scale, 6.0, 0.0);
    near(sample.clip_start, 0.1, 0.0);
    near(sample.clip_end, 1000.0, 0.0);
    near(sample.unit_scale, 1.0, 0.0);
    assert_eq!(sample.resolution, [800, 600]);
    assert_eq!(sample.resolution_percentage, 100);
    assert_eq!(sample.pixel_aspect, [1.0, 1.0]);
    near(sample.matrix_world[0][3], 3.5, 0.0);
    near(sample.matrix_world[1][3], 2.0, 0.0);
    near(sample.matrix_world[2][3], 3.0, 0.0);
    near(sample.matrix_projection[0][0], 110.0 / 36.0, 0.0);
    near(sample.matrix_projection[3][2], -1.0, 0.0);

    assert!(decode(&[]).is_err());
    let mut invalid = packet.clone();
    invalid[0] ^= 1;
    assert!(matches!(decode(&invalid), Err(DecodeError::InvalidMagic)));
    assert!(matches!(
        decode(&packet[..packet.len() - 1]),
        Err(DecodeError::Truncated { .. })
    ));
    assert!(matches!(
        decode(&vec![0; 65_537]),
        Err(DecodeError::TooLarge { .. })
    ));
    let mut invalid = packet.clone();
    invalid[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(decode(&invalid).is_err());
    let mut invalid = packet.clone();
    invalid[24] = 0xff;
    assert!(matches!(
        decode(&invalid),
        Err(DecodeError::InvalidUtf8 { .. })
    ));
    let mut invalid = packet.clone();
    invalid[kind_offset] = 2;
    assert!(matches!(
        decode(&invalid),
        Err(DecodeError::InvalidEnum { .. })
    ));
    let mut invalid = packet.clone();
    invalid[kind_offset + 1] = 3;
    assert!(matches!(
        decode(&invalid),
        Err(DecodeError::InvalidEnum { .. })
    ));
    let mut invalid = packet.clone();
    invalid[lens_offset..lens_offset + 8].copy_from_slice(&f64::NAN.to_le_bytes());
    assert!(matches!(
        decode(&invalid),
        Err(DecodeError::InvalidValue { .. })
    ));
    let mut invalid = packet;
    invalid.push(0);
    assert!(matches!(
        decode(&invalid),
        Err(DecodeError::TrailingBytes { .. })
    ));
}

struct Fixture {
    blender: PathBuf,
    directory: PathBuf,
    source: PathBuf,
    source_bytes: Vec<u8>,
    sentinel: PathBuf,
}

fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?
        .write_all(bytes)
}

fn make_fixture() -> Result<Fixture, String> {
    let blender = PathBuf::from(
        env::var_os("NEXA_TEST_BLENDER").ok_or("set NEXA_TEST_BLENDER explicitly")?,
    );
    let directory =
        PathBuf::from(env::var_os("NEXA_TEST_DIR").ok_or("set NEXA_TEST_DIR explicitly")?);
    if !blender.is_absolute() || !directory.is_absolute() {
        return Err("test paths must be absolute".into());
    }
    let target = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/blender-tests")
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let parent = directory
        .parent()
        .ok_or("fixture directory needs a parent")?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if parent != target {
        return Err("NEXA_TEST_DIR must be a fresh direct child of target/blender-tests".into());
    }
    fs::create_dir(&directory).map_err(|error| error.to_string())?;
    let script = directory.join("create_camera_fixture.py");
    let source = directory.join("camera_fixture.blend");
    let sentinel = directory.join("autoexec-sentinel.txt");
    write_new(&script, FIXTURE_SCRIPT.as_bytes()).map_err(|error| error.to_string())?;
    let output = Command::new(&blender)
        .args([
            "--background",
            "--factory-startup",
            "--disable-autoexec",
            "--python-exit-code",
            "1",
            "--python",
        ])
        .arg(&script)
        .arg("--")
        .arg(&source)
        .arg(&sentinel)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "fixture generation failed: {:?}\nstdout: {}\nstderr: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    if sentinel.exists() {
        return Err("embedded autoexec sentinel unexpectedly ran during fixture creation".into());
    }
    let source_bytes = fs::read(&source).map_err(|error| error.to_string())?;
    if source_bytes.is_empty() {
        return Err("generated blend fixture is empty".into());
    }
    Ok(Fixture {
        blender,
        directory,
        source,
        source_bytes,
        sentinel,
    })
}

fn fixture() -> &'static Fixture {
    match FIXTURE.get_or_init(make_fixture) {
        Ok(fixture) => fixture,
        Err(message) => panic!("fixture unavailable; no retry: {message}"),
    }
}

fn assert_preserved(fixture: &Fixture) {
    let actual = fs::read(&fixture.source).expect("read source blend after import");
    assert!(
        actual == fixture.source_bytes,
        "source blend bytes changed during import"
    );
    assert!(
        !fixture.sentinel.exists(),
        "embedded blend text must not execute"
    );
}

fn import_fixture(fixture: &Fixture, frame: &str, output: &Path, camera_name: &str) {
    assert_preserved(fixture);
    assert!(!output.exists(), "packet output must initially be absent");
    let sample = import_camera(&fixture.blender, &fixture.source, output, Request {
        frame: frame.parse().expect("fixture frame"), scene: Some(SCENE), camera: None,
    }).expect("import the fixture camera through the standalone public API");
    assert_eq!(sample.camera_name, camera_name);
    assert_preserved(fixture);
}

fn assert_common(sample: &CameraSample) {
    assert_eq!(sample.blender_version[..2], [4, 5]);
    assert_eq!(sample.scene_name, SCENE);
    assert_eq!(sample.view_layer_name, LAYER);
    assert_eq!((sample.frame_start, sample.frame_end), (1, 3));
    assert_eq!(sample.fps, 24);
    near(sample.fps_base, 1.001, 1e-6);
    assert_eq!((sample.frame_map_old, sample.frame_map_new), (100, 100));
    assert!(matches!(sample.sensor_fit, SensorFit::Horizontal));
    near(sample.sensor_width_mm, 36.0, 1e-6);
    near(sample.sensor_height_mm, 24.0, 1e-6);
    near(sample.clip_start, 0.1, 1e-6);
    near(sample.clip_end, 1000.0, 1e-6);
    near(sample.unit_scale, 1.0, 1e-6);
    assert_eq!(sample.resolution, [800, 600]);
    assert_eq!(sample.resolution_percentage, 100);
    assert_eq!(sample.pixel_aspect, [1.0, 1.0]);
}

#[test]
#[ignore = "requires caller-selected Blender"]
fn b_imports_perspective_fractional_frame_with_parent_constraint_and_driver() {
    let _guard = begin_case();
    let fixture = fixture();
    let output = fixture.directory.join("perspective.nexacam");
    import_fixture(fixture, "1.5", &output, PERSPECTIVE);
    let sample = read_camera(&output).expect("Rust reader consumes the actual Blender packet");
    assert_common(&sample);
    assert_eq!(sample.camera_name, PERSPECTIVE);
    assert!(matches!(sample.kind, CameraKind::Perspective));
    assert_eq!(sample.frame, 1);
    near(sample.subframe, 0.5, 1e-9);
    near(sample.evaluated_frame, 1.5, 1e-6);
    near(sample.lens_mm, 55.0, 1e-5);
    near(sample.shift_x, 0.015, 1e-6);
    near(sample.shift_y, 0.0, 1e-6);
    let position: [f64; 3] = [3.5, 2.0, 3.0];
    let length = position.iter().map(|value| value * value).sum::<f64>().sqrt();
    for (axis, expected) in position.iter().enumerate() {
        near(sample.matrix_world[axis][3], *expected, 1e-5);
        near(sample.matrix_world[axis][2], expected / length, 1e-5);
    }
    near(sample.matrix_world[3][3], 1.0, 1e-9);
    near(sample.matrix_projection[0][0], 2.0 * 55.0 / 36.0, 1e-5);
    near(sample.matrix_projection[3][2], -1.0, 1e-9);
    near(sample.matrix_projection[3][3], 0.0, 1e-9);
    assert_preserved(fixture);
}

#[test]
#[ignore = "requires caller-selected Blender"]
fn c_imports_orthographic_active_camera_and_refuses_overwrite_before_launch() {
    let _guard = begin_case();
    let fixture = fixture();
    let output = fixture.directory.join("orthographic.nexacam");
    import_fixture(fixture, "3", &output, ORTHOGRAPHIC);
    let sample = read_camera(&output).expect("Rust reader consumes the actual orthographic packet");
    assert_common(&sample);
    assert_eq!(sample.camera_name, ORTHOGRAPHIC);
    assert!(matches!(sample.kind, CameraKind::Orthographic));
    assert_eq!(sample.frame, 3);
    near(sample.subframe, 0.0, 0.0);
    near(sample.evaluated_frame, 3.0, 1e-6);
    near(sample.ortho_scale, 10.0, 1e-6);
    for (axis, expected) in [-4.0, 5.0, 6.0].iter().enumerate() {
        near(sample.matrix_world[axis][3], *expected, 1e-5);
    }
    near(sample.matrix_projection[0][0], 0.2, 1e-6);
    near(sample.matrix_projection[3][2], 0.0, 1e-9);
    near(sample.matrix_projection[3][3], 1.0, 1e-9);

    let packet_before = fs::read(&output).expect("snapshot existing output");
    let never_launch = fixture.directory.join("must-not-launch-blender.exe");
    assert!(!never_launch.exists());
    let result = import_camera(
        &never_launch,
        &fixture.source,
        &output,
        Request {
            frame: 3.0,
            scene: Some(SCENE),
            camera: None,
        },
    );
    assert!(
        matches!(result, Err(ImportError::OutputExists)),
        "existing output must be refused before any Blender launch"
    );
    assert!(
        fs::read(&output).expect("read protected output") == packet_before,
        "overwrite guard changed the existing packet"
    );
    assert_preserved(fixture);
}

const FIXTURE_SCRIPT: &str = r#"import bpy
import pathlib
import sys

source, sentinel = [pathlib.Path(value) for value in sys.argv[sys.argv.index("--") + 1:]]
assert not source.exists()
assert not sentinel.exists()
bpy.ops.wm.read_factory_settings(use_empty=True)
scene = bpy.context.scene
scene.name = "NexaScene"
scene.view_layers[0].name = "NexaLayer"
scene.render.resolution_x = 800
scene.render.resolution_y = 600
scene.render.resolution_percentage = 100
scene.render.pixel_aspect_x = 1.0
scene.render.pixel_aspect_y = 1.0
scene.render.fps = 24
scene.render.fps_base = 1.001
scene.render.frame_map_old = 100
scene.render.frame_map_new = 100
scene.unit_settings.scale_length = 1.0
scene.frame_start = 1
scene.frame_end = 3

parent = bpy.data.objects.new("NexaParent", None)
target = bpy.data.objects.new("NexaTarget", None)
scene.collection.objects.link(parent)
scene.collection.objects.link(target)
parent.location.x = 0.0
parent.keyframe_insert(data_path="location", index=0, frame=1)
parent.location.x = 10.0
parent.keyframe_insert(data_path="location", index=0, frame=3)

def camera(name, kind, location):
    data = bpy.data.cameras.new(name + "Data")
    data.type = kind
    data.sensor_fit = "HORIZONTAL"
    data.sensor_width = 36.0
    data.sensor_height = 24.0
    data.clip_start = 0.1
    data.clip_end = 1000.0
    obj = bpy.data.objects.new(name, data)
    scene.collection.objects.link(obj)
    obj.location = location
    return obj

perspective = camera("Nexaカメラ", "PERSP", (1.0, 2.0, 3.0))
perspective.parent = parent
constraint = perspective.constraints.new("TRACK_TO")
constraint.target = target
constraint.track_axis = "TRACK_NEGATIVE_Z"
constraint.up_axis = "UP_Y"
perspective.data.lens = 50.0
perspective.data.keyframe_insert(data_path="lens", frame=1)
perspective.data.lens = 70.0
perspective.data.keyframe_insert(data_path="lens", frame=3)
driver = perspective.data.driver_add("shift_x").driver
driver.type = "SCRIPTED"
driver.expression = "frame*0.01"
assert driver.is_simple_expression

orthographic = camera("NexaOrtho", "ORTHO", (-4.0, 5.0, 6.0))
orthographic.data.ortho_scale = 10.0
scene.timeline_markers.new("NexaPerspectiveCut", frame=1).camera = perspective
scene.timeline_markers.new("NexaOrthographicCut", frame=3).camera = orthographic

for action in bpy.data.actions:
    for layer in action.layers:
        for strip in layer.strips:
            for bag in strip.channelbags:
                for fcurve in bag.fcurves:
                    for point in fcurve.keyframe_points:
                        point.interpolation = "LINEAR"

text = bpy.data.texts.new("NexaUntrustedAutoexec")
text.write("import pathlib\npathlib.Path(" + repr(str(sentinel)) + ").write_text('unexpected autoexec', encoding='utf-8')\n")
text.use_module = True
scene.camera = perspective
scene.frame_set(1)
bpy.ops.wm.save_as_mainfile(filepath=str(source), check_existing=False)
assert source.exists()
assert not sentinel.exists()
"#;
#[test]
fn v139_decoder_rejects_empty_names() {
    let names = ["場面🌌", " Layer ", "カメラ"];
    let (control, _, _) = known_packet_with_names(names);
    let sample = decode(&control).expect("nonempty Unicode names must remain valid");
    assert_eq!(
        (
            sample.scene_name.as_str(),
            sample.view_layer_name.as_str(),
            sample.camera_name.as_str(),
        ),
        (names[0], names[1], names[2])
    );

    for (index, field) in ["scene_name", "view_layer_name", "camera_name"]
        .into_iter()
        .enumerate()
    {
        let mut invalid_names = names;
        invalid_names[index] = "";
        let (packet, _, _) = known_packet_with_names(invalid_names);
        assert_eq!(
            decode(&packet).unwrap_err(),
            DecodeError::InvalidValue { field },
            "empty {field}"
        );
    }
}

#[test]
fn v139_decoder_rejects_nul_names() {
    let names = ["KnownScene", "KnownLayer", "Knownカメラ"];
    let (control, _, _) = known_packet_with_names(names);
    decode(&control).expect("NUL-free control must remain valid");

    for (index, field) in ["scene_name", "view_layer_name", "camera_name"]
        .into_iter()
        .enumerate()
    {
        for invalid_name in ["\0", "前\0後"] {
            let mut invalid_names = names;
            invalid_names[index] = invalid_name;
            let (packet, _, _) = known_packet_with_names(invalid_names);
            assert_eq!(
                decode(&packet).unwrap_err(),
                DecodeError::InvalidValue { field },
                "NUL-containing {field}: {invalid_name:?}"
            );
        }
    }
}

#[test]
fn v139_decoder_rejects_reversed_range_but_accepts_equal_bounds() {
    let (mut packet, kind_offset, _) = known_packet();
    let sample = decode(&packet).expect("ordered control range must decode");
    assert_eq!((sample.frame_start, sample.frame_end), (1, 3));

    // Frozen wire layout: frame_start precedes kind by 28 bytes.
    let start_offset = kind_offset - 28;
    packet[start_offset..start_offset + 4].copy_from_slice(&3_i32.to_le_bytes());
    let sample = decode(&packet).expect("a single-frame range must remain valid");
    assert_eq!((sample.frame_start, sample.frame_end), (3, 3));

    packet[start_offset..start_offset + 4].copy_from_slice(&4_i32.to_le_bytes());
    assert_eq!(
        decode(&packet).unwrap_err(),
        DecodeError::InvalidValue { field: "frame_end" }
    );
}
