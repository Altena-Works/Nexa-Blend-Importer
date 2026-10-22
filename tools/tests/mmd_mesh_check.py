"""Compare saved native rig silhouettes with independent fixture vertices.

This supplements the world-space PMX/VMD checks; pixel coverage cannot prove
1e-4 world-space precision. Original BMPs are neither modified nor resampled.
"""
import argparse
import hashlib
import json
import math
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from mmd_projection_check import BMP
from mmd_scene_fixture import project


def segment_distance(p, a, b):
    dx, dy = b[0]-a[0], b[1]-a[1]
    t = max(0, min(1, ((p[0]-a[0])*dx+(p[1]-a[1])*dy)/(dx*dx+dy*dy)))
    return math.dist(p, (a[0]+t*dx, a[1]+t*dy))


def inspect(image, polygon):
    edges = list(zip(polygon, polygon[1:]+polygon[:1]))
    inside_count, outside_count, mismatches = 0, 0, 0
    # Fixed region contains the rig at all three ticks and excludes red markers.
    for y in range(180, 320):
        for x in range(280, 520):
            p = (x, y)
            if min(segment_distance(p, a, b) for a, b in edges) <= 1.0:
                continue
            signs = [(b[0]-a[0])*(y-a[1])-(b[1]-a[1])*(x-a[0]) for a, b in edges]
            inside = all(v >= 0 for v in signs) or all(v <= 0 for v in signs)
            r, g, b = image.pixel(x, y)
            red = r-max(g, b) >= 8
            mismatches += red != inside
            inside_count += inside
            outside_count += not inside
    return dict(passed=mismatches == 0 and inside_count > 1000,
                interior_pixels=inside_count, exterior_pixels=outside_count,
                mismatched_pixels=mismatches, edge_exclusion_px=1.0)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('expectations', type=Path)
    parser.add_argument('bitmap_directory', type=Path)
    parser.add_argument('result', type=Path)
    args = parser.parse_args()
    calibration = json.loads(args.expectations.read_text())
    rows = []
    for frame in calibration['rig_frames']:
        tick = frame['frame']
        raw = (args.bitmap_directory / f'native-tick{tick}-20261022.bmp').read_bytes()
        image = BMP(raw)
        projection = 'PERSP' if tick == 0 else 'ORTHO'
        polygon = [project(p, projection) for p in frame['source_world_vertices']]
        row = inspect(image, polygon)
        row.update(frame=tick, polygon=polygon, bitmap_sha256=hashlib.sha256(raw).hexdigest())
        rows.append(row)
        if tick == 2:
            frozen = [project(p, projection) for p in calibration['rig_frames'][0]['source_world_vertices']]
            negative = inspect(image, frozen)
    result = dict(status='passed' if all(r['passed'] for r in rows) and not negative['passed'] else 'failed',
                  rows=rows, frozen_pose_control=negative,
                  scope='Native silhouette/material coverage; precise skin/rotation/morph acceptance uses independent output readback')
    with args.result.open('x', encoding='utf-8') as stream:
        json.dump(result, stream, indent=2)
    print(json.dumps(result))
    return 0 if result['status'] == 'passed' else 1


if __name__ == '__main__':
    sys.exit(main())
