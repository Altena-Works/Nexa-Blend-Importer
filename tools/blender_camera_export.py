"""Export one evaluated Blender camera as a bounded Nexa binary packet."""
import argparse
import math
from pathlib import Path
import struct
import sys

import bpy


MAX_PACKET_BYTES = 65536


def number(value, label, positive=False):
    value = float(value)
    if not math.isfinite(value) or (positive and value <= 0):
        raise ValueError(f'{label} must be finite' + (' and positive' if positive else ''))
    return value


def integer(value, label, minimum=0, maximum=0xFFFFFFFF):
    if not isinstance(value, int) or not minimum <= value <= maximum:
        raise ValueError(f'{label} must be an integer in [{minimum}, {maximum}]')
    return value


def append_name(packet, value, label):
    if not isinstance(value, str) or not value or '\x00' in value:
        raise ValueError(f'{label} must be a nonempty string without NUL')
    encoded = value.encode('utf-8', errors='strict')
    if len(encoded) > MAX_PACKET_BYTES:
        raise ValueError(f'{label} is too long')
    packet.extend(struct.pack('<I', len(encoded)))
    packet.extend(encoded)


def matrix_values(matrix, label):
    if len(matrix) != 4 or any(len(row) != 4 for row in matrix):
        raise ValueError(f'{label} must be a 4 by 4 matrix')
    return [number(matrix[row][column], f'{label}[{row},{column}]')
            for row in range(4) for column in range(4)]


def reject_python_drivers(scene):
    # Include all loaded dependencies and embedded node trees before frame evaluation.
    user_map = bpy.data.user_map()
    owners = set(user_map)
    for users in user_map.values():
        owners.update(users)
    owners.add(scene)
    for owner in tuple(owners):
        node_tree = getattr(owner, 'node_tree', None)
        if node_tree is not None:
            owners.add(node_tree)
    for owner in sorted(owners, key=lambda item: (item.bl_rna.identifier, item.name_full)):
        animation = getattr(owner, 'animation_data', None)
        if animation is None:
            continue
        for curve in animation.drivers:
            driver = curve.driver
            if driver.type == 'SCRIPTED' and not driver.is_simple_expression:
                raise ValueError(f'Unsupported Python driver on {owner.name_full!r}: '
                                 f'{curve.data_path}[{curve.array_index}]')


def camera_packet(scene, view_layer, camera, depsgraph):
    evaluated_camera = camera.evaluated_get(depsgraph)
    if not evaluated_camera.is_evaluated:
        raise ValueError('Selected camera is not evaluated in the selected view layer')
    evaluated_scene = scene.evaluated_get(depsgraph)
    data = evaluated_camera.data
    camera_types = {'PERSP': 0, 'ORTHO': 1}
    sensor_fits = {'AUTO': 0, 'HORIZONTAL': 1, 'VERTICAL': 2}
    if data.type not in camera_types:
        raise ValueError(f'Unsupported camera type: {data.type}')
    if data.sensor_fit not in sensor_fits:
        raise ValueError(f'Unsupported sensor fit: {data.sensor_fit}')

    version = tuple(integer(value, 'Blender version') for value in bpy.app.version)
    if len(version) != 3:
        raise ValueError('Blender version must contain three integers')
    frame_current = integer(scene.frame_current, 'frame_current', -(2 ** 31), 2 ** 31 - 1)
    subframe = number(scene.frame_subframe, 'frame_subframe')
    if not 0 <= subframe < 1:
        raise ValueError('frame_subframe must be in [0, 1)')
    frame_final = number(scene.frame_current_final, 'frame_current_final')
    frame_start = integer(scene.frame_start, 'frame_start', -(2 ** 31), 2 ** 31 - 1)
    frame_end = integer(scene.frame_end, 'frame_end', -(2 ** 31), 2 ** 31 - 1)
    if frame_start > frame_end:
        raise ValueError('frame_start must not exceed frame_end')

    render = evaluated_scene.render
    fps = integer(render.fps, 'fps', 1)
    fps_base = number(render.fps_base, 'fps_base', positive=True)
    frame_map_old = integer(render.frame_map_old, 'frame_map_old', 1)
    frame_map_new = integer(render.frame_map_new, 'frame_map_new', 1)
    clip_start = number(data.clip_start, 'clip_start', positive=True)
    clip_end = number(data.clip_end, 'clip_end', positive=True)
    if clip_end <= clip_start:
        raise ValueError('clip_end must exceed clip_start')
    camera_values = (
        number(data.lens, 'lens', positive=True),
        number(data.sensor_width, 'sensor_width', positive=True),
        number(data.sensor_height, 'sensor_height', positive=True),
        number(data.shift_x, 'shift_x'),
        number(data.shift_y, 'shift_y'),
        number(data.ortho_scale, 'ortho_scale', positive=True),
        clip_start,
        clip_end,
        number(evaluated_scene.unit_settings.scale_length, 'scale_length', positive=True),
    )
    resolution_x = integer(render.resolution_x, 'resolution_x', 1)
    resolution_y = integer(render.resolution_y, 'resolution_y', 1)
    percentage = integer(render.resolution_percentage, 'resolution_percentage', 1)
    aspect_x = number(render.pixel_aspect_x, 'pixel_aspect_x', positive=True)
    aspect_y = number(render.pixel_aspect_y, 'pixel_aspect_y', positive=True)
    width = max(1, resolution_x * percentage // 100)
    height = max(1, resolution_y * percentage // 100)
    world = matrix_values(evaluated_camera.matrix_world, 'matrix_world')
    projection = matrix_values(evaluated_camera.calc_matrix_camera(
        depsgraph, x=width, y=height, scale_x=aspect_x, scale_y=aspect_y), 'projection')

    packet = bytearray(b'NEXACAM1')
    packet.extend(struct.pack('<3I', *version))
    append_name(packet, scene.name, 'scene name')
    append_name(packet, view_layer.name, 'view layer name')
    append_name(packet, camera.name, 'camera name')
    packet.extend(struct.pack('<iddiiIdII', frame_current, subframe, frame_final,
                              frame_start, frame_end, fps, fps_base,
                              frame_map_old, frame_map_new))
    packet.extend(struct.pack('<BB', camera_types[data.type], sensor_fits[data.sensor_fit]))
    packet.extend(struct.pack('<9d', *camera_values))
    packet.extend(struct.pack('<3I2d', resolution_x, resolution_y, percentage,
                              aspect_x, aspect_y))
    packet.extend(struct.pack('<16d', *world))
    packet.extend(struct.pack('<16d', *projection))
    if len(packet) > MAX_PACKET_BYTES:
        raise ValueError(f'Camera packet exceeds {MAX_PACKET_BYTES} bytes')
    return packet


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--frame', type=float, required=True)
    parser.add_argument('--scene')
    parser.add_argument('--camera')
    if '--' not in sys.argv:
        parser.error('Exporter arguments must follow Blender\'s -- separator')
    args = parser.parse_args(sys.argv[sys.argv.index('--') + 1:])
    requested_frame = number(args.frame, 'frame')
    frame = math.floor(requested_frame)
    if not -1048574 <= frame <= 1048574:
        raise ValueError('Frame floor must be in [-1048574, 1048574]')
    subframe = requested_frame - frame
    source = args.input.resolve(strict=True)
    output = args.output.absolute()
    if not source.is_file():
        raise ValueError('Input must be a file')
    if output.exists() or output.is_symlink():
        raise FileExistsError(f'Output already exists: {output}')

    bpy.context.preferences.filepaths.use_scripts_auto_execute = False
    bpy.ops.wm.open_mainfile(filepath=str(source), load_ui=False, use_scripts=False)
    scene = bpy.data.scenes.get(args.scene) if args.scene is not None else bpy.context.scene
    if scene is None:
        raise ValueError(f'Scene not found: {args.scene!r}')
    view_layer = (bpy.context.view_layer if scene == bpy.context.scene
                  else scene.view_layers[0])
    reject_python_drivers(scene)
    with bpy.context.temp_override(scene=scene, view_layer=view_layer):
        scene.frame_set(frame, subframe=subframe)
        depsgraph = bpy.context.evaluated_depsgraph_get()
        camera = scene.objects.get(args.camera) if args.camera is not None else scene.camera
        if camera is None:
            raise ValueError(f'Camera not found: {args.camera!r}')
        if camera.type != 'CAMERA' or scene.objects.get(camera.name) != camera:
            raise ValueError('Selected object must be a camera in the selected scene')
        packet = camera_packet(scene, view_layer, camera, depsgraph)
    # Exclusive creation occurs only after every field and the whole packet are valid.
    with output.open('xb') as stream:
        if stream.write(packet) != len(packet):
            raise OSError('Incomplete camera packet write')


if __name__ == '__main__':
    main()
