"""Create one Blender scene fixture, or independently decode/check its NEXASCN1 export."""
import argparse
import hashlib
import json
import math
from pathlib import Path
import struct
import sys
import zlib


ROOT_BONE = '日本語の長い親ボーン名称'
CHILD_BONE = '先端の子ボーン'


def create_fixture(path):
    import bpy
    from mathutils import Matrix

    if path.exists():
        raise FileExistsError(path)
    bpy.ops.wm.read_factory_settings(use_empty=True)
    scene = bpy.context.scene
    scene.name = 'NexaFixtureScene'
    scene.frame_start, scene.frame_end = 1, 3
    scene.render.fps, scene.render.fps_base = 30, 1.0
    scene.render.resolution_x, scene.render.resolution_y, scene.render.resolution_percentage = 64, 48, 50
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
    constraint.owner_space = 'WORLD'
    constraint.target_space = 'WORLD'
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
    for frame, angle, translation in [(1, 0.0, 0.0), (3, 0.2, 0.2)]:
        root_pose.rotation_euler.z = angle
        root_pose.keyframe_insert('rotation_euler', index=2, frame=frame)
        child_pose.location.x = translation
        child_pose.keyframe_insert('location', index=0, frame=frame)
    data = bpy.data.meshes.new('SeamMeshData')
    data.from_pydata([(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0)], [], [(0, 1, 2), (0, 2, 3)])
    data.update()
    obj = bpy.data.objects.new('SeamMesh', data)
    scene.collection.objects.link(obj)
    obj.parent = rig
    obj.matrix_parent_inverse = Matrix.Identity(4)
    uv = data.uv_layers.new(name='UVMap')
    for loop, value in zip(uv.data, [(0, 0), (1, 0), (1, 1), (0.25, 0), (1, 1), (0, 1)]):
        loop.uv = value
    root_group = obj.vertex_groups.new(name=ROOT_BONE)
    child_group = obj.vertex_groups.new(name=CHILD_BONE)
    root_group.add([0], 1.0, 'REPLACE')
    root_group.add([1], 0.25, 'REPLACE')
    child_group.add([1], 0.75, 'REPLACE')
    child_group.add([2], 1.0, 'REPLACE')
    modifier = obj.modifiers.new('Armature', 'ARMATURE')
    modifier.object = rig
    modifier.use_vertex_groups = True
    modifier.use_bone_envelopes = False
    modifier.use_deform_preserve_volume = False
    obj.shape_key_add(name='Basis')
    key = obj.shape_key_add(name='持上げ')
    key.data[0].co.z += 0.3
    key.data[2].co.z += 0.1
    for frame, value in [(1, 0.0), (3, 0.8)]:
        key.value = value
        key.keyframe_insert('value', frame=frame)
    image = bpy.data.images.new('PackedRed', width=2, height=2, alpha=True)
    image.pixels.foreach_set([1.0, 0.0, 0.0, 1.0] * 4)
    image.file_format = 'PNG'
    image.pack()
    material = bpy.data.materials.new('SimplePrincipled')
    material.use_nodes = True
    texture = material.node_tree.nodes.new('ShaderNodeTexImage')
    texture.image = image
    shader = material.node_tree.nodes.get('Principled BSDF')
    material.node_tree.links.new(texture.outputs['Color'], shader.inputs['Base Color'])
    material.node_tree.links.new(texture.outputs['Alpha'], shader.inputs['Alpha'])
    obj.data.materials.append(material)
    cameras = []
    for name, projection, position in [('CameraA', 'PERSP', (0, -6, 4)), ('CameraB', 'ORTHO', (3, -6, 4))]:
        camera_data = bpy.data.cameras.new(name)
        camera_data.type = projection
        camera_data.lens, camera_data.ortho_scale = 50, 4
        camera = bpy.data.objects.new(name, camera_data)
        scene.collection.objects.link(camera)
        camera.location = position
        camera.rotation_euler = (math.radians(60), 0, 0)
        cameras.append(camera)
    scene.camera = cameras[0]
    scene.timeline_markers.new('Perspective start', frame=1).camera = cameras[0]
    scene.timeline_markers.new('Orthographic cut', frame=2).camera = cameras[1]
    for owner in (mover, turn, rig, obj.data.shape_keys):
        action = owner.animation_data.action
        # Read layered-action channelbags directly instead of relying on the legacy slot API.
        curves = [curve for layer in action.layers for strip in layer.strips
                  for bag in getattr(strip, 'channelbags', ()) for curve in bag.fcurves]
        if not curves:
            curves = list(action.fcurves)
        for curve in curves:
            for point in curve.keyframe_points:
                point.interpolation = 'LINEAR'
    scene.frame_set(1)
    bpy.ops.wm.save_as_mainfile(filepath=str(path), check_existing=True)
    print(json.dumps({'fixture': str(path), 'blender': list(bpy.app.version)}, ensure_ascii=False))


class Reader:
    """Independent reader using the documented primitive layout, without exporter imports."""
    def __init__(self, payload):
        self.payload, self.offset = payload, 0

    def take(self, size):
        end = self.offset + size
        if size < 0 or end > len(self.payload):
            raise ValueError('truncated scene packet')
        result = self.payload[self.offset:end]
        self.offset = end
        return result

    def unpack(self, format_string):
        return struct.unpack('<' + format_string, self.take(struct.calcsize('<' + format_string)))

    def scalar(self, format_string):
        return self.unpack(format_string)[0]

    def name(self):
        size = self.scalar('I')
        assert 0 < size <= 1024 * 1024
        return self.take(size).decode('utf-8')

    def array(self, read, maximum=134217728):
        count = self.scalar('I')
        assert count <= maximum
        return [read() for _ in range(count)]

    def matrix(self):
        values = self.unpack('16d')
        assert all(math.isfinite(value) for value in values)
        return values

    def vertex(self):
        values = self.unpack('8d')
        return dict(position=values[:3], normal=values[3:6], uv=values[6:], weights=self.array(lambda: self.unpack('Id'), 4))

    def material(self):
        name = self.name()
        values = self.unpack('4di')
        return dict(name=name, diffuse=values[:4], image=values[4])

    def morph(self):
        return dict(source_id=self.name(), name=self.name(), offsets=self.array(lambda: self.unpack('I3d'), 4_000_000))

    def mesh(self):
        return dict(source_id=self.name(), name=self.name(), world=self.matrix(), vertices=self.array(self.vertex, 4_000_000),
                    triangles=self.array(lambda: self.unpack('4I')), materials=self.array(self.material), morphs=self.array(self.morph))

    def bone(self):
        return dict(source_id=self.name(), name=self.name(), parent=self.scalar('i'), rest=self.matrix(), head=self.unpack('3d'), tail=self.unpack('3d'))

    def pose(self):
        tick, frame = self.unpack('Id')
        return dict(tick=tick, frame=frame, world=self.matrix(), skin=self.array(self.matrix, 65535), morphs=self.array(lambda: self.unpack('2Id')))

    def group(self):
        return dict(source_id=self.name(), name=self.name(), reference_world=self.matrix(), meshes=self.array(self.mesh),
                    bones=self.array(self.bone, 65535), poses=self.array(self.pose, 108001))

    def image(self):
        source_id, name = self.name(), self.name()
        width, height, size = self.unpack('3I')
        return dict(source_id=source_id, name=name, width=width, height=height, png=self.take(size))

    def camera(self):
        tick, frame = self.unpack('Id')
        source_id, name = self.name(), self.name()
        projection, sensor = self.unpack('2B')
        return dict(tick=tick, frame=frame, source_id=source_id, name=name, projection=projection, sensor=sensor,
                    optics=self.unpack('10d'), world=self.matrix(), projection_matrix=self.matrix())


def multiply(a, b):
    return tuple(sum(a[r * 4 + k] * b[k * 4 + c] for k in range(4)) for r in range(4) for c in range(4))


def translation(x, y=0, z=0):
    return (1, 0, 0, x, 0, 1, 0, y, 0, 0, 1, z, 0, 0, 0, 1)


def rotation(angle):
    c, s = math.cos(angle), math.sin(angle)
    return (c, -s, 0, 0, s, c, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1)


def close(actual, expected, tolerance=1e-6):
    assert len(actual) == len(expected)
    error = max(abs(a - b) for a, b in zip(actual, expected))
    assert error <= tolerance, (error, actual, expected)


def check_fixture(packet_path, source, expected_hash, result_path):
    assert hashlib.sha256(source.read_bytes()).hexdigest() == expected_hash, 'input blend changed'
    payload = packet_path.read_bytes()
    assert len(payload) <= 512 * 1024 * 1024
    r = Reader(payload)
    assert r.take(8) == b'NEXASCN1'
    scene_name = r.name()
    reference, start, end, fps, units, width, height = r.unpack('5d2I')
    groups = r.array(r.group, 256)
    images = r.array(r.image, 4096)
    cameras = r.array(r.camera, 108001)
    assert r.offset == len(payload), 'packet trailing bytes'
    assert (scene_name, reference, start, end, fps, units, width, height) == ('NexaFixtureScene', 1, 1, 3, 30, 1, 32, 24)
    assert len(groups) == 1 and len(images) == 1 and len(cameras) == 3
    group = groups[0]
    assert group['name'] == 'FixtureRig'
    close(group['reference_world'], translation(2))
    assert [bone['name'] for bone in group['bones']] == [ROOT_BONE, CHILD_BONE]
    assert [bone['parent'] for bone in group['bones']] == [-1, 0]
    close(group['bones'][0]['head'], (2, 0, 0))
    close(group['bones'][1]['head'], (2, 1, 0))
    mesh, = group['meshes']
    assert len(mesh['vertices']) == 5, 'UV seam must duplicate exactly one source vertex'
    assert len(mesh['triangles']) == 2 and all(len(set(t[:3])) == 3 and max(t[:3]) < 5 and t[3] == 0 for t in mesh['triangles'])
    seam = [v for v in mesh['vertices'] if v['position'] == (0, 0, 0)]
    assert len(seam) == 2 and {v['uv'] for v in seam} == {(0, 0), (0.25, 0)}
    weighted, = [v for v in mesh['vertices'] if v['position'] == (1, 0, 0)]
    assert weighted['weights'] == [(0, 0.25), (1, 0.75)]
    assert any(not v['weights'] for v in mesh['vertices']), 'unweighted root fallback fixture missing'
    assert all(v['normal'] == (0, 0, 1) for v in mesh['vertices'])
    morph, = mesh['morphs']
    assert morph['name'] == '持上げ' and len(morph['offsets']) == 3
    for index, x, y, z in morph['offsets']:
        close((x, y, z), (0, 0, 0.3 if mesh['vertices'][index]['position'] == (0, 0, 0) else 0.1))
    material, = mesh['materials']
    assert material['image'] == 0 and material['diffuse'] == (1, 1, 1, 1)
    image, = images
    assert (image['width'], image['height']) == (2, 2)
    png = image['png']
    assert png[:8] == b'\x89PNG\r\n\x1a\n'
    cursor, chunks, channels = 8, [], None
    while cursor < len(png):
        size = struct.unpack('>I', png[cursor:cursor + 4])[0]
        kind, data = png[cursor + 4:cursor + 8], png[cursor + 8:cursor + 8 + size]
        crc = struct.unpack('>I', png[cursor + 8 + size:cursor + 12 + size])[0]
        assert zlib.crc32(kind + data) == crc, 'PNG CRC'
        if kind == b'IHDR':
            png_width, png_height, depth, color_type, compression, filtering, interlace = struct.unpack('>IIBBBBB', data)
            assert (png_width, png_height, depth, compression, filtering, interlace) == (2, 2, 8, 0, 0, 0)
            assert color_type in (2, 6)
            channels = 3 if color_type == 2 else 4
        if kind == b'IDAT':
            chunks.append(data)
        cursor += size + 12
    raw = zlib.decompress(b''.join(chunks))
    assert channels is not None and len(raw) == 2 * (1 + 2 * channels)
    previous = [0] * (2 * channels)
    for row_index in range(2):
        begin = row_index * (1 + 2 * channels)
        filter_type, encoded = raw[begin], raw[begin + 1:begin + 1 + 2 * channels]
        assert filter_type <= 4
        row = []
        for index, value in enumerate(encoded):
            left = row[index - channels] if index >= channels else 0
            up = previous[index]
            upper_left = previous[index - channels] if index >= channels else 0
            estimate = left + up - upper_left
            distances = (abs(estimate - left), abs(estimate - up), abs(estimate - upper_left))
            paeth = (left, up, upper_left)[distances.index(min(distances))]
            predictor = (0, left, up, (left + up) // 2, paeth)[filter_type]
            row.append((value + predictor) & 255)
        assert row == ([255, 0, 0, 255] if channels == 4 else [255, 0, 0]) * 2, 'PNG pixel colors differ'
        previous = row
    assert len(group['poses']) == 3
    for tick, pose in enumerate(group['poses']):
        ratio = tick / 2
        assert (pose['tick'], pose['frame']) == (tick, tick + 1)
        world = multiply(translation(2 + 0.4 * ratio), rotation(0.1 * ratio))
        close(pose['world'], world)
        expected_parent = multiply(multiply(world, rotation(0.2 * ratio)), translation(-2))
        expected_child = multiply(multiply(multiply(world, rotation(0.2 * ratio)), translation(0.2 * ratio)), translation(-2))
        close(pose['skin'][0], expected_parent)
        close(pose['skin'][1], expected_child)
        assert len(pose['morphs']) == 1 and pose['morphs'][0][:2] == (0, 0)
        close(pose['morphs'][0][2:], (0.8 * ratio,))
    assert [(c['tick'], c['frame'], c['name'], c['projection']) for c in cameras] == [(0, 1, 'CameraA', 0), (1, 2, 'CameraB', 1), (2, 3, 'CameraB', 1)]
    for camera in cameras:
        close((camera['world'][3], camera['world'][7], camera['world'][11]), (0 if camera['name'] == 'CameraA' else 3, -6, 4))
    report = dict(status='passed', input_sha256=expected_hash, input_unchanged=True, packet_bytes=len(payload),
                  groups=1, bones=2, seam_split_vertices=5, triangles=2, morph_offsets=3, images=1, poses=3,
                  cameras=3, camera_cut=True, analytic_pose_tolerance=1e-6,
                  limitations=['Packet extraction only; PMX/VMD writing and MMD runtime are separate gates.'])
    with result_path.open('x', encoding='utf-8') as stream:
        json.dump(report, stream, indent=2)
        stream.write('\n')
    print(json.dumps(report))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--mode', choices=['create', 'check'], required=True)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--packet', type=Path)
    parser.add_argument('--source', type=Path)
    parser.add_argument('--source-sha256')
    parser.add_argument('--result', type=Path)
    args = parser.parse_args(sys.argv[sys.argv.index('--') + 1:] if '--' in sys.argv else None)
    if args.mode == 'create':
        create_fixture(args.output.absolute())
    else:
        check_fixture(args.packet, args.source, args.source_sha256, args.result)


if __name__ == '__main__':
    main()
