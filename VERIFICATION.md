# Verification — 0.3.0 publication candidate

The initial supported scope has bounded M0–M5 acceptance evidence. Publication checks below are complete for the v0.3.0 preview candidate; no release has been published. The tested environment is Windows x64, Rust 1.95.0/MSVC, Blender 4.5.4 and MMD 9.32 x64. This is not a guarantee for arbitrary scenes or long sessions.

- Modal-loop reentry into GUI state is prevented by a RefCell guard; close requests are deferred until the active borrow ends. The dedicated hidden-window lifecycle test passes.
- The current release build links the CRT statically. Its EXE and DLL imports contain no separately installed VC++ runtime dependency; packaging rejects a regression to that dependency.
- All 45 unit tests pass. The combined run initially encountered two Windows access denials in hardlink creation and directory publication; each unchanged test passed in one explicitly authorized rerun through automatic approval review. Three opt-in library tests were ignored, with real-Blender/lifecycle evidence recorded separately. No tolerance was loosened.
- Additional real-Blender evidence covers Japanese paths, concave untextured meshes, two static models, explicit negative frames at 24fps and missing-camera rejection. Six motion ticks have maximum translation error 4.77e-8; twelve existing files remain unchanged. These cases used the previous release executable; conversion source is unchanged. The new static-CRT executable also passed the real-Blender all-components GUI conversion and independent output decoding: two models, three camera keys, skin error 2.23e-6, rotation 1.97e-8 rad and morph 1.20e-8.
- Fresh ZIP installation passed native MMD startup, English menu/picker/GUI, automatic Blender detection, conversion, modal completion dismissal, worker exit, MMD frame navigation, worker relaunch/picker cancellation and normal MMD exit. The installed EXE/DLL hashes match the release build. No hang occurred in this bounded smoke session.
- A previously reported MMD hang/exit has no confirmed cause. A later PMM load and frame change succeeded; this does not establish long-session stability. Windows Application Error events could not be read because access was denied and were not retried.

| Area | Evidence | State |
| --- | --- | --- |
| Scene extraction | Real Blender scene with rig, seam UVs, material/image, morph, three poses and a perspective/orthographic camera cut; source unchanged | Passed |
| PMX/VMD output | Six selection combinations independently decoded; three-tick skin position error ≤2.23e-6, rotation error ≤1.97e-8 rad, morph error ≤1.20e-8 | Passed for this fixture |
| Animation-only | Matching/mismatching reference axes tested. A fresh real-Blender request and independent three-tick VMD decoding pass: position ≤2.23e-6, rotation ≤1.97e-8 rad, morph ≤1.20e-8; input and 19 existing files unchanged | Passed for this fixture; all seven selections have real-Blender success evidence |
| Publication | Fixed transaction test; existing output protection | Passed |
| Child lifecycle | Cancellation and shortened timeout reclaim the owned child | Passed |
| Interrupted assets | Generated scratch PNGs removed, unknown files preserved | Passed |
| Unsupported scene data | Weighted segmented B-Bones and mid-range unsupported modifiers rejected; connected/unconnected 8-bit RGBA image-alpha variants independently pixel-decoded | Passed for dedicated fixtures |
| GUI entry | Dedicated MMD menu → blend-only standard picker → Japanese three-component screen; Blender detected | Passed |
| GUI input guard | All-OFF refusal confirmed manually in the dedicated MMD/worker setup | Passed |
| Localization | English picker title, component screen and details observed; native menu follows English-to-Japanese switching without restart | Passed for the tested paths |
| GUI cancellation | Before-conversion Cancel closes the screen. During an import with range 1–108000, Cancel returns to the incomplete state, publishes no output and preserves eight protected files. No Blender process remained; active-child termination is covered separately by the lifecycle test | Passed for the tested GUI paths |
| Native asset loading and motion | Two generated models, model-motion and camera VMD loaded through standard menus. Three native frames show textured rig deformation. Independent analytic vertex projections match interior/exterior red-material coverage with zero mismatches beyond a 1px edge exclusion. A frozen-pose control produces 6043 mismatches. World-space position/rotation/morph tolerances are established separately by independent output decoding | Passed for this fixture; raster coverage is not a world-space precision measurement |
| Rendered projection | Three native 640×480 BMPs; all 12 perspective/orthographic points pass, maximum 0.163332 px. Area/arm shape checks scale with known projected marker dimensions; position tolerance remains 0.5 px, with no resampling or bias. Correct, shifted-by-0.75px and missing-marker controls pass | Passed for this fixture |

Tests cover finite fixtures, not arbitrary Blender scenes. Unsupported selected information must fail the import rather than disappear silently. Package integrity and a successful build do not establish MMD runtime compatibility.
