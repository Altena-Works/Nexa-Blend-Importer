"""Export selected evaluated Blender components as one bounded NEXASCN1 packet."""
import argparse
from fractions import Fraction
import math
import os
from pathlib import Path
import stat
import struct
import sys
import tempfile
import zlib

import bpy

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from blender_camera_export import matrix_values, number, reject_python_drivers


MAX_BYTES = 512 * 1024 * 1024
MAX_GROUPS = 256
MAX_VERTICES = 4_000_000
MAX_BONES = 65_535
MAX_IMAGES = 4096
MAX_EDGE = 16_384
MAX_TICKS = 108_001


def fail(target, reason):
    raise ValueError(f'{target}: {reason}')


def text(value):
    if not value or '\0' in value:
        fail('name/id', 'must be nonempty and contain no NUL')
    encoded = value.encode('utf-8', errors='strict')
    if len(encoded) > 1024 * 1024:
        fail('name/id', 'exceeds 1 MiB')
    return encoded


class Budget:
    def __init__(self):
        self.bytes = 0

    def add(self, size):
        if not 0 <= size <= MAX_BYTES - self.bytes:
            fail('scene packet', f'exceeds {MAX_BYTES} bytes')
        self.bytes += size

    def names(self, *names):
        for name in names:
            self.add(4 + len(text(name)))


def source_id(owner, kind):
    library = owner.library.name_full if owner.library else 'local'
    return f'{kind}:{library}:{owner.name_full}'


def local_path(value, *, library=None, existing=False):
    value = os.fspath(value)
    # Rust canonical paths use the local extended-length disk prefix on Windows.
    # Accept that disk form, while still rejecting UNC, device and volume paths.
    if value.startswith('\\\\?\\'):
        if len(value) < 7 or value[4].upper() not in 'ABCDEFGHIJKLMNOPQRSTUVWXYZ' or value[5:7] != ':\\':
            fail('file reference', 'only local extended-length disk paths are supported')
        value = value[4:]
    if value.startswith('\\\\') or '://' in value:
        fail('file reference', 'URL/UNC references are unsupported')
    path = Path(bpy.path.abspath(value, library=library)).absolute()
    if str(path).startswith(('\\\\', '//')):
        fail('file reference', 'network paths are unsupported')
    for ancestor in [path, *path.parents]:
        try:
            info = ancestor.lstat()
        except FileNotFoundError:
            continue
        if stat.S_ISLNK(info.st_mode) or getattr(info, 'st_file_attributes', 0) & 0x400:
            fail('file reference', f'reparse/symlink entry is unsupported: {ancestor}')
    if existing and not path.is_file():
        fail(path, 'missing local file')
    return path.resolve(strict=existing)


def check_library(owner):
    library = getattr(owner, 'library', None)
    seen = set()
    while library is not None and library not in seen:
        seen.add(library)
        local_path(library.filepath, library=library.parent, existing=True)
        if library.is_missing:
            fail(owner.name_full, 'missing linked library')
        library = library.parent


def matrix(matrix, label):
    values = matrix_values(matrix, label)
    if any(abs(values[i]) > 1e-10 for i in (12, 13, 14)) or abs(values[15] - 1) > 1e-10:
        fail(label, 'expected an affine matrix')
    if abs(matrix.to_3x3().determinant()) < 1e-12:
        fail(label, 'singular transform')
    return values


def rigid(matrix_value, label):
    matrix(matrix_value, label)
    basis = matrix_value.to_3x3()
    residual = basis.transposed() @ basis
    error = max(abs(residual[row][col] - (row == col)) for row in range(3) for col in range(3))
    if error > 1e-5 or abs(basis.determinant() - 1) > 1e-5:
        fail(label, 'remaining animated scale/shear/reflection cannot be represented by VMD')


def frame_set(scene, frame):
    frame = number(frame, 'frame')
    integer = math.floor(frame)
    if not -1048574 <= integer <= 1048574:
        fail('frame', 'outside Blender frame limits')
    scene.frame_set(integer, subframe=frame - integer)


def render_meshes(scene):
    visible = set()

    def walk(collection, enabled):
        enabled = enabled and not collection.hide_render
        if enabled:
            for obj in collection.objects:
                if not obj.hide_render and obj.instance_type != 'NONE':
                    fail(obj.name_full, 'rendered object/collection instancing is unsupported')
                if obj.type == 'MESH' and not obj.hide_render:
                    visible.add(obj)
        for child in collection.children:
            walk(child, enabled)

    walk(scene.collection, True)
    return sorted(visible, key=lambda obj: (obj.name_full, source_id(obj, 'mesh')))


def validate_mesh(mesh, scene, frame, graph):
    target = f'{mesh.name_full} at frame {frame:.17g}'
    if mesh.instance_type != 'NONE':
        fail(target, 'object instancing is unsupported')
    evaluated = mesh.evaluated_get(graph)
    modifiers = [modifier for modifier in evaluated.modifiers if modifier.show_render]
    unknown = next((modifier for modifier in modifiers if modifier.type != 'ARMATURE'), None)
    if unknown is not None:
        fail(target, f'modifier {unknown.name!r} cannot preserve proven rest/weight/morph correspondence')
    if len(modifiers) > 1:
        fail(target, 'multiple armature deformation modifiers are unsupported')
    if not modifiers:
        return mesh, ()
    modifier = modifiers[0]
    root = modifier.object.original if modifier.object is not None else None
    if root is None or root.type != 'ARMATURE' or root.name not in scene.objects:
        fail(target, 'armature target is missing from the selected scene')
    if modifier.use_deform_preserve_volume or modifier.use_bone_envelopes or not modifier.use_vertex_groups or modifier.vertex_group:
        fail(target, 'dual-quaternion/envelope/masked armature deformation is unsupported')
    return root, (modifier.name, source_id(root, 'group'))


def group_sources(scene, selection, graph, frame):
    groups = {}
    animation_only = selection == {'animation'}
    if animation_only:
        for obj in scene.objects:
            if obj.type == 'ARMATURE':
                groups[source_id(obj, 'group')] = [obj, []]
    for mesh in render_meshes(scene):
        root, _ = validate_mesh(mesh, scene, frame, graph)
        key = source_id(root, 'group')
        groups.setdefault(key, [root, []])[1].append(mesh)
    values = sorted(groups.values(), key=lambda pair: (pair[0].name_full, source_id(pair[0], 'group')))
    if len(values) > MAX_GROUPS:
        fail('groups', f'exceeds {MAX_GROUPS}')
    return values


def sorted_bones(armature):
    if armature.type != 'ARMATURE':
        return []
    bones = list(armature.data.bones)
    if len(bones) > MAX_BONES:
        fail(armature.name_full, f'exceeds {MAX_BONES} bones')
    # Parent depth gives a deterministic parent-before-child order, including control bones.
    return sorted(bones, key=lambda bone: (len(bone.parent_recursive), bone.name))


def animated(owner):
    animation = getattr(owner, 'animation_data', None)
    return animation is not None and (animation.action is not None or len(animation.drivers) or len(animation.nla_tracks))


def png_chunks(png):
    if png[:8] != b'\x89PNG\r\n\x1a\n':
        fail('opaque PNG', 'invalid PNG signature')
    offset = 8
    view = memoryview(png)
    while offset < len(png):
        if len(png) - offset < 12:
            fail('opaque PNG', 'truncated chunk')
        size = struct.unpack_from('>I', png, offset)[0]
        end = offset + size + 12
        if end > len(png):
            fail('opaque PNG', 'truncated chunk data')
        kind, data = png[offset + 4:offset + 8], view[offset + 8:end - 4]
        crc = struct.unpack_from('>I', png, end - 4)[0]
        if zlib.crc32(data, zlib.crc32(kind)) != crc:
            fail('opaque PNG', 'chunk CRC mismatch')
        yield kind, data
        offset = end


def opaque_png(png, maximum):
    """Change only alpha samples of Blender's PNG, using bounded scanline buffers."""
    chunks = png_chunks(png)
    kind, header = next(chunks)
    if kind != b'IHDR' or len(header) != 13:
        fail('opaque PNG', 'missing IHDR')
    width, height, depth, color, compression, filtering, interlace = struct.unpack('>IIBBBBB', header)
    channels = {0: 1, 2: 3, 4: 2, 6: 4}.get(color)
    if not (1 <= width <= MAX_EDGE and 1 <= height <= MAX_EDGE) or depth not in {8, 16} or channels is None:
        fail('opaque PNG', 'only 8/16-bit grayscale/RGB with optional alpha is supported')
    if (compression, filtering, interlace) != (0, 0, 0):
        fail('opaque PNG', 'unsupported compression/filter/interlace mode')
    sample_bytes, pixel_bytes = depth // 8, channels * (depth // 8)
    row_bytes = width * pixel_bytes
    output = bytearray(b'\x89PNG\r\n\x1a\n')

    def put(chunk_kind, data):
        if len(output) + 12 + len(data) > maximum:
            fail('opaque PNG', 'variant exceeds remaining scene packet budget')
        output.extend(struct.pack('>I', len(data)))
        output.extend(chunk_kind)
        output.extend(data)
        output.extend(struct.pack('>I', zlib.crc32(data, zlib.crc32(chunk_kind))))

    put(b'IHDR', header)
    before_idat = True
    saw_idat = saw_end = False
    for kind, data in chunks:
        if kind == b'IDAT':
            if saw_end:
                fail('opaque PNG', 'IDAT after IEND')
            before_idat, saw_idat = False, True
        elif kind == b'IEND':
            if len(data):
                fail('opaque PNG', 'invalid IEND')
            saw_end = True
        elif kind == b'IHDR' or (kind[0] & 32 == 0 and kind != b'PLTE'):
            fail('opaque PNG', 'unknown or repeated critical chunk')
        elif before_idat and kind != b'tRNS':
            put(kind, data)
    if not saw_idat or not saw_end:
        fail('opaque PNG', 'missing IDAT/IEND')
    inflater, deflater = zlib.decompressobj(), zlib.compressobj()
    previous = bytearray(row_bytes)
    encoded = bytearray()
    rows = 0

    def consume(data):
        nonlocal previous, rows
        encoded.extend(data)
        if len(encoded) != row_bytes + 1:
            return
        if rows >= height or encoded[0] > 4:
            fail('opaque PNG', 'unexpected scanline/filter')
        mode = encoded[0]
        row = bytearray(row_bytes)
        for index in range(row_bytes):
            left = row[index - pixel_bytes] if index >= pixel_bytes else 0
            up = previous[index]
            upper_left = previous[index - pixel_bytes] if index >= pixel_bytes else 0
            estimate = left + up - upper_left
            distance = (abs(estimate - left), abs(estimate - up), abs(estimate - upper_left))
            paeth = (left, up, upper_left)[distance.index(min(distance))]
            predictor = (0, left, up, (left + up) // 2, paeth)[mode]
            row[index] = (encoded[index + 1] + predictor) & 255
        # PNG filters refer to original bytes, so retain this row before changing alpha.
        previous = row.copy()
        if color in {4, 6}:
            alpha_offset = (channels - 1) * sample_bytes
            for pixel in range(width):
                begin = pixel * pixel_bytes + alpha_offset
                row[begin:begin + sample_bytes] = b'\xff' * sample_bytes
        compressed = deflater.compress(b'\0' + row)
        if compressed:
            put(b'IDAT', compressed)
        rows += 1
        encoded.clear()

    for kind, data in png_chunks(png):
        if kind != b'IDAT':
            continue
        for offset in range(0, len(data), 65536):
            pending = data[offset:offset + 65536]
            while pending:
                consume(inflater.decompress(pending, row_bytes + 1 - len(encoded)))
                pending = inflater.unconsumed_tail
                if inflater.unused_data:
                    fail('opaque PNG', 'trailing compressed data')
    while True:
        remaining = inflater.decompress(b'', row_bytes + 1 - len(encoded))
        if not remaining:
            break
        consume(remaining)
    if not inflater.eof or encoded or rows != height:
        fail('opaque PNG', 'incomplete or excess pixel data')
    compressed = deflater.flush()
    if compressed:
        put(b'IDAT', compressed)
    after_idat = False
    for kind, data in png_chunks(png):
        if kind == b'IDAT':
            after_idat = True
        elif after_idat and kind not in {b'IEND', b'tRNS'}:
            put(kind, data)
    put(b'IEND', b'')
    return bytes(output)


class Images:
    def __init__(self, directory, budget):
        self.directory = directory
        self.budget = budget
        self.images = []
        self.indices = {}
        self.owned = []

    def get(self, image, use_alpha):
        identity = source_id(image, 'image') + (':alpha' if use_alpha else ':opaque')
        if identity in self.indices:
            return self.indices[identity]
        if len(self.images) >= MAX_IMAGES:
            fail('images', f'exceeds {MAX_IMAGES}')
        check_library(image)
        if image.source not in {'FILE', 'GENERATED'} or image.type != 'IMAGE':
            fail(image.name_full, f'unsupported image source/type {image.source}/{image.type}')
        if image.source == 'FILE' and image.packed_file is None:
            local_path(image.filepath, library=image.library, existing=True)
        width, height = tuple(image.size)
        if not image.has_data or not (1 <= width <= MAX_EDGE and 1 <= height <= MAX_EDGE):
            fail(image.name_full, f'missing image data or dimensions exceed {MAX_EDGE}')
        index = len(self.images)
        path = self.directory / f'image_{index + 1:04}.png'
        # Blender overwrites only the empty file exclusively created by this exporter.
        with path.open('xb'):
            pass
        self.owned.append(path)
        old_path, old_format = image.filepath_raw, image.file_format
        try:
            image.filepath_raw = str(path)
            image.file_format = 'PNG'
            image.save()
        finally:
            image.filepath_raw, image.file_format = old_path, old_format
        size = path.stat().st_size
        self.budget.names(identity, image.name_full)
        self.budget.add(12)
        if size > MAX_BYTES:
            fail(image.name_full, 'PNG exceeds scene packet byte limit')
        with path.open('rb') as stream:
            png = stream.read(MAX_BYTES + 1)
        if len(png) != size or len(png) < 33 or png[:8] != b'\x89PNG\r\n\x1a\n' or png[16:24] != struct.pack('>II', width, height):
            fail(image.name_full, 'PNG export framing/dimensions mismatch')
        if not use_alpha:
            png = opaque_png(png, MAX_BYTES - self.budget.bytes)
        self.budget.add(len(png))
        self.indices[identity] = index
        self.images.append(dict(source_id=identity, name=image.name_full, width=width, height=height, png=png))
        return index


def material_packet(material, images, animation):
    if material is None:
        return dict(name='Default diffuse', diffuse=[0.8, 0.8, 0.8, 1.0], image=None)
    check_library(material)
    if animation and (animated(material) or animated(material.node_tree)):
        fail(material.name_full, 'animated materials are unsupported')
    if not material.use_nodes or material.node_tree is None:
        fail(material.name_full, 'only a simple Principled BSDF material is supported')
    outputs = [node for node in material.node_tree.nodes if node.type == 'OUTPUT_MATERIAL' and node.is_active_output]
    if len(outputs) != 1 or len(outputs[0].inputs['Surface'].links) != 1:
        fail(material.name_full, 'expected one active directly connected surface')
    surface_link = outputs[0].inputs['Surface'].links[0]
    shader = surface_link.from_node
    if shader.type != 'BSDF_PRINCIPLED' or shader.mute or surface_link.from_socket.name != 'BSDF':
        fail(material.name_full, 'surface graph is not a direct Principled BSDF')
    if outputs[0].inputs['Volume'].is_linked or outputs[0].inputs['Displacement'].is_linked:
        fail(material.name_full, 'volume/displacement graphs are unsupported')
    if any(socket.is_linked for socket in shader.inputs if socket.name not in {'Base Color', 'Alpha'}):
        fail(material.name_full, 'only Base Color/Alpha links are supported')
    base, alpha = shader.inputs['Base Color'], shader.inputs['Alpha']
    image_index = None
    diffuse = list(base.default_value[:3]) + [alpha.default_value]
    if base.is_linked:
        link = base.links[0]
        texture = link.from_node
        if len(base.links) != 1 or texture.type != 'TEX_IMAGE' or texture.mute or link.from_socket.name != 'Color' or texture.image is None:
            fail(material.name_full, 'Base Color must connect directly to one image Color')
        if texture.projection != 'FLAT' or texture.extension != 'REPEAT':
            fail(material.name_full, 'non-flat/non-repeating image mapping is unsupported')
        if texture.inputs['Vector'].is_linked:
            uv = texture.inputs['Vector'].links[0]
            if uv.from_node.type != 'UVMAP':
                fail(material.name_full, 'image mapping must use the active UV map')
            uv_name = uv.from_node.uv_map
        else:
            uv_name = None
        diffuse[:3] = [1.0, 1.0, 1.0]
        if alpha.is_linked:
            link = alpha.links[0]
            if len(alpha.links) != 1 or link.from_node != texture or link.from_socket.name != 'Alpha':
                fail(material.name_full, 'Alpha must use the same base-color image Alpha')
            diffuse[3] = 1.0
        image_index = images.get(texture.image, use_alpha=alpha.is_linked)
    elif alpha.is_linked:
        fail(material.name_full, 'image Alpha without the matching base-color image is unsupported')
    for value in diffuse:
        if not math.isfinite(value) or not 0 <= value <= 1:
            fail(material.name_full, 'diffuse color/alpha must be finite within [0, 1]')
    return dict(name=material.name_full, diffuse=diffuse, image=image_index,
                _uv_name=uv_name if image_index is not None else None)


def mesh_packet(obj, bone_indices, images, budget, remaining_vertices, animation, include_model):
    check_library(obj)
    check_library(obj.data)
    if len(obj.data.vertices) > remaining_vertices:
        fail(obj.name_full, f'group exceeds {MAX_VERTICES} vertices')
    if animated(obj.data):
        fail(obj.name_full, 'animated mesh data outside shape keys is unsupported')
    mesh = obj.data
    triangle_count = sum(max(0, polygon.loop_total - 2) for polygon in mesh.polygons)
    if triangle_count * 16 > MAX_BYTES - budget.bytes:
        fail(obj.name_full, 'triangle indices exceed the packet byte budget before triangulation')
    mesh.calc_loop_triangles()
    identity = source_id(obj, 'mesh')
    budget.names(identity, obj.name_full)
    budget.add(144)
    uv_layer = next((uv for uv in mesh.uv_layers if uv.active_render), mesh.uv_layers.active)
    materials = ([material_packet(slot.material, images, animation) for slot in obj.material_slots]
                 if include_model else [material_packet(None, images, False)] * max(1, len(obj.material_slots)))
    if not materials:
        materials = [material_packet(None, images, animation)]
    for material in materials:
        budget.names(material['name'])
        budget.add(36)
        if material['image'] is not None and (uv_layer is None or material['_uv_name'] not in {None, '', uv_layer.name}):
            fail(obj.name_full, 'base-color image requires the active render UV layer')
    weights, weighted_bones = [], set()
    for vertex in mesh.vertices:
        influences = []
        for assignment in vertex.groups:
            group_name = obj.vertex_groups[assignment.group].name
            if group_name in bone_indices:
                value = number(assignment.weight, f'{obj.name_full} weight')
                if value < 0:
                    fail(obj.name_full, 'negative bone weight')
                if value > 0:
                    bone_index, segments = bone_indices[group_name]
                    if segments > 1:
                        fail(f'{obj.name_full}/{group_name}', f'weighted deform B-Bone has {segments} segments; segment deformation is unsupported')
                    influences.append((bone_index, value))
                    weighted_bones.add(group_name)
        if len(influences) > 4:
            fail(obj.name_full, 'more than four positive deform-bone weights')
        weights.append(sorted(influences))
    vertices, triangles, original_indices, lookup = [], [], [], {}
    for triangle in mesh.loop_triangles:
        if triangle.material_index >= len(materials):
            fail(obj.name_full, 'triangle material index out of range')
        indices = []
        for loop_index in triangle.loops:
            original = mesh.loops[loop_index].vertex_index
            normal = tuple(number(v, 'normal') for v in mesh.corner_normals[loop_index].vector)
            uv = tuple(number(v, 'UV') for v in uv_layer.data[loop_index].uv) if uv_layer else (0.0, 0.0)
            key = (original, normal, uv)
            if key not in lookup:
                if len(vertices) >= remaining_vertices:
                    fail(obj.name_full, f'group exceeds {MAX_VERTICES} seam-split vertices')
                budget.add(68 + 12 * len(weights[original]))
                lookup[key] = len(vertices)
                vertices.append(dict(position=tuple(mesh.vertices[original].co), normal=normal, uv=uv, weights=weights[original]))
                original_indices.append(original)
            indices.append(lookup[key])
        budget.add(16)
        triangles.append((indices, triangle.material_index))
    if not triangles:
        fail(obj.name_full, 'mesh has no renderable triangles')
    morphs, key_names = [], []
    keys = mesh.shape_keys
    if keys is not None:
        if obj.show_only_shape_key:
            fail(obj.name_full, 'isolated shape-key display is unsupported')
        if not keys.use_relative:
            fail(obj.name_full, 'absolute shape keys are unsupported')
        basis = keys.reference_key
        for key in keys.key_blocks:
            if key == basis:
                continue
            if key.relative_key != basis or key.vertex_group or len(key.data) != len(mesh.vertices):
                fail(obj.name_full, f'shape key {key.name!r} lacks direct Basis correspondence')
            key_id = f'{identity}:shape:{key.name}'
            budget.names(key_id, key.name)
            budget.add(4)
            offsets = []
            for split_index, original in enumerate(original_indices):
                delta = tuple(key.data[original].co - basis.data[original].co)
                if any(value != 0 for value in delta):
                    budget.add(28)
                    offsets.append((split_index, delta))
            morphs.append(dict(source_id=key_id, name=key.name, offsets=offsets))
            key_names.append(key.name)
    return dict(source_id=identity, name=obj.name_full, world_from_mesh=matrix(obj.matrix_world, obj.name_full),
                vertices=vertices, triangles=triangles, materials=materials, morphs=morphs,
                _source=obj, _key_names=key_names, _weighted_bones=weighted_bones)


def extract_group(root, objects, graph, images, budget, animation, include_model, scene, reference_frame):
    check_library(root)
    check_library(root.data)
    evaluated = root.evaluated_get(graph)
    if not evaluated.is_evaluated:
        fail(root.name_full, 'not evaluated in the selected view layer')
    reference = evaluated.matrix_world.copy()
    reference_inverse = reference.inverted()
    group_id = source_id(root, 'group')
    budget.names(group_id, root.name_full)
    budget.add(140)
    bones, bone_indices, reference_rests = [], {}, []
    source_bones = sorted_bones(root)
    all_indices = {bone.name: index for index, bone in enumerate(source_bones)}
    for bone in source_bones:
        rest = reference @ bone.matrix_local
        rest_id = f'{group_id}:bone:{bone.name}'
        budget.names(rest_id, bone.name)
        budget.add(180)
        bones.append(dict(source_id=rest_id, name=bone.name, parent=all_indices[bone.parent.name] if bone.parent else None,
                          rest_world=matrix(rest, bone.name), head_world=tuple(reference @ bone.head_local), tail_world=tuple(reference @ bone.tail_local)))
        reference_rests.append(rest.inverted())
        if bone.use_deform:
            bone_indices[bone.name] = (all_indices[bone.name], bone.bbone_segments)
    meshes = []
    remaining = MAX_VERTICES
    for obj in sorted(objects, key=lambda item: (item.name_full, source_id(item, 'mesh'))):
        evaluated_mesh = obj.evaluated_get(graph)
        if not evaluated_mesh.is_evaluated:
            fail(obj.name_full, 'not evaluated in the selected view layer')
        # Only an armature modifier is accepted; source datablock geometry is the rest/Basis
        # geometry, so neither evaluated shape deformation nor armature deformation is baked twice.
        mesh = mesh_packet(obj, bone_indices, images, budget, remaining, animation, include_model)
        mesh['world_from_mesh'] = matrix(evaluated_mesh.matrix_world, obj.name_full)
        mesh['_reference_relative'] = reference_inverse @ evaluated_mesh.matrix_world
        _, mesh['_modifier_signature'] = validate_mesh(obj, scene, reference_frame, graph)
        remaining -= len(mesh['vertices'])
        meshes.append(mesh)
    return dict(source_id=group_id, name=root.name_full, reference_object_world=matrix(reference, root.name_full),
                meshes=meshes, bones=bones, poses=[], _source=root,
                _reference=reference, _reference_inverse=reference_inverse, _inverse_rests=reference_rests)


def camera_sample(scene, graph, tick, frame, budget):
    camera = scene.camera
    if camera is None or camera.type != 'CAMERA' or camera.name not in scene.objects:
        fail('camera', 'active scene camera is missing')
    check_library(camera)
    check_library(camera.data)
    evaluated = camera.evaluated_get(graph)
    if not evaluated.is_evaluated:
        fail(camera.name_full, 'not evaluated in selected view layer')
    data = evaluated.data
    if data.type not in {'PERSP', 'ORTHO'} or data.sensor_fit not in {'AUTO', 'HORIZONTAL', 'VERTICAL'}:
        fail(camera.name_full, 'unsupported camera projection/sensor fit')
    render = scene.evaluated_get(graph).render
    width = max(1, render.resolution_x * render.resolution_percentage // 100)
    height = max(1, render.resolution_y * render.resolution_percentage // 100)
    if not (1 <= width <= MAX_EDGE and 1 <= height <= MAX_EDGE):
        fail('render size', f'exceeds {MAX_EDGE}')
    identity = source_id(camera, 'camera')
    budget.names(identity, camera.name_full)
    budget.add(350)
    positive = [data.lens, data.sensor_width, data.sensor_height, data.ortho_scale,
                data.clip_start, data.clip_end, render.pixel_aspect_x, render.pixel_aspect_y]
    for value in positive:
        number(value, camera.name_full, positive=True)
    if data.clip_start >= data.clip_end:
        fail(camera.name_full, 'invalid clipping range')
    return dict(tick=tick, source_frame=frame, source_id=identity, name=camera.name_full,
                projection={'PERSP': 0, 'ORTHO': 1}[data.type], sensor_fit={'AUTO': 0, 'HORIZONTAL': 1, 'VERTICAL': 2}[data.sensor_fit],
                lens_mm=data.lens, sensor_mm=(data.sensor_width, data.sensor_height), shift=(data.shift_x, data.shift_y),
                ortho_scale=data.ortho_scale, clip=(data.clip_start, data.clip_end), pixel_aspect=(render.pixel_aspect_x, render.pixel_aspect_y),
                world_from_camera=matrix(evaluated.matrix_world, camera.name_full),
                projection_matrix=matrix_values(evaluated.calc_matrix_camera(graph, x=width, y=height,
                    scale_x=render.pixel_aspect_x, scale_y=render.pixel_aspect_y), 'camera projection'), _render_size=(width, height))


def sample_group(group, graph, tick, frame):
    source = group['_source']
    for mesh in group['meshes']:
        root, signature = validate_mesh(mesh['_source'], bpy.context.scene, frame, graph)
        if root != source or signature != mesh['_modifier_signature']:
            fail(f'{mesh["name"]} at frame {frame:.17g}', 'render-enabled armature modifier configuration changed')
        if root.type == 'ARMATURE':
            for name in mesh['_weighted_bones']:
                bone = root.evaluated_get(graph).data.bones[name]
                if bone.bbone_segments > 1:
                    fail(f'{mesh["name"]}/{name} at frame {frame:.17g}',
                         f'weighted deform B-Bone has {bone.bbone_segments} segments; segment deformation is unsupported')
    evaluated = source.evaluated_get(graph)
    world = evaluated.matrix_world.copy()
    rigid(world @ group['_reference_inverse'], f'{source.name_full} object motion')
    skin = []
    for bone, rest_inverse in zip(group['bones'], group['_inverse_rests']):
        deformation = world @ evaluated.pose.bones[bone['name']].matrix @ rest_inverse
        rigid(deformation, f'{source.name_full}/{bone["name"]} skin motion')
        skin.append(matrix(deformation, 'world skin'))
    morph_weights = []
    for mesh_index, mesh in enumerate(group['meshes']):
        obj = mesh['_source']
        relative = world.inverted() @ obj.evaluated_get(graph).matrix_world
        if max(abs(relative[r][c] - mesh['_reference_relative'][r][c]) for r in range(4) for c in range(4)) > 1e-5:
            fail(obj.name_full, 'mesh motion relative to the shared model root is unsupported')
        if mesh['_key_names']:
            keys = obj.data.shape_keys.evaluated_get(graph)
            for morph_index, name in enumerate(mesh['_key_names']):
                key = keys.key_blocks[name]
                value = 0.0 if key.mute else number(key.value, 'morph weight')
                morph_weights.append((mesh_index, morph_index, value))
    return dict(tick=tick, source_frame=frame, object_world=matrix(world, 'object world'),
                bone_skin_world=skin, morph_weights=morph_weights)


class Writer:
    def __init__(self, stream=None):
        self.stream = stream
        self.size = 0

    def raw(self, data):
        if len(data) > MAX_BYTES - self.size:
            fail('scene packet', f'exceeds {MAX_BYTES} bytes')
        self.size += len(data)
        if self.stream is not None and self.stream.write(data) != len(data):
            raise OSError('Incomplete scene packet write')

    def pack(self, format_string, *values):
        if 'd' in format_string and any(not math.isfinite(value) for value in values):
            fail('scene packet', 'non-finite number')
        self.raw(struct.pack('<' + format_string, *values))

    def name(self, value):
        encoded = text(value)
        self.pack('I', len(encoded))
        self.raw(encoded)

    def array(self, values, write):
        self.pack('I', len(values))
        for value in values:
            write(value)


def write_packet(packet, writer):
    w = writer
    w.raw(b'NEXASCN1')
    w.name(packet['scene_name'])
    w.pack('5d2I', *(packet[name] for name in ('reference_frame', 'frame_start', 'frame_end', 'frames_per_second', 'unit_scale')), *packet['render_size'])

    def mesh(value):
        w.name(value['source_id']); w.name(value['name']); w.pack('16d', *value['world_from_mesh'])
        def vertex(v):
            w.pack('8d', *v['position'], *v['normal'], *v['uv'])
            w.array(v['weights'], lambda weight: w.pack('Id', *weight))
        w.array(value['vertices'], vertex)
        w.array(value['triangles'], lambda t: w.pack('4I', *t[0], t[1]))
        def material(m):
            w.name(m['name']); w.pack('4di', *m['diffuse'], m['image'] if m['image'] is not None else -1)
        w.array(value['materials'], material)
        def morph(m):
            w.name(m['source_id']); w.name(m['name'])
            w.array(m['offsets'], lambda item: w.pack('I3d', item[0], *item[1]))
        w.array(value['morphs'], morph)

    def group(value):
        w.name(value['source_id']); w.name(value['name']); w.pack('16d', *value['reference_object_world'])
        w.array(value['meshes'], mesh)
        def bone(b):
            w.name(b['source_id']); w.name(b['name'])
            w.pack('i16d6d', b['parent'] if b['parent'] is not None else -1, *b['rest_world'], *b['head_world'], *b['tail_world'])
        w.array(value['bones'], bone)
        def pose(p):
            w.pack('Id16d', p['tick'], p['source_frame'], *p['object_world'])
            w.array(p['bone_skin_world'], lambda m: w.pack('16d', *m))
            w.array(p['morph_weights'], lambda m: w.pack('2Id', *m))
        w.array(value['poses'], pose)
    w.array(packet['groups'], group)
    def image(value):
        w.name(value['source_id']); w.name(value['name']); w.pack('3I', value['width'], value['height'], len(value['png'])); w.raw(value['png'])
    w.array(packet['images'], image)
    def camera(value):
        w.pack('Id', value['tick'], value['source_frame']); w.name(value['source_id']); w.name(value['name'])
        w.pack('2B', value['projection'], value['sensor_fit'])
        w.pack('10d', value['lens_mm'], *value['sensor_mm'], *value['shift'], value['ortho_scale'], *value['clip'], *value['pixel_aspect'])
        w.pack('16d', *value['world_from_camera']); w.pack('16d', *value['projection_matrix'])
    w.array(packet['cameras'], camera)


def export(args):
    source = local_path(args.input, existing=True)
    output = local_path(args.packet)
    if output.exists() or output.is_symlink():
        raise FileExistsError(f'Packet already exists: {output}')
    if output == source or not output.parent.is_dir():
        fail('packet path', 'must name a new file in an existing local directory')
    selection = set(args.selection.split(','))
    if not selection or selection - {'model', 'camera', 'animation'} or '' in selection:
        fail('selection', 'choose a nonempty subset of model,camera,animation')
    bpy.context.preferences.filepaths.use_scripts_auto_execute = False
    bpy.ops.wm.open_mainfile(filepath=str(source), load_ui=False, use_scripts=False)
    scene = bpy.data.scenes.get(args.scene) if args.scene else bpy.context.scene
    if scene is None or not scene.view_layers:
        fail('scene', 'selected scene/view layer not found')
    layer = bpy.context.view_layer if scene == bpy.context.scene else scene.view_layers[0]
    reject_python_drivers(scene)
    reference = number(args.reference_frame if args.reference_frame is not None else scene.frame_current + scene.frame_subframe, 'reference frame')
    start = number(args.frame_start if args.frame_start is not None else scene.frame_start, 'frame start')
    end = number(args.frame_end if args.frame_end is not None else scene.frame_end, 'frame end')
    if start > end:
        fail('frame range', 'start must not exceed end')
    directory = Path(tempfile.mkdtemp(prefix='.nexa-scene-', dir=output.parent))
    budget = Budget()
    images = Images(directory, budget)
    created = False
    try:
        with bpy.context.temp_override(scene=scene, view_layer=layer):
            frame_set(scene, reference)
            graph = bpy.context.evaluated_depsgraph_get()
            evaluated_scene = scene.evaluated_get(graph)
            render = evaluated_scene.render
            fps = Fraction(render.fps) / Fraction(str(render.fps_base))
            number(float(fps), 'fps', positive=True)
            render_size = (max(1, render.resolution_x * render.resolution_percentage // 100), max(1, render.resolution_y * render.resolution_percentage // 100))
            if not all(1 <= size <= MAX_EDGE for size in render_size):
                fail('render size', f'exceeds {MAX_EDGE}')
            budget.add(68); budget.names(scene.name_full)
            sources = group_sources(scene, selection, graph, reference) if 'model' in selection or selection == {'animation'} else []
            if 'model' in selection and not any(objects for _, objects in sources):
                fail('models', 'no render-enabled mesh found')
            if selection == {'animation'} and not sources:
                fail('animation', 'no armature/independent mesh group found')
            groups = [extract_group(root, objects, graph, images, budget, 'animation' in selection, 'model' in selection, scene, reference) for root, objects in sources]
            packet = dict(scene_name=scene.name_full, reference_frame=reference, frame_start=start, frame_end=end,
                          frames_per_second=float(fps), unit_scale=number(evaluated_scene.unit_settings.scale_length, 'scene unit scale', positive=True),
                          render_size=render_size, groups=groups, images=images.images, cameras=[])
            if 'animation' in selection:
                span = (Fraction(str(end)) - Fraction(str(start))) * 30 / fps
                steps = (span.numerator + span.denominator - 1) // span.denominator
                count = steps + 1
                if not 1 <= count <= MAX_TICKS:
                    fail('range ticks', f'{count} exceeds {MAX_TICKS}')
                # Bound all pose allocations before evaluating/allocating sampled matrices.
                for group in groups:
                    morph_count = sum(len(mesh['morphs']) for mesh in group['meshes'])
                    budget.add(count * (148 + 128 * len(group['bones']) + 16 * morph_count))
                frames = (float(min(Fraction(str(end)), Fraction(str(start)) + tick * fps / 30)) for tick in range(count))
            else:
                count, frames = 1, iter([reference])
            for tick, frame in enumerate(frames):
                frame_set(scene, frame)
                graph = bpy.context.evaluated_depsgraph_get()
                current = scene.evaluated_get(graph)
                if Fraction(current.render.fps) / Fraction(str(current.render.fps_base)) != fps or current.unit_settings.scale_length != packet['unit_scale']:
                    fail('scene', 'animated FPS/unit scale is unsupported')
                if 'animation' in selection:
                    for group in groups:
                        group['poses'].append(sample_group(group, graph, tick, frame))
                if 'camera' in selection:
                    sample = camera_sample(scene, graph, tick, frame, budget)
                    if sample['_render_size'] != render_size:
                        fail('camera', 'animated render dimensions are unsupported')
                    packet['cameras'].append(sample)
                if tick % max(1, count // 100) == 0 or tick == count - 1:
                    print(f'NEXA_SCENE_PROGRESS {tick + 1}/{count}', file=sys.stderr, flush=True)
            counter = Writer()
            write_packet(packet, counter)
            if counter.size != budget.bytes:
                fail('scene packet', f'preflight size mismatch: {budget.bytes} != {counter.size}')
            with output.open('xb') as stream:
                created = True
                write_packet(packet, Writer(stream))
                stream.flush()
                os.fsync(stream.fileno())
            print(f'NEXA_SCENE_META groups={len(groups)} images={len(images.images)} cameras={len(packet["cameras"])} bytes={counter.size}', file=sys.stderr, flush=True)
            print('NEXA_SCENE_NOTICE Principled materials use the defined diffuse conversion; PBR shading is not reproduced.', file=sys.stderr, flush=True)
    except BaseException:
        if created:
            output.unlink()
        raise
    finally:
        for path in reversed(images.owned):
            path.unlink()
        directory.rmdir()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input', type=Path, required=True)
    parser.add_argument('--packet', type=Path, required=True)
    parser.add_argument('--selection', required=True)
    parser.add_argument('--scene')
    parser.add_argument('--reference-frame', type=float)
    parser.add_argument('--frame-start', type=float)
    parser.add_argument('--frame-end', type=float)
    parser.add_argument('--error', type=Path)
    if '--' not in sys.argv:
        parser.error('Exporter arguments must follow --')
    args = parser.parse_args(sys.argv[sys.argv.index('--') + 1:])
    try:
        export(args)
    except Exception as error:
        diagnostic = str(error).encode('utf-8', errors='replace')[:8192]
        if args.error is not None:
            error_path = local_path(args.error)
            with error_path.open('xb') as stream:
                stream.write(diagnostic)
        print(diagnostic.decode('utf-8', errors='replace'), file=sys.stderr, flush=True)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
