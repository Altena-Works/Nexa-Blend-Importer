"""Create/check the bounded B-Bone, animated-modifier and image-alpha guard fixtures."""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import sys
import zlib

sys.dont_write_bytecode = True


def create(path):
    import bpy

    if path.exists():
        raise FileExistsError(path)
    bpy.ops.wm.read_factory_settings(use_empty=True)

    def configure(scene):
        scene.frame_start, scene.frame_end = 1, 3
        scene.render.fps, scene.render.fps_base = 30, 1.0
        scene.render.resolution_x, scene.render.resolution_y = 32, 24
        scene.render.resolution_percentage = 100
        scene.unit_settings.scale_length = 1.0

    def triangle(scene, name):
        data = bpy.data.meshes.new(name + 'Data')
        data.from_pydata([(0, 0, 0), (0.5, 0.5, 0), (0, 1, 0)], [], [(0, 1, 2)])
        data.update()
        obj = bpy.data.objects.new(name, data)
        scene.collection.objects.link(obj)
        return obj

    scene = bpy.context.scene
    scene.name = 'BboneScene'
    configure(scene)
    armature = bpy.data.armatures.new('BboneArmature')
    rig = bpy.data.objects.new('BboneRig', armature)
    scene.collection.objects.link(rig)
    bpy.context.view_layer.objects.active = rig
    rig.select_set(True)
    bpy.ops.object.mode_set(mode='EDIT')
    bone = armature.edit_bones.new('BendyBone')
    bone.head, bone.tail = (0, 0, 0), (0, 1, 0)
    bpy.ops.object.mode_set(mode='OBJECT')
    armature.bones['BendyBone'].bbone_segments = 3
    rig.pose.bones['BendyBone'].bbone_curveinx = 0.2
    obj = triangle(scene, 'BboneMesh')
    weights = obj.vertex_groups.new(name='BendyBone')
    weights.add([0, 1, 2], 1.0, 'REPLACE')
    modifier = obj.modifiers.new('Armature', 'ARMATURE')
    modifier.object = rig
    modifier.use_bone_envelopes = False
    modifier.use_deform_preserve_volume = False

    scene = bpy.data.scenes.new('ModifierScene')
    bpy.context.window.scene = scene
    configure(scene)
    obj = triangle(scene, 'ModifierMesh')
    modifier = obj.modifiers.new('SubdivLater', 'SUBSURF')
    modifier.levels = modifier.render_levels = 1
    modifier.show_render = False
    modifier.keyframe_insert(data_path='show_render', frame=1)
    modifier.show_render = True
    modifier.keyframe_insert(data_path='show_render', frame=2)
    scene.frame_set(1)

    scene = bpy.data.scenes.new('AlphaScene')
    bpy.context.window.scene = scene
    configure(scene)
    data = bpy.data.meshes.new('AlphaMeshData')
    data.from_pydata([(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0)], [], [(0, 1, 2), (0, 2, 3)])
    data.update()
    obj = bpy.data.objects.new('AlphaMesh', data)
    scene.collection.objects.link(obj)
    uv = data.uv_layers.new(name='UVMap')
    for loop, coordinate in zip(uv.data, [(0, 0), (1, 0), (1, 1), (0, 0), (1, 1), (0, 1)]):
        loop.uv = coordinate
    image = bpy.data.images.new('SharedRGBA', width=2, height=1, alpha=True)
    image.pixels.foreach_set([1, 0, 0, 0.25, 0, 1, 0, 0.75])
    image.file_format = 'PNG'
    image.pack()
    for name, linked in [('AlphaConnected', True), ('AlphaUnconnected', False)]:
        material = bpy.data.materials.new(name)
        material.use_nodes = True
        shader = material.node_tree.nodes.get('Principled BSDF')
        shader.inputs['Alpha'].default_value = 0.6
        texture = material.node_tree.nodes.new('ShaderNodeTexImage')
        texture.image = image
        material.node_tree.links.new(texture.outputs['Color'], shader.inputs['Base Color'])
        if linked:
            material.node_tree.links.new(texture.outputs['Alpha'], shader.inputs['Alpha'])
        data.materials.append(material)
    data.polygons[0].material_index, data.polygons[1].material_index = 0, 1
    scene.frame_set(1)
    bpy.ops.wm.save_as_mainfile(filepath=str(path), check_existing=True)
    print(json.dumps({'fixture': str(path), 'blender': list(bpy.app.version), 'scenes': ['BboneScene', 'ModifierScene', 'AlphaScene']}))


def png_pixels(png):
    """Independent full-image fixture decoder; fixture images contain only two pixels."""
    assert png[:8] == b'\x89PNG\r\n\x1a\n'
    cursor, compressed, metadata = 8, bytearray(), None
    while cursor < len(png):
        size = struct.unpack_from('>I', png, cursor)[0]
        kind = png[cursor + 4:cursor + 8]
        data = png[cursor + 8:cursor + 8 + size]
        crc = struct.unpack_from('>I', png, cursor + size + 8)[0]
        assert zlib.crc32(kind + data) == crc
        if kind == b'IHDR':
            metadata = struct.unpack('>IIBBBBB', data)
        elif kind == b'IDAT':
            compressed.extend(data)
        cursor += size + 12
    assert cursor == len(png)
    assert metadata == (2, 1, 8, 6, 0, 0, 0), metadata
    raw = zlib.decompress(compressed)
    assert len(raw) == 9 and raw[0] <= 4
    row = []
    for index, value in enumerate(raw[1:]):
        left = row[index - 4] if index >= 4 else 0
        # There is one row, so Up/UpperLeft are zero and Paeth chooses Left.
        row.append((value + (0, left, 0, left // 2, left)[raw[0]]) & 255)
    return [tuple(row[index:index + 4]) for index in range(0, len(row), 4)]


def check(root, expected_hash):
    from blender_scene_fixture import Reader

    source = root / 'fixture-01' / 'guards.blend'
    assert hashlib.sha256(source.read_bytes()).hexdigest() == expected_hash, 'input blend changed'
    for folder, expected in [
            ('bbone-01', ('BboneMesh/BendyBone', '3 segments', 'unsupported')),
            ('modifier-model-01', ('ModifierMesh at frame 2', 'SubdivLater', 'correspondence')),
            ('modifier-animation-01', ('ModifierMesh at frame 2', 'SubdivLater', 'correspondence'))]:
        directory = root / folder
        diagnostic = (directory / 'error.txt').read_text(encoding='utf-8')
        assert all(value in diagnostic for value in expected), diagnostic
        assert not (directory / 'scene.nexascn').exists(), 'failed request published a packet'
        assert not list(directory.glob('.nexa-scene-*')), 'owned scratch was not cleaned'
    directory = root / 'alpha-01'
    assert not (directory / 'error.txt').exists()
    assert not list(directory.glob('.nexa-scene-*'))
    payload = (directory / 'scene.nexascn').read_bytes()
    r = Reader(payload)
    assert r.take(8) == b'NEXASCN1'
    assert r.name() == 'AlphaScene'
    r.unpack('5d2I')
    groups, images, cameras = r.array(r.group, 256), r.array(r.image, 4096), r.array(r.camera, 108001)
    assert r.offset == len(payload)
    assert len(groups) == 1 and len(images) == 2 and not cameras
    mesh, = groups[0]['meshes']
    linked, unlinked = mesh['materials']
    assert linked['name'] == 'AlphaConnected' and unlinked['name'] == 'AlphaUnconnected'
    assert linked['image'] != unlinked['image']
    transparent, opaque = images[linked['image']], images[unlinked['image']]
    assert transparent['source_id'].endswith(':alpha') and opaque['source_id'].endswith(':opaque')
    assert transparent['name'] == opaque['name'] == 'SharedRGBA'
    alpha_pixels, opaque_pixels = png_pixels(transparent['png']), png_pixels(opaque['png'])
    assert alpha_pixels == [(255, 0, 0, 64), (0, 255, 0, 191)], alpha_pixels
    assert opaque_pixels == [(255, 0, 0, 255), (0, 255, 0, 255)], opaque_pixels
    assert [pixel[:3] for pixel in alpha_pixels] == [pixel[:3] for pixel in opaque_pixels]
    assert linked['diffuse'][3] == 1 and abs(unlinked['diffuse'][3] - 0.6) < 1e-6
    report = dict(status='passed', input_sha256=expected_hash, input_unchanged=True,
                  bbone_static_rejected=True, modifier_model_animation_rejected=True,
                  modifier_animation_only_rejected=True, alpha_variants=2,
                  connected_rgba=alpha_pixels, unconnected_rgba=opaque_pixels, rgb_bytes_unchanged=True,
                  export_calls=4, runs_per_branch=1, packet_bytes=len(payload))
    with (root / 'result.json').open('x', encoding='utf-8') as stream:
        json.dump(report, stream, indent=2)
        stream.write('\n')
    print(json.dumps(report))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--mode', choices=['create', 'check'], required=True)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--root', type=Path)
    parser.add_argument('--source-sha256')
    args = parser.parse_args(sys.argv[sys.argv.index('--') + 1:] if '--' in sys.argv else None)
    if args.mode == 'create':
        create(args.output.absolute())
    else:
        check(args.root, args.source_sha256)


if __name__ == '__main__':
    main()
