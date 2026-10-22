"""Prepare and independently evaluate the bounded M5 native-MMD scene fixture.

The rig follows blender_scene_fixture.py. No exporter or product modules are used.
This tool creates no PMX/VMD and does not launch or qualify MMD.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import struct
import subprocess
import sys


BLENDER = Path('C:/Program Files/Blender Foundation/Blender 4.5/blender.exe')
PYTHON = Path(sys.executable)
ROOT_BONE = '日本語の長い親ボーン名称'
CHILD_BONE = '先端の子ボーン'
WIDTH, HEIGHT = 640, 480
CAMERA_POSITION = (2.0, -6.0, 4.0)
RIGHT = (1.0, 0.0, 0.0)
UP = (0.0, 0.5, math.sqrt(3.0) / 2.0)
BACK = (0.0, -math.sqrt(3.0) / 2.0, 0.5)
SENSOR_WIDTH, ORTHO_WIDTH = 36.0, 6.0
LENS_MODE = 'exact'
LENS = (SENSOR_WIDTH * HEIGHT / WIDTH) / (2 * math.tan(math.radians(15)))
FOCAL_PIXELS = WIDTH * LENS / SENSOR_WIDTH
CONVERTER_FOV_TOLERANCE_DEGREES = 1e-4
MARKERS = (
    (7.5, (152.25, 111.75), (1.0, 0.0, 0.0)),
    (7.8, (487.75, 128.25), (0.0, 1.0, 0.0)),
    (8.1, (173.5, 357.5), (0.0, 0.0, 1.0)),
    (8.4, (464.5, 339.5), (1.0, 0.0, 1.0)),
)
POSITION_TOLERANCE = 1e-4
ROTATION_TOLERANCE = 1e-4
WEIGHT_TOLERANCE = 1e-6
PIXEL_TOLERANCE = 0.5


def write_json(path, value):
    with path.open('x', encoding='utf-8') as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2, allow_nan=False)
        stream.write('\n')


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def fov_preflight():
    # Blender camera lens and projection matrices are stored as float32.
    lens_f32 = struct.unpack('<f', struct.pack('<f', LENS))[0]
    sensor_height = SENSOR_WIDTH * HEIGHT / WIDTH
    fy_f32 = struct.unpack('<f', struct.pack('<f', 2*lens_f32/sensor_height))[0]
    value = math.degrees(2 * math.atan(1/fy_f32))
    integer = round(value)
    result = dict(mode=LENS_MODE, requested_lens_mm=LENS, float32_lens_mm=lens_f32,
                  ideal_vertical_fov_degrees=math.degrees(2*math.atan(sensor_height/(2*LENS))),
                  predicted_float32_projection_y=fy_f32, predicted_vertical_fov_degrees=value,
                  nearest_integer_fov_degrees=integer, integer_error_degrees=abs(value-integer),
                  converter_approximation_threshold_degrees=CONVERTER_FOV_TOLERANCE_DEGREES,
                  requires_approximation=abs(value-integer) > CONVERTER_FOV_TOLERANCE_DEGREES)
    if LENS_MODE == 'exact':
        assert integer == 30 and not result['requires_approximation'], result
    return result


def multiply(a, b):
    return [sum(a[4*r+k] * b[4*k+c] for k in range(4)) for r in range(4) for c in range(4)]


def translation(x, y=0.0, z=0.0):
    return [1, 0, 0, x, 0, 1, 0, y, 0, 0, 1, z, 0, 0, 0, 1]


def rotation(angle):
    c, s = math.cos(angle), math.sin(angle)
    return [c, -s, 0, 0, s, c, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]


def point(matrix, vector):
    return [sum(matrix[4*r+c] * vector[c] for c in range(3)) + matrix[4*r+3] for r in range(3)]


def marker_world(depth, xy):
    offsets = ((xy[0] - WIDTH/2) * depth / FOCAL_PIXELS,
               (HEIGHT/2 - xy[1]) * depth / FOCAL_PIXELS)
    return [CAMERA_POSITION[i] + RIGHT[i]*offsets[0] + UP[i]*offsets[1] - BACK[i]*depth for i in range(3)]


def project(world, projection):
    relative = [world[i] - CAMERA_POSITION[i] for i in range(3)]
    x = sum(relative[i]*RIGHT[i] for i in range(3))
    y = sum(relative[i]*UP[i] for i in range(3))
    depth = -sum(relative[i]*BACK[i] for i in range(3))
    pixels_per_unit = FOCAL_PIXELS / depth if projection == 'PERSP' else WIDTH / ORTHO_WIDTH
    return [WIDTH/2 + x*pixels_per_unit, HEIGHT/2 - y*pixels_per_unit]


def analytic_expectations():
    rows, rig_frames = [], []
    for tick in range(3):
        projection = 'PERSP' if tick == 0 else 'ORTHO'
        for index, (depth, xy, rgb) in enumerate(MARKERS):
            world = marker_world(depth, xy)
            size_ratio = 1.0 if tick == 0 else depth * WIDTH / ORTHO_WIDTH / FOCAL_PIXELS
            rows.append(dict(name=f'F{tick}_P{index}', frame=tick, source_frame=tick+1,
                             point_index=index, projection=projection, source_world=world,
                             mmd_world=[world[0], world[2], world[1]], expected_xy=project(world, projection),
                             depth=depth, rgb=rgb, half_arm_source=8*depth/FOCAL_PIXELS,
                             half_thickness_source=1.5*depth/FOCAL_PIXELS,
                             half_arm_pixels=8*size_ratio, half_thickness_pixels=1.5*size_ratio))
        ratio = tick / 2
        world = multiply(translation(2+0.4*ratio), rotation(0.1*ratio))
        root_skin = multiply(multiply(world, rotation(0.2*ratio)), translation(-2))
        child_skin = multiply(multiply(multiply(world, rotation(0.2*ratio)), translation(0.2*ratio)), translation(-2))
        local_vertices = [(0, 0, 0.24*ratio), (1, 0, 0), (1, 1, 0.08*ratio), (0, 1, 0)]
        vertices = []
        for index, vertex in enumerate(local_vertices):
            reference_world = (vertex[0]+2, vertex[1], vertex[2])
            parent_point, child_point = point(root_skin, reference_world), point(child_skin, reference_world)
            if index == 0:
                value = parent_point
            elif index == 1:
                value = [0.25*a + 0.75*b for a, b in zip(parent_point, child_point)]
            elif index == 2:
                value = child_point
            else:
                value = point(world, vertex)
            vertices.append(value)
        rig_frames.append(dict(frame=tick, source_frame=tick+1, root_world=world,
                               bone_skin_world=[root_skin, child_skin], morph_weight=0.8*ratio,
                               source_world_vertices=vertices,
                               mmd_world_vertices=[[p[0], p[2], p[1]] for p in vertices]))
    return dict(schema='NexaM5NativeFixture1', width=WIDTH, height=HEIGHT, frames=[0, 1, 2],
                source_frames=[1, 2, 3], units_per_meter=1.0, scene_unit_scale=1.0,
                camera_position=CAMERA_POSITION, camera_rotation_euler=[math.pi/3, 0, 0],
                camera_right=RIGHT, camera_up=UP, camera_back=BACK,
                lens_mode=LENS_MODE, lens_mm=LENS, sensor_width_mm=SENSOR_WIDTH, sensor_fit='HORIZONTAL',
                fov_preflight=fov_preflight(),
                ortho_width=ORTHO_WIDTH, pixel_aspect=[1, 1], markers=rows, rig_frames=rig_frames,
                expected_source='Independent analytic scene constants; no product exporter or VMD inverse',
                pixel_center_convention='D3D9 integer pixel centers; no raster bias correction',
                tolerances=dict(position=POSITION_TOLERANCE, rotation_radians=ROTATION_TOLERANCE,
                                weights_and_morph=WEIGHT_TOLERANCE, camera_pixels=PIXEL_TOLERANCE),
                model_groups=['FixtureRig', 'ProjectionLandmarks'],
                preserved_features=['UV seam', 'BDEF1', 'BDEF2 0.25/0.75', 'unweighted generated root',
                                    'parent', 'COPY_ROTATION constraint', 'root/bone/morph animation', 'packed RGBA PNG'],
                native_render='not_run')


def create(directory):
    import bpy
    from mathutils import Matrix

    blend = directory / 'm5-native.blend'
    if blend.exists() or (directory / 'expectations.json').exists():
        raise FileExistsError('fixture destination exists; no overwrite')
    bpy.ops.wm.read_factory_settings(use_empty=True)
    scene = bpy.context.scene
    scene.name = 'NexaM5NativeScene'
    scene.frame_start, scene.frame_end = 1, 3
    scene.render.fps, scene.render.fps_base = 30, 1.0
    scene.render.resolution_x, scene.render.resolution_y, scene.render.resolution_percentage = WIDTH, HEIGHT, 100
    scene.render.pixel_aspect_x = scene.render.pixel_aspect_y = 1.0
    scene.render.image_settings.color_mode = 'RGBA'
    scene.unit_settings.scale_length = 1.0
    mover = bpy.data.objects.new('Mover', None)
    turn = bpy.data.objects.new('ConstraintRotationTarget', None)
    scene.collection.objects.link(mover)
    scene.collection.objects.link(turn)
    for frame, displacement, angle in [(1, 0.0, 0.0), (3, 0.4, 0.1)]:
        mover.location.x = displacement
        mover.keyframe_insert('location', index=0, frame=frame)
        turn.rotation_euler.z = angle
        turn.keyframe_insert('rotation_euler', index=2, frame=frame)
    armature = bpy.data.armatures.new('FixtureArmature')
    rig = bpy.data.objects.new('FixtureRig', armature)
    scene.collection.objects.link(rig)
    rig.parent = mover
    rig.location = (2, 0, 0)
    constraint = rig.constraints.new('COPY_ROTATION')
    constraint.target = turn
    constraint.owner_space = constraint.target_space = 'WORLD'
    bpy.context.view_layer.objects.active = rig
    rig.select_set(True)
    bpy.ops.object.mode_set(mode='EDIT')
    parent = armature.edit_bones.new(ROOT_BONE)
    parent.head, parent.tail = (0, 0, 0), (0, 1, 0)
    child = armature.edit_bones.new(CHILD_BONE)
    child.head, child.tail, child.parent = (0, 1, 0), (0, 2, 0), parent
    bpy.ops.object.mode_set(mode='OBJECT')
    root_pose, child_pose = rig.pose.bones[ROOT_BONE], rig.pose.bones[CHILD_BONE]
    root_pose.rotation_mode = 'XYZ'
    for frame, angle, displacement in [(1, 0.0, 0.0), (3, 0.2, 0.2)]:
        root_pose.rotation_euler.z = angle
        root_pose.keyframe_insert('rotation_euler', index=2, frame=frame)
        child_pose.location.x = displacement
        child_pose.keyframe_insert('location', index=0, frame=frame)
    mesh = bpy.data.meshes.new('SeamMeshData')
    mesh.from_pydata([(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0)], [], [(0, 1, 2), (0, 2, 3)])
    mesh.update()
    obj = bpy.data.objects.new('SeamMesh', mesh)
    scene.collection.objects.link(obj)
    obj.parent, obj.matrix_parent_inverse = rig, Matrix.Identity(4)
    uv = mesh.uv_layers.new(name='UVMap')
    for loop, value in zip(uv.data, [(0, 0), (1, 0), (1, 1), (0.25, 0), (1, 1), (0, 1)]):
        loop.uv = value
    root_group, child_group = obj.vertex_groups.new(name=ROOT_BONE), obj.vertex_groups.new(name=CHILD_BONE)
    root_group.add([0], 1.0, 'REPLACE')
    root_group.add([1], 0.25, 'REPLACE')
    child_group.add([1], 0.75, 'REPLACE')
    child_group.add([2], 1.0, 'REPLACE')
    modifier = obj.modifiers.new('Armature', 'ARMATURE')
    modifier.object = rig
    modifier.use_vertex_groups, modifier.use_bone_envelopes = True, False
    modifier.use_deform_preserve_volume = False
    obj.shape_key_add(name='Basis')
    key = obj.shape_key_add(name='持上げ')
    key.data[0].co.z += 0.3
    key.data[2].co.z += 0.1
    for frame, value in [(1, 0.0), (3, 0.8)]:
        key.value = value
        key.keyframe_insert('value', frame=frame)
    image = bpy.data.images.new('PackedRedRGBA', width=2, height=2, alpha=True)
    image.pixels.foreach_set([1, 0, 0, 1, 1, 0, 0, 0.75, 1, 0, 0, 0.5, 1, 0, 0, 1])
    image.file_format, image.alpha_mode = 'PNG', 'STRAIGHT'
    image.pack()
    material = bpy.data.materials.new('SimplePrincipledRGBA')
    material.use_nodes = True
    texture = material.node_tree.nodes.new('ShaderNodeTexImage')
    texture.image = image
    shader = material.node_tree.nodes.get('Principled BSDF')
    material.node_tree.links.new(texture.outputs['Color'], shader.inputs['Base Color'])
    material.node_tree.links.new(texture.outputs['Alpha'], shader.inputs['Alpha'])
    obj.data.materials.append(material)
    vertices, faces = [], []
    for index, (depth, xy, rgb) in enumerate(MARKERS):
        center = marker_world(depth, xy)
        a, t = 8*depth/FOCAL_PIXELS, 1.5*depth/FOCAL_PIXELS
        outline = [(-a, -t), (-t, -t), (-t, -a), (t, -a), (t, -t), (a, -t),
                   (a, t), (t, t), (t, a), (-t, a), (-t, t), (-a, t)]
        begin = len(vertices)
        vertices.extend(tuple(center[i]+RIGHT[i]*x+UP[i]*y for i in range(3)) for x, y in outline)
        faces.append(tuple(range(begin, begin+len(outline))))
    landmark_data = bpy.data.meshes.new('ProjectionCrossesData')
    landmark_data.from_pydata(vertices, [], faces)
    landmark_data.update()
    landmark = bpy.data.objects.new('ProjectionLandmarks', landmark_data)
    scene.collection.objects.link(landmark)
    for index, (_, _, rgb) in enumerate(MARKERS):
        mat = bpy.data.materials.new(f'ProjectionColor{index}')
        mat.use_nodes = True
        mat.node_tree.nodes.get('Principled BSDF').inputs['Base Color'].default_value = (*rgb, 1)
        landmark_data.materials.append(mat)
        landmark_data.polygons[index].material_index = index
    cameras = []
    for name, projection in [('CameraA', 'PERSP'), ('CameraB', 'ORTHO')]:
        data = bpy.data.cameras.new(name)
        data.type, data.lens, data.ortho_scale = projection, LENS, ORTHO_WIDTH
        data.sensor_fit, data.sensor_width, data.sensor_height = 'HORIZONTAL', SENSOR_WIDTH, 24.0
        data.shift_x = data.shift_y = 0
        camera = bpy.data.objects.new(name, data)
        scene.collection.objects.link(camera)
        camera.location, camera.rotation_euler = CAMERA_POSITION, (math.pi/3, 0, 0)
        cameras.append(camera)
    scene.camera = cameras[0]
    scene.timeline_markers.new('Perspective start', frame=1).camera = cameras[0]
    scene.timeline_markers.new('Orthographic cut', frame=2).camera = cameras[1]
    for owner in (mover, turn, rig, obj.data.shape_keys):
        action = owner.animation_data.action
        curves = [curve for layer in action.layers for strip in layer.strips
                  for bag in getattr(strip, 'channelbags', ()) for curve in bag.fcurves]
        for curve in curves or list(action.fcurves):
            for point_key in curve.keyframe_points:
                point_key.interpolation = 'LINEAR'
    scene.frame_set(1)
    bpy.ops.wm.save_as_mainfile(filepath=str(blend), check_existing=True)
    expected = analytic_expectations()
    expected.update(input_blend=str(blend), input_sha256=sha(blend), blender_version=list(bpy.app.version))
    write_json(directory / 'expectations.json', expected)
    with (directory / 'input-sha256.txt').open('x', encoding='ascii') as stream:
        stream.write(expected['input_sha256']+'\n')
    print(json.dumps(dict(status='fixture_created', input_blend=str(blend), input_sha256=expected['input_sha256'],
                          native_render='not_run')))


def maximum_error(actual, expected):
    assert len(actual) == len(expected)
    return max(abs(a-b) for a, b in zip(actual, expected))


def rotation_error(actual, expected):
    relative = [[sum(actual[4*k+r]*expected[4*k+c] for k in range(3)) for c in range(3)] for r in range(3)]
    sine = math.sqrt(sum(v*v for v in (relative[2][1]-relative[1][2], relative[0][2]-relative[2][0],
                                     relative[1][0]-relative[0][1]))) / 2
    cosine = (sum(relative[i][i] for i in range(3))-1) / 2
    return abs(math.atan2(sine, cosine))


def check(directory):
    import bpy
    from bpy_extras.object_utils import world_to_camera_view
    from mathutils import Vector

    saved = json.loads((directory / 'expectations.json').read_text(encoding='utf-8'))
    assert sha(directory / 'm5-native.blend') == saved['input_sha256'], 'input changed'
    expected = analytic_expectations()
    for key, value in expected.items():
        assert json.loads(json.dumps(value)) == saved[key], ('changed analytic expectation', key)
    scene, rig, mesh_obj = bpy.context.scene, bpy.data.objects['FixtureRig'], bpy.data.objects['SeamMesh']
    assert (scene.render.resolution_x, scene.render.resolution_y, scene.render.resolution_percentage) == (640, 480, 100)
    assert (scene.render.pixel_aspect_x, scene.render.pixel_aspect_y) == (1, 1)
    assert scene.unit_settings.scale_length == 1 and (scene.frame_start, scene.frame_end) == (1, 3)
    assert len([o for o in scene.objects if o.type == 'MESH' and not o.hide_render]) == 2
    assert len(mesh_obj.data.vertices) == 4 and len(mesh_obj.data.polygons) == 2
    assert [tuple(loop.uv) for loop in mesh_obj.data.uv_layers.active.data] == [(0, 0), (1, 0), (1, 1), (.25, 0), (1, 1), (0, 1)]
    assert [[(g.group, g.weight) for g in v.groups] for v in mesh_obj.data.vertices] == [[(0, 1)], [(0, .25), (1, .75)], [(1, 1)], []]
    png = bytes(bpy.data.images['PackedRedRGBA'].packed_file.data)
    assert png[:8] == b'\x89PNG\r\n\x1a\n' and struct.unpack('>IIBB', png[16:26]) == (2, 2, 8, 6), 'packed image must be RGBA PNG'
    positions, rotations, morphs, projections, rows, camera_matrices = [], [], [], [], [], []
    for frame in saved['rig_frames']:
        tick = frame['frame']
        scene.frame_set(frame['source_frame'])
        depsgraph = bpy.context.evaluated_depsgraph_get()
        evaluated_rig = rig.evaluated_get(depsgraph)
        world = [v for row in evaluated_rig.matrix_world for v in row]
        positions.append(maximum_error(world, frame['root_world']))
        rotations.append(rotation_error(world, frame['root_world']))
        for index, bone_name in enumerate((ROOT_BONE, CHILD_BONE)):
            skin = evaluated_rig.matrix_world @ evaluated_rig.pose.bones[bone_name].matrix @ rig.data.bones[bone_name].matrix_local.inverted()
            from mathutils import Matrix
            skin = skin @ Matrix(((1, 0, 0, -2), (0, 1, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1)))
            actual = [v for row in skin for v in row]
            positions.append(maximum_error(actual, frame['bone_skin_world'][index]))
            rotations.append(rotation_error(actual, frame['bone_skin_world'][index]))
        evaluated = mesh_obj.evaluated_get(depsgraph)
        mesh = evaluated.to_mesh()
        try:
            for index, vertex in enumerate(mesh.vertices):
                actual = list(evaluated.matrix_world @ vertex.co)
                positions.append(maximum_error(actual, frame['source_world_vertices'][index]))
        finally:
            evaluated.to_mesh_clear()
        morphs.append(abs(mesh_obj.data.shape_keys.key_blocks['持上げ'].value-frame['morph_weight']))
        assert scene.camera.name == ('CameraA' if tick == 0 else 'CameraB')
        assert scene.camera.data.type == ('PERSP' if tick == 0 else 'ORTHO')
        evaluated_camera = scene.camera.evaluated_get(depsgraph)
        projection = evaluated_camera.calc_matrix_camera(depsgraph, x=WIDTH, y=HEIGHT, scale_x=1, scale_y=1)
        camera_row = dict(frame=tick, source_frame=tick+1, projection=scene.camera.data.type,
                          saved_lens_mm=evaluated_camera.data.lens,
                          saved_world_matrix=[list(row) for row in evaluated_camera.matrix_world],
                          saved_projection_matrix=[list(row) for row in projection])
        if tick == 0:
            fov = math.degrees(2*math.atan(1/projection[1][1]))
            error = abs(fov-round(fov))
            camera_row.update(vertical_fov_degrees=fov, nearest_integer_fov_degrees=round(fov),
                              integer_error_degrees=error,
                              converter_approximation_threshold_degrees=CONVERTER_FOV_TOLERANCE_DEGREES,
                              requires_approximation=error > CONVERTER_FOV_TOLERANCE_DEGREES)
            if LENS_MODE == 'exact':
                assert round(fov) == 30 and error <= CONVERTER_FOV_TOLERANCE_DEGREES, camera_row
        camera_matrices.append(camera_row)
        for marker in [m for m in saved['markers'] if m['frame'] == tick]:
            projected = world_to_camera_view(scene, scene.camera, Vector(marker['source_world']))
            actual = [projected.x*WIDTH, (1-projected.y)*HEIGHT]
            error = math.dist(actual, marker['expected_xy'])
            projections.append(error)
            rows.append(dict(name=marker['name'], source_frame=tick+1, projection=marker['projection'],
                             analytic_xy=marker['expected_xy'], blender_xy=actual, error_px=error))
    assert max(positions) <= POSITION_TOLERANCE, ('position', max(positions))
    assert max(rotations) <= ROTATION_TOLERANCE, ('rotation', max(rotations))
    assert max(morphs) <= WEIGHT_TOLERANCE, ('morph', max(morphs))
    assert max(projections) <= PIXEL_TOLERANCE, ('camera', max(projections))
    assert sha(directory / 'm5-native.blend') == saved['input_sha256'], 'check changed input'
    report = dict(status='passed', input_sha256=saved['input_sha256'], input_unchanged=True,
                  evidence='Saved Blender fixture evaluation against independent analytic constants',
                  maximum_position_error=max(positions), maximum_rotation_error_radians=max(rotations),
                  maximum_morph_weight_error=max(morphs), maximum_projection_error_px=max(projections),
                  camera_rows=rows, tolerances=saved['tolerances'], groups=2, ticks=3, rgba_png=True,
                  lens_mode=LENS_MODE, fov_preflight=fov_preflight(), saved_camera_matrices=camera_matrices,
                  native_render='not_run', pmx_vmd_export='not_run')
    write_json(directory / 'blender-evaluation.json', report)
    print(json.dumps(report, ensure_ascii=False))


def prepare(directory):
    assert sys.version_info >= (3, 10), 'Python 3.10 or newer is required'
    assert sys.flags.isolated, 'Python -I is required'
    directory.mkdir(parents=True, exist_ok=False)
    resources, extensions = directory / 'runtime-user', directory / 'runtime-extensions'
    resources.mkdir()
    extensions.mkdir()
    environment = os.environ.copy()
    environment.update(BLENDER_USER_RESOURCES=str(resources), BLENDER_USER_EXTENSIONS=str(extensions))
    script = Path(__file__).resolve()
    base = [str(BLENDER), '--background', '--factory-startup', '--disable-autoexec', '--python-exit-code', '1']
    commands = [('create', base + ['--python', str(script), '--', '--mode', 'create', '--directory', str(directory), '--lens-mode', LENS_MODE]),
                ('evaluate', base + [str(directory/'m5-native.blend'), '--python', str(script), '--', '--mode', 'check', '--directory', str(directory), '--lens-mode', LENS_MODE]),
                ('comparator-self-test', [str(PYTHON), '-I', str(script.with_name('mmd_projection_check.py')),
                                          str(directory/'expectations.json'), '--self-test'])]
    source_paths = [script, script.with_name('mmd_projection_check.py'),
                    script.parents[2]/'crates'/'nexa-blend-camera'/'src'/'convert.rs']
    receipt = dict(child_environment_overrides={k: environment[k] for k in ('BLENDER_USER_RESOURCES', 'BLENDER_USER_EXTENSIONS')},
                   fov_preflight=fov_preflight(),
                   source_provenance=[dict(path=str(path), sha256=sha(path)) for path in source_paths],
                   commands=[], native_render='not_run')
    for name, command in commands:
        result = subprocess.run(command, env=environment, capture_output=True, text=True, encoding='utf-8', errors='replace', timeout=180)
        (directory/f'{name}.stdout.log').write_text(result.stdout, encoding='utf-8')
        (directory/f'{name}.stderr.log').write_text(result.stderr, encoding='utf-8')
        receipt['commands'].append(dict(name=name, argv=command, returncode=result.returncode))
        if result.returncode != 0:
            receipt['status'] = 'stopped_command_failure_no_retry'
            write_json(directory/'prepare-receipt.json', receipt)
            raise RuntimeError(f'{name} failed ({result.returncode}); stopped without retry; see saved logs')
    receipt['status'] = 'fixture_and_analytic_evaluation_passed'
    write_json(directory/'prepare-receipt.json', receipt)
    print(json.dumps(receipt, ensure_ascii=False, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--mode', choices=['prepare', 'create', 'check'], required=True)
    parser.add_argument('--directory', type=Path, required=True)
    parser.add_argument('--lens-mode', choices=['exact', 'approx'], default='exact')
    args = parser.parse_args(sys.argv[sys.argv.index('--')+1:] if '--' in sys.argv else None)
    global LENS_MODE, LENS, FOCAL_PIXELS
    LENS_MODE = args.lens_mode
    LENS = ((SENSOR_WIDTH*HEIGHT/WIDTH)/(2*math.tan(math.radians(15)))) if LENS_MODE == 'exact' else 50.0
    FOCAL_PIXELS = WIDTH*LENS/SENSOR_WIDTH
    {'prepare': prepare, 'create': create, 'check': check}[args.mode](args.directory.resolve())


if __name__ == '__main__':
    main()
