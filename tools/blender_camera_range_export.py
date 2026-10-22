"""Export a camera range using the trusted common packet exporter."""
import argparse
from fractions import Fraction
from pathlib import Path
import struct
import sys
import bpy

MAX_RANGE_SAMPLES = 108001
MAX_RANGE_BYTES = 128 * 1024 * 1024

def endpoint(value, label):
    result = Fraction(str(number(value, label)))
    if not -1048574 <= result <= 1048574:
        raise ValueError(f'{label} is outside Blender frame limits')
    return result

def sample(scene, layer, position, camera_name):
    frame = position.numerator // position.denominator
    scene.frame_set(frame, subframe=float(position - frame))
    graph = bpy.context.evaluated_depsgraph_get()
    camera = scene.objects.get(camera_name) if camera_name is not None else scene.camera
    if camera is None or camera.type != 'CAMERA' or scene.objects.get(camera.name) != camera:
        raise ValueError(f'Camera not found in selected scene: {camera_name!r}')
    packet = camera_packet(scene, layer, camera, graph)
    render = scene.evaluated_get(graph).render
    return packet, (integer(render.fps, 'fps', 1),
                    number(render.fps_base, 'fps_base', positive=True))

def range_main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--input', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--start')
    parser.add_argument('--end')
    parser.add_argument('--scene')
    parser.add_argument('--camera')
    if '--' not in sys.argv:
        parser.error('Arguments must follow --')
    args = parser.parse_args(sys.argv[sys.argv.index('--') + 1:])
    source = args.input.resolve(strict=True)
    output = args.output.absolute()
    if not source.is_file():
        raise ValueError('Input must be a file')
    if output.exists() or output.is_symlink():
        raise FileExistsError(f'Output already exists: {output}')
    bpy.context.preferences.filepaths.use_scripts_auto_execute = False
    bpy.ops.wm.open_mainfile(filepath=str(source), load_ui=False, use_scripts=False)
    scene = bpy.data.scenes.get(args.scene) if args.scene is not None else bpy.context.scene
    if scene is None or not scene.view_layers:
        raise ValueError('Scene or view layer not found')
    layer = bpy.context.view_layer if scene == bpy.context.scene else scene.view_layers[0]
    reject_python_drivers(scene)
    start = endpoint(args.start if args.start is not None else scene.frame_start, 'start')
    end = endpoint(args.end if args.end is not None else scene.frame_end, 'end')
    if start > end:
        raise ValueError('start must not exceed end')
    with bpy.context.temp_override(scene=scene, view_layer=layer):
        first, fps = sample(scene, layer, start, args.camera)
        rate = Fraction(fps[0]) / Fraction(str(fps[1]))
        number(float(rate), 'input_fps', positive=True)
        ticks = (end - start) * 30 / rate
        last = (ticks.numerator + ticks.denominator - 1) // ticks.denominator
        count = last + 1
        if not 1 <= count <= MAX_RANGE_SAMPLES:
            raise ValueError(f'Range requires {count} samples; limit is {MAX_RANGE_SAMPLES}')
        extension = Fraction(last, 30) - (end - start) / rate
        print(f'NEXA_RANGE_META fps={float(rate):.17g} samples={count} '
              f'endpoint_extension_seconds={float(extension):.17g}', file=sys.stderr, flush=True)
        data = bytearray(b'NEXARAN1') + struct.pack('<I', count)
        for index in range(count):
            if index == 0:
                packet, current_rate = first, fps
            else:
                packet, current_rate = sample(scene, layer, min(end, start + index * rate / 30), args.camera)
            if current_rate != fps:
                raise ValueError('Animated input FPS is unsupported')
            if len(data) + 4 + len(packet) > MAX_RANGE_BYTES:
                raise ValueError('Camera range exceeds transport limit')
            data.extend(struct.pack('<I', len(packet)))
            data.extend(packet)
            if index % max(1, count // 100) == 0 or index == last:
                print(f'NEXA_RANGE_PROGRESS {index + 1}/{count}', file=sys.stderr, flush=True)
    with output.open('xb') as stream:
        if stream.write(data) != len(data):
            raise OSError('Incomplete camera range write')

if __name__ == '__main__':
    range_main()
