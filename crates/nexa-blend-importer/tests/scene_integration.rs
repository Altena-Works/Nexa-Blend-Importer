//! Opt-in real Blender gate; every successful selection owns exactly one extraction.
use nexa_blend_importer::{ImportRequest, Selection, packet, pipeline::{self, Options}, source::SourceOptions};
use std::{collections::{BTreeMap, BTreeSet}, env, fs::{self, OpenOptions}, io::Write,
    path::{Path, PathBuf}, sync::atomic::AtomicBool};

fn required(name: &str) -> PathBuf {
    let path = PathBuf::from(env::var_os(name).unwrap_or_else(|| panic!("set {name} explicitly")));
    assert!(path.is_absolute(), "{name} must be absolute");
    path
}

fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, directory: &Path, values: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let metadata = fs::symlink_metadata(entry.path()).unwrap();
            assert!(!metadata.file_type().is_symlink());
            if metadata.is_dir() { visit(root, &entry.path(), values); }
            else { values.insert(entry.path().strip_prefix(root).unwrap().to_owned(), fs::read(entry.path()).unwrap()); }
        }
    }
    let mut values = BTreeMap::new();
    visit(root, root, &mut values);
    values
}

fn entries(root: &Path) -> BTreeSet<PathBuf> {
    fs::read_dir(root).unwrap().map(|e| e.unwrap().path()).collect()
}

#[test]
#[ignore = "requires explicit Blender executable, source .blend, and new output root"]
fn real_blender_seven_selections_and_publication_preservation() {
    let source = required("NEXA_TEST_SOURCE_BLEND");
    let blender = required("NEXA_TEST_BLENDER");
    let root = required("NEXA_TEST_OUTPUT_ROOT");
    assert!(source.is_file() && blender.is_file() && root.is_dir());
    let scratch = root.join("scratch");
    let output = root.join("outputs");
    fs::create_dir(&scratch).unwrap();
    fs::create_dir(&output).unwrap();
    let existing = output.join("existing-output");
    fs::create_dir(&existing).unwrap();
    fs::write(existing.join("keep.bin"), b"preexisting output must remain byte-identical\0\xff").unwrap();
    let mut receipt = OpenOptions::new().write(true).create_new(true).open(root.join("receipt.tsv")).unwrap();
    writeln!(receipt, "case\tmodel\tcamera\tanimation\tdirectory").unwrap();
    let mut targets = Vec::new();
    let selections = [
        ("all", Selection {model: true, camera: true, animation: true}),
        ("model", Selection {model: true, camera: false, animation: false}),
        ("camera", Selection {model: false, camera: true, animation: false}),
        ("model-camera", Selection {model: true, camera: true, animation: false}),
        ("model-animation", Selection {model: true, camera: false, animation: true}),
        ("camera-animation", Selection {model: false, camera: true, animation: true}),
        ("animation", Selection {model: false, camera: false, animation: true}),
    ];
    let base = Options { source: SourceOptions { blender, input: source,
        scratch_parent: scratch.clone(), scene: None, request: ImportRequest::default() },
        output_parent: output.clone(), allow_fov_approximation: true };
    let running = AtomicBool::new(false);
    for (name, selection) in selections {
        let before = files(&output);
        let mut options = base.clone();
        options.source.request.selection = selection;
        let chosen = if selection.animation_only() { targets.as_slice() } else { &[] };
        let result = pipeline::run(&options, chosen, &running)
            .unwrap_or_else(|error| panic!("real Blender selection {name}: {error}"));
        assert!(result.directory.is_dir());
        let mut expected = BTreeSet::new();
        if selection.model {
            expected.extend([PathBuf::from("model_0001.pmx"), PathBuf::from("textures/texture_0001.png")]);
            assert_eq!(result.models, [PathBuf::from("model_0001.pmx")]);
        } else { assert!(result.models.is_empty()); }
        let model_motion = selection.animation && (selection.model || selection.animation_only());
        if model_motion {
            expected.insert(PathBuf::from("model_0001.motion.vmd"));
            assert_eq!(result.motions.len(), 1);
            assert_eq!(result.motions[0].1, PathBuf::from("model_0001.motion.vmd"));
            if selection.animation_only() {
                assert_eq!(fs::canonicalize(&result.motions[0].0).unwrap(), fs::canonicalize(&targets[0]).unwrap());
            }
        } else { assert!(result.motions.is_empty()); }
        if selection.camera { expected.insert(PathBuf::from("camera.vmd")); }
        assert_eq!(result.camera.is_some(), selection.camera);
        if selection.model || selection.animation_only() { expected.insert(PathBuf::from("name-map.tsv")); }
        assert_eq!(files(&result.directory).into_keys().collect::<BTreeSet<_>>(), expected, "{name} components");
        for (relative, content) in before {
            assert_eq!(fs::read(output.join(relative)).unwrap(), content, "{name} modified an existing output");
        }
        assert!(entries(&scratch).is_empty(), "{name} left extraction scratch");
        if name == "all" { targets.push(result.directory.join("model_0001.pmx")); }
        writeln!(receipt, "{name}\t{}\t{}\t{}\t{}", u8::from(selection.model), u8::from(selection.camera),
            u8::from(selection.animation), result.directory.display()).unwrap();
        receipt.flush().unwrap();
        println!("selection={name} output={}", result.directory.display());
    }

    // Pre-cancellation precedes executable validation and spawning. A nonexistent executable
    // gives a deterministic distinction from both a launched process and path inspection.
    let before = files(&output);
    let before_entries = entries(&output);
    let mut options = base.clone();
    options.source.blender = root.join("must-not-launch.exe");
    let error = pipeline::run(&options, &[], &AtomicBool::new(true)).err().expect("cancellation must fail");
    assert_eq!(error, "conversion cancelled");
    assert_eq!(files(&output), before);
    assert_eq!(entries(&output), before_entries);
    assert!(entries(&scratch).is_empty());
    println!("pre_cancel=passed zero_child=true output_unchanged=true");

    // Independent post-extraction failure: invalid existing PMX must roll back its newly
    // created staging and preserve all seven successful folders and existing entries.
    let packet_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/m1-scene-export-20261021/export-01/scene.nexascn");
    let scene = packet::read_packet(&packet_path).unwrap();
    let invalid = root.join("existing-invalid.pmx");
    let protected = b"existing user PMX placeholder, never replace";
    fs::write(&invalid, protected).unwrap();
    options = base;
    options.source.request.selection = Selection {model: false, camera: false, animation: true};
    let error = pipeline::publish(&scene, &options, &[invalid.clone()], &running).err().expect("bad PMX must fail");
    assert!(error.contains("PMX"), "unexpected target error: {error}");
    assert_eq!(fs::read(&invalid).unwrap(), protected);
    assert_eq!(files(&output), before);
    assert_eq!(entries(&output), before_entries);
    assert!(entries(&scratch).is_empty());
    println!("invalid_existing_pmx=passed output_unchanged=true reason={error}");
    fs::write(root.join("protection.txt"), "seven selections passed\npre-cancel: zero Blender child, no output\ninvalid existing PMX: failed, no partial publication\nall previous outputs: byte-identical\n").unwrap();
}

/// Targeted continuation after a diagnosed matcher fix. This uses the independently
/// checked M1 scene packet and the PMX already published by the all-ON Blender request.
/// It does not repeat source extraction or any of the six completed selections.
#[test]
#[ignore = "requires explicit existing generated PMX and a fresh output root"]
fn saved_packet_animation_only_and_publication_protection() {
    let root = required("NEXA_TEST_OUTPUT_ROOT");
    let target = required("NEXA_TEST_EXISTING_PMX");
    assert!(root.is_dir() && target.is_file());
    let previous_output = target.parent().unwrap().parent().unwrap();
    let previous_files = files(previous_output);
    let previous_entries = entries(previous_output);
    let target_bytes = fs::read(&target).unwrap();
    let output = root.join("outputs");
    let scratch = root.join("scratch");
    fs::create_dir(&output).unwrap();
    fs::create_dir(&scratch).unwrap();
    let options = Options { source: SourceOptions {
        blender: required("NEXA_TEST_BLENDER"), input: required("NEXA_TEST_SOURCE_BLEND"),
        scratch_parent: scratch.clone(), scene: None, request: ImportRequest {
            selection: Selection { model: false, camera: false, animation: true },
            ..ImportRequest::default()
        } }, output_parent: output.clone(), allow_fov_approximation: true };
    let packet_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/m1-scene-export-20261021/export-01/scene.nexascn");
    let packet = packet::read_packet(&packet_path).unwrap();
    let running = AtomicBool::new(false);
    let result = pipeline::publish(&packet, &options, &[target.clone()], &running)
        .unwrap_or_else(|error| panic!("saved-packet animation-only publication: {error}"));
    assert!(result.models.is_empty() && result.camera.is_none());
    assert_eq!(result.motions.len(), 1);
    assert_eq!(fs::canonicalize(&result.motions[0].0).unwrap(), fs::canonicalize(&target).unwrap());
    assert_eq!(files(&result.directory).into_keys().collect::<BTreeSet<_>>(),
        BTreeSet::from([PathBuf::from("model_0001.motion.vmd"), PathBuf::from("name-map.tsv")]));
    let mut receipt = OpenOptions::new().write(true).create_new(true).open(root.join("receipt.tsv")).unwrap();
    writeln!(receipt, "case\tmodel\tcamera\tanimation\tdirectory").unwrap();
    writeln!(receipt, "animation\t0\t0\t1\t{}", result.directory.display()).unwrap();
    receipt.flush().unwrap();
    let completed_files = files(&output);
    let completed_entries = entries(&output);
    let mut cancelled_options = options.clone();
    cancelled_options.source.blender = root.join("must-not-launch.exe");
    assert_eq!(pipeline::run(&cancelled_options, &[target.clone()], &AtomicBool::new(true)).err().unwrap(), "conversion cancelled");
    assert_eq!(files(&output), completed_files);
    assert_eq!(entries(&output), completed_entries);
    let invalid = root.join("existing-invalid.pmx");
    let invalid_bytes = b"existing PMX placeholder to preserve";
    fs::write(&invalid, invalid_bytes).unwrap();
    let error = pipeline::publish(&packet, &options, &[invalid.clone()], &running).err().expect("invalid PMX must fail");
    assert!(error.contains("PMX"), "unexpected error: {error}");
    assert_eq!(fs::read(&invalid).unwrap(), invalid_bytes);
    assert_eq!(files(&output), completed_files);
    assert_eq!(entries(&output), completed_entries);
    assert_eq!(files(previous_output), previous_files);
    assert_eq!(entries(previous_output), previous_entries);
    assert_eq!(fs::read(&target).unwrap(), target_bytes);
    assert!(entries(&scratch).is_empty());
    fs::write(root.join("protection.txt"), "saved-packet animation-only: passed (no Blender process)\npre-cancel: zero child and no output\ninvalid existing PMX: rollback passed\nall six previous outputs and existing PMX: byte-identical\n").unwrap();
    println!("saved_packet_animation_only=passed blender_children=0 output={}", result.directory.display());
    println!("pre_cancel=passed invalid_existing_pmx=passed previous_six_and_pmx_unchanged=true");
}
