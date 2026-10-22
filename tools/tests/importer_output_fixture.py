"""Independent PMX/VMD reader and analytic Blender-fixture integration assertions.

This gate checks exported files and skinning mathematics. It does not qualify MMD
runtime rendering, plugin UI, or the separate camera pixel-projection acceptance.
"""
import argparse
import csv
import hashlib
import json
import math
from pathlib import Path
import struct

from blender_scene_fixture import (CHILD_BONE, ROOT_BONE, Reader, check_fixture,
                                   close, multiply, rotation, translation)


SCALE = 12.5
AXES = (0, 2, 1)
ALIASES = ('NexaRoot', 'bone_1', CHILD_BONE)
MORPH = '持上げ'
SELECTIONS = {
    'all': (1, 1, 1), 'model': (1, 0, 0), 'camera': (0, 1, 0),
    'model-camera': (1, 1, 0), 'model-animation': (1, 0, 1),
    'camera-animation': (0, 1, 1), 'animation': (0, 0, 1),
}


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def mapped_point(point):
    return tuple(SCALE * point[axis] for axis in AXES)


def point(matrix, vector):
    return tuple(sum(matrix[4 * row + col] * vector[col] for col in range(3)) + matrix[4 * row + 3]
                 for row in range(3))


def mapped_matrix(matrix):
    out = list(translation(0))
    for row in range(3):
        for col in range(3):
            out[4 * row + col] = matrix[4 * AXES[row] + AXES[col]]
        out[4 * row + 3] = SCALE * matrix[4 * AXES[row] + 3]
    return tuple(out)


def subtract(a, b):
    return tuple(x - y for x, y in zip(a, b))


def add(a, b):
    return tuple(x + y for x, y in zip(a, b))


def quaternion_matrix(q):
    x, y, z, w = q
    norm = sum(v * v for v in q)
    assert abs(norm - 1) <= 1e-6, ('quaternion norm', norm)
    return (1-2*(y*y+z*z), 2*(x*y-z*w), 2*(x*z+y*w), 0,
            2*(x*y+z*w), 1-2*(x*x+z*z), 2*(y*z-x*w), 0,
            2*(x*z-y*w), 2*(y*z+x*w), 1-2*(x*x+y*y), 0,
            0, 0, 0, 1)


def rotation_error(a, b):
    # atan2 avoids acos amplification close to the identity rotation.
    relative = [[sum(a[4*k+r] * b[4*k+c] for k in range(3)) for c in range(3)] for r in range(3)]
    sine = math.sqrt(sum(v*v for v in (relative[2][1]-relative[1][2],
                                      relative[0][2]-relative[2][0],
                                      relative[1][0]-relative[0][1]))) / 2
    cosine = (sum(relative[i][i] for i in range(3))-1) / 2
    return abs(math.atan2(sine, cosine))


class OutputReader(Reader):
    def text(self):
        size = self.scalar('i')
        assert 0 <= size <= 1024 * 1024 and size % 2 == 0
        return self.take(size).decode('utf-16le')

    def count(self):
        value = self.scalar('i')
        assert 0 <= value <= 4_000_000
        return value

    def fixed_name(self, size):
        field = self.take(size)
        name, separator, padding = field.partition(b'\0')
        assert separator and not any(padding), 'VMD names must be terminated and zero-padded'
        return name.decode('cp932')


def read_pmx(path):
    r = OutputReader(path.read_bytes())
    assert r.take(4) == b'PMX ' and r.scalar('f') == 2.0
    assert r.scalar('B') == 8
    assert r.take(8) == bytes((0, 0, 4, 4, 4, 4, 4, 4)), 'PMX UTF16LE / index layout'
    model = dict(name=r.text(), original=r.text(), comment=r.text(), english_comment=r.text())
    vertices = []
    for _ in range(r.count()):
        p, n, uv = r.unpack('3f'), r.unpack('3f'), r.unpack('2f')
        kind = r.scalar('B')
        if kind == 0:
            weights = [(r.scalar('i'), 1.0)]
        elif kind == 1:
            a, b, weight = r.unpack('2if')
            weights = [(a, weight), (b, 1-weight)]
        elif kind == 2:
            indices, values = r.unpack('4i'), r.unpack('4f')
            weights = [(i, w) for i, w in zip(indices, values) if w > 0]
        else:
            raise AssertionError(('unexpected deform type', kind))
        assert r.scalar('f') == 0, 'edge disabled'
        vertices.append(dict(position=p, normal=n, uv=uv, weights=weights, kind=kind))
    model['vertices'] = vertices
    model['surfaces'] = [r.scalar('I') for _ in range(r.count())]
    model['textures'] = [r.text() for _ in range(r.count())]
    materials = []
    for _ in range(r.count()):
        name, original = r.text(), r.text()
        diffuse, specular_power_ambient = r.unpack('4f'), r.unpack('7f')
        flags, edge, image, sphere, sphere_mode, shared_toon = (r.scalar('B'), r.unpack('5f'),
            r.scalar('i'), r.scalar('i'), r.scalar('B'), r.scalar('B'))
        toon = r.scalar('B' if shared_toon else 'i')
        materials.append(dict(name=name, original=original, diffuse=diffuse,
            lighting=specular_power_ambient, flags=flags, edge=edge, image=image,
            sphere=sphere, sphere_mode=sphere_mode, toon=toon, memo=r.text(), surfaces=r.count()))
    model['materials'] = materials
    bones = []
    for _ in range(r.count()):
        bone = dict(name=r.text(), original=r.text(), head=r.unpack('3f'), parent=r.scalar('i'),
                    layer=r.scalar('i'), flags=r.scalar('H'))
        flags = bone['flags']
        bone['tail'] = r.scalar('i') if flags & 1 else r.unpack('3f')
        assert flags & ~(0x0800 | 0x001e) == 0, 'fixture must not acquire IK, append or external-parent semantics'
        bone['axes'] = r.unpack('6f') if flags & 0x0800 else None
        bones.append(bone)
    model['bones'] = bones
    morphs = []
    for _ in range(r.count()):
        name, original, panel, kind = r.text(), r.text(), r.scalar('B'), r.scalar('B')
        assert (panel, kind) == (4, 1), 'only the known vertex morph is expected'
        morphs.append(dict(name=name, original=original, offsets=[r.unpack('I3f') for _ in range(r.count())]))
    model['morphs'] = morphs
    displays = []
    for _ in range(r.count()):
        label, english, special = r.text(), r.text(), r.scalar('B')
        displays.append((label, english, special, [r.unpack('BI') for _ in range(r.count())]))
    assert len(displays) == 3
    assert r.count() == r.count() == 0, 'no inferred rigid bodies or joints'
    assert r.offset == len(r.payload), 'unexpected PMX trailing fields'
    return model


def read_vmd(path):
    r = OutputReader(path.read_bytes())
    assert r.take(30) == b'Vocaloid Motion Data 0002'.ljust(30, b'\0')
    model_name = r.fixed_name(20)
    bones = {}
    for _ in range(r.scalar('I')):
        name, tick = r.fixed_name(15), r.scalar('I')
        translation_value, q, interpolation = r.unpack('3f'), r.unpack('4f'), r.take(64)
        for offset in range(4):
            assert interpolation[offset] == interpolation[offset+4]
            assert interpolation[offset+8] == interpolation[offset+12]
        assert (name, tick) not in bones
        bones[name, tick] = dict(translation=translation_value, rotation=q)
    morphs = {}
    for _ in range(r.scalar('I')):
        name, tick, weight = r.fixed_name(15), r.scalar('I'), r.scalar('f')
        assert (name, tick) not in morphs
        morphs[name, tick] = weight
    cameras = []
    for _ in range(r.scalar('I')):
        tick, distance = r.unpack('If')
        center, angles, interpolation = r.unpack('3f'), r.unpack('3f'), r.take(24)
        assert all(interpolation[i] == interpolation[i+2] and interpolation[i+1] == interpolation[i+3]
                   for i in range(0, 24, 4)), 'camera linear interpolation'
        cameras.append(dict(tick=tick, distance=distance, center=center, angles=angles,
                            fov=r.scalar('I'), orthographic=r.scalar('B')))
    assert [r.scalar('I') for _ in range(3)] == [0, 0, 0], 'no light, shadow, or IK tracks'
    assert r.offset == len(r.payload), 'unexpected VMD trailing fields'
    return dict(name=model_name, bones=bones, morphs=morphs, cameras=cameras)


def read_scene(path):
    r = Reader(path.read_bytes())
    assert r.take(8) == b'NEXASCN1'
    name, settings = r.name(), r.unpack('5d2I')
    groups, images, cameras = r.array(r.group), r.array(r.image), r.array(r.camera)
    assert r.offset == len(r.payload)
    assert name == 'NexaFixtureScene' and settings == (1, 1, 3, 30, 1, 32, 24)
    assert len(groups) == len(images) == 1 and len(cameras) == 3
    return groups[0], images[0], cameras


def check_map(path):
    lines = [line for line in path.read_text(encoding='utf-8').splitlines() if not line.startswith('#')]
    rows = list(csv.DictReader(lines, delimiter='\t'))
    expected = [('model', 'FixtureRig', 'FixtureRig'), ('root', 'NexaRoot', 'NexaRoot'),
                ('bone', ROOT_BONE, 'bone_1'), ('bone', CHILD_BONE, CHILD_BONE), ('morph', MORPH, MORPH)]
    assert len(rows) == len(expected)
    for row, (kind, original, alias) in zip(rows, expected):
        assert (row['kind'], row['original'], row['pmx_vmd_alias']) == (kind, original, alias)
        assert row['source_id'] and bytes.fromhex(row['cp932_hex']) == alias.encode('cp932')
        assert len(alias.encode('cp932')) <= 14


def check_model(model, group, image, directory):
    assert (model['name'], model['original']) == ('FixtureRig', 'FixtureRig')
    mesh, = group['meshes']
    assert len(model['vertices']) == len(mesh['vertices']) == 5
    assert [b['name'] for b in model['bones']] == list(ALIASES)
    assert [b['original'] for b in model['bones']] == ['NexaRoot', ROOT_BONE, CHILD_BONE]
    assert [b['parent'] for b in model['bones']] == [-1, 0, 1]
    for bone, expected in zip(model['bones'], [(0, 0, 0), (25, 0, 0), (25, 0, 12.5)]):
        close(bone['head'], expected, 1e-4)
    for actual, source in zip(model['vertices'], mesh['vertices']):
        close(actual['position'], mapped_point(point(mesh['world'], source['position'])), 1e-4)
        close(actual['normal'], (0, 1, 0), 1e-6)
        close(actual['uv'], (source['uv'][0], 1-source['uv'][1]), 1e-6)
        expected = [(i+1, weight) for i, weight in source['weights']] or [(0, 1.0)]
        assert [i for i, _ in actual['weights']] == [i for i, _ in expected]
        close([w for _, w in actual['weights']], [w for _, w in expected], 1e-6)
    assert {v['kind'] for v in model['vertices']} == {0, 1}, 'known BDEF1 and BDEF2'
    expected_surfaces = [i for triangle in mesh['triangles'] for i in (triangle[0], triangle[2], triangle[1])]
    assert model['surfaces'] == expected_surfaces
    assert model['textures'] == ['textures/texture_0001.png']
    material, = model['materials']
    assert (material['name'], material['original'], material['image'], material['surfaces']) == (
        'SimplePrincipled', 'SimplePrincipled', 0, 6)
    close(material['diffuse'], (1, 1, 1, 1), 1e-6)
    assert material['lighting'] == (0,)*7 and material['edge'] == (0,)*5
    assert (material['flags'], material['sphere'], material['sphere_mode'], material['toon']) == (0, -1, 0, -1)
    assert (directory / model['textures'][0]).read_bytes() == image['png'], 'PNG differs from independently checked red pixels'
    morph, = model['morphs']
    source_morph, = mesh['morphs']
    assert (morph['name'], morph['original']) == (MORPH, MORPH)
    assert len(morph['offsets']) == len(source_morph['offsets']) == 3
    for actual, expected in zip(morph['offsets'], source_morph['offsets']):
        assert actual[0] == expected[0]
        close(actual[1:], mapped_point(expected[1:]), 1e-4)


def check_motion(vmd, model, group):
    assert vmd['name'] == model['name'] == 'FixtureRig'
    assert not vmd['cameras']
    assert set(vmd['bones']) == {(name, tick) for name in ALIASES for tick in range(3)}
    assert set(vmd['morphs']) == {(MORPH, tick) for tick in range(3)}
    maximum_position, maximum_rotation, maximum_morph = 0.0, 0.0, 0.0
    mesh, = group['meshes']
    delta_by_vertex = {item[0]: item[1:] for item in model['morphs'][0]['offsets']}
    source_delta = {item[0]: item[1:] for item in mesh['morphs'][0]['offsets']}
    for tick in range(3):
        ratio = tick / 2
        # Independent fixture equations: parent and COPY_ROTATION object motion,
        # root-bone rotation, and child local X translation, all linear in time.
        world = multiply(translation(2 + 0.4*ratio), rotation(0.1*ratio))
        expected_skin = [multiply(world, translation(-2)),
                         multiply(multiply(world, rotation(0.2*ratio)), translation(-2)),
                         multiply(multiply(multiply(world, rotation(0.2*ratio)), translation(0.2*ratio)), translation(-2))]
        close(group['poses'][tick]['world'], world, 1e-6)
        for source_skin, analytic in zip(group['poses'][tick]['skin'], expected_skin[1:]):
            close(source_skin, analytic, 1e-6)
        frames, skins = [], []
        for bone in model['bones']:
            key = vmd['bones'][bone['name'], tick]
            local = list(quaternion_matrix(key['rotation']))
            parent = bone['parent']
            parent_rest = (0, 0, 0) if parent == -1 else model['bones'][parent]['head']
            rest_offset = subtract(bone['head'], parent_rest)
            for axis in range(3):
                local[4*axis+3] = rest_offset[axis] + key['translation'][axis]
            frame = tuple(local) if parent == -1 else multiply(frames[parent], local)
            frames.append(frame)
            skin = multiply(frame, translation(*(-v for v in bone['head'])))
            skins.append(skin)
            expected = mapped_matrix(expected_skin[len(skins)-1])
            error = max(abs(skin[4*r+3]-expected[4*r+3]) for r in range(3))
            maximum_position = max(maximum_position, error)
            maximum_rotation = max(maximum_rotation, rotation_error(skin, expected))
            assert error <= 1e-4 and maximum_rotation <= 1e-4, ('skin transform', tick, bone['name'], error, maximum_rotation)
        weight = vmd['morphs'][MORPH, tick]
        maximum_morph = max(maximum_morph, abs(weight - 0.8*ratio))
        assert maximum_morph <= 1e-6
        for index, (vertex, source_vertex) in enumerate(zip(model['vertices'], mesh['vertices'])):
            deformed = add(vertex['position'], tuple(weight*v for v in delta_by_vertex.get(index, (0, 0, 0))))
            actual = tuple(sum(w*point(skins[bone], deformed)[axis] for bone, w in vertex['weights']) for axis in range(3))
            source_local = add(source_vertex['position'], tuple(0.8*ratio*v for v in source_delta.get(index, (0, 0, 0))))
            source_rest = point(mesh['world'], source_local)
            source_weights = [(bone+1, w) for bone, w in source_vertex['weights']] or [(0, 1.0)]
            expected_blender = tuple(sum(w*point(expected_skin[bone], source_rest)[axis] for bone, w in source_weights) for axis in range(3))
            expected = mapped_point(expected_blender)
            error = max(abs(a-b) for a, b in zip(actual, expected))
            maximum_position = max(maximum_position, error)
            assert error <= 1e-4, ('skinned vertex', tick, index, error, actual, expected)
    return dict(maximum_position_error=maximum_position, maximum_rotation_error_rad=maximum_rotation,
                maximum_morph_weight_error=maximum_morph, ticks=3, weighted_vertices=15)


def check_camera(vmd, animated):
    assert vmd['name'] == 'カメラ・照明' and not vmd['bones'] and not vmd['morphs']
    assert [key['tick'] for key in vmd['cameras']] == ([0, 1, 2] if animated else [0])
    for key in vmd['cameras']:
        orthographic = int(key['tick'] >= 1)
        assert key['orthographic'] == orthographic
        close(key['angles'], (-math.pi/6, 0, 0), 1e-6)
        eye = (37.5 if orthographic else 0, 50, -75)
        # For this fixed camera, the VMD back-axis is (0,-1/2,sqrt(3)/2).
        back = (0, -0.5, math.sqrt(3)/2)
        actual_eye = tuple(key['center'][i] + key['distance']*back[i] for i in range(3))
        close(actual_eye, eye, 1e-4)
        close((key['distance'],), (-50 if orthographic else 0,), 1e-4)
        assert key['fov'] == (30 if orthographic else 30)


def check(args):
    before = json.loads(args.before.read_text(encoding='utf-8'))
    assert sha(args.source) == before['source_sha256'], 'input .blend changed'
    for relative, digest in before['preexisting_files'].items():
        assert sha(args.output_root / relative) == digest, ('existing file changed', relative)
    assert sha(args.source) == args.source_sha256, 'fixture source hash differs from M1 evidence'
    check_fixture(args.packet, args.source, args.source_sha256, args.result.parent / 'source-expectations.json')
    group, image, _ = read_scene(args.packet)
    roots = [args.output_root] + ([args.supplement_root] if args.supplement_root else [])
    rows = []
    for root in roots:
        rows.extend(csv.DictReader((root / 'receipt.tsv').read_text(encoding='utf-8').splitlines(), delimiter='\t'))
        if root != args.output_root:
            snapshot = json.loads((root / 'before.json').read_text(encoding='utf-8'))
            assert sha(args.source) == snapshot['source_sha256']
            for relative, digest in snapshot['preexisting_files'].items():
                assert sha(root / relative) == digest
    cases = {row['case'] for row in rows}
    assert len(cases) == len(rows) and cases <= set(SELECTIONS)
    missing = sorted(set(SELECTIONS) - cases)
    assert not missing or args.incomplete_reason, 'seven successful request receipts are required'
    directories = {row['case']: Path(row['directory']) for row in rows}
    baseline = read_pmx(directories['all'] / 'model_0001.pmx')
    reports = []
    for row in rows:
        name, directory = row['case'], Path(row['directory'])
        selected = tuple(int(row[k]) for k in ('model', 'camera', 'animation'))
        assert selected == SELECTIONS[name]
        model, camera, animation = selected
        model_motion = animation and (model or name == 'animation')
        expected = set()
        if model: expected.update(('model_0001.pmx', 'textures/texture_0001.png', 'name-map.tsv'))
        if camera: expected.add('camera.vmd')
        if model_motion: expected.update(('model_0001.motion.vmd', 'name-map.tsv'))
        actual = {p.relative_to(directory).as_posix() for p in directory.rglob('*') if p.is_file()}
        assert actual == expected, (name, actual, expected)
        report = dict(case=name, directory=str(directory), files={p: sha(directory / p) for p in sorted(actual)})
        if model:
            check_model(read_pmx(directory / 'model_0001.pmx'), group, image, directory)
        if model or model_motion:
            check_map(directory / 'name-map.tsv')
        if model_motion:
            report['motion'] = check_motion(read_vmd(directory / 'model_0001.motion.vmd'), baseline, group)
        if camera:
            check_camera(read_vmd(directory / 'camera.vmd'), bool(animation))
        reports.append(report)
    if not missing:
        assert (roots[-1] / 'protection.txt').is_file(), 'Rust protection group did not finish'
    for root in roots:
        assert not any((root / 'scratch').iterdir()), 'extraction scratch was not collected'
        for directory in (root / 'outputs').iterdir():
            assert directory.name == 'existing-output' or any(directory.samefile(p) for p in directories.values()), ('unexpected published/staging folder', directory)
    report = dict(status='incomplete' if missing else 'passed', source_sha256=sha(args.source), input_unchanged=True,
                  preexisting_files_unchanged=True, selections=reports,
                  missing_selections=missing, incomplete_reason=args.incomplete_reason,
                  real_blender_request_gate=('six passed; animation-only failed before matcher fix and was not rerun'
                                             if args.supplement_root else ('incomplete' if missing else 'passed')),
                  supplemental_publication=('animation-only from saved packet; zero Blender launches'
                                            if args.supplement_root else None),
                  limits=dict(position=1e-4, rotation_radians=1e-4, weights_and_morph=1e-6),
                  evidence='independent file decoding and analytic Blender fixture; not MMD runtime',
                  remaining=['MMD model/material/pose/camera loading and GUI gate',
                             'PERSP/ORTHO rendered camera projection 0.5px gate'])
    with args.result.open('x', encoding='utf-8') as stream:
        json.dump(report, stream, ensure_ascii=False, indent=2)
        stream.write('\n')
    print(json.dumps(dict(status=report['status'], selections=len(rows), source_sha256=report['source_sha256'],
                          input_unchanged=True, independent_skin_ticks=3, mmd_runtime='not_run')))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=('snapshot', 'check'))
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--output-root', type=Path, required=True)
    parser.add_argument('--before', type=Path)
    parser.add_argument('--packet', type=Path)
    parser.add_argument('--source-sha256')
    parser.add_argument('--incomplete-reason', help='Record partial evidence as incomplete after an earlier stopped run')
    parser.add_argument('--supplement-root', type=Path, help='A separately authorized saved-packet continuation receipt')
    parser.add_argument('--result', type=Path, required=True)
    args = parser.parse_args()
    if args.mode == 'snapshot':
        record = dict(source_sha256=sha(args.source), preexisting_files={
            p.relative_to(args.output_root).as_posix(): sha(p) for p in args.output_root.rglob('*') if p.is_file()})
        with args.result.open('x', encoding='utf-8') as stream:
            json.dump(record, stream, indent=2)
            stream.write('\n')
        print(json.dumps(record))
    else:
        check(args)


if __name__ == '__main__':
    main()
