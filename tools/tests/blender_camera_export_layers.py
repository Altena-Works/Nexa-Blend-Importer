"""Run with Blender background/factory startup and automatic scripts disabled."""
from contextlib import contextmanager
from pathlib import Path
import runpy
import struct
import unittest

import bpy


PRODUCER = Path(__file__).resolve().parents[1] / 'blender_camera_export.py'
CAMERA_PACKET = runpy.run_path(
    str(PRODUCER), run_name='nexa_camera_layer_tests'
)['camera_packet']
EXPECTED_ERROR = 'Selected camera is not evaluated in the selected view layer'


def position(matrix):
    return [float(matrix[axis][3]) for axis in range(3)]


class CameraLayerTests(unittest.TestCase):
    def new(self, kind, *args):
        value = getattr(bpy.data, kind).new(*args)
        self.owned.append((kind, value))
        return value

    def cleanup_owned(self):
        for kind, value in reversed(self.owned):
            getattr(bpy.data, kind).remove(value, do_unlink=True)

    def setUp(self):
        self.owned = []
        self.addCleanup(self.cleanup_owned)
        self.default_scene = bpy.context.scene
        self.default_layer = bpy.context.view_layer

        self.scene = self.new('scenes', 'NexaCameraLayerTest')
        self.scene.frame_start = 1
        self.scene.frame_end = 9
        self.first = self.scene.view_layers[0]
        self.first.name = 'Selected'
        self.alternate = self.scene.view_layers.new('Available')

        self.collection = self.new('collections', 'NexaTestCameraCollection')
        self.scene.collection.children.link(self.collection)

        self.parent = self.new('objects', 'NexaTestParent', None)
        self.scene.collection.objects.link(self.parent)
        camera_data = self.new('cameras', 'NexaTestCameraData')
        self.camera = self.new('objects', 'NexaTestCamera', camera_data)
        self.collection.objects.link(self.camera)
        self.camera.parent = self.parent
        self.camera.location = (1.0, 2.0, 3.0)

        # The explicitly selected camera is not the scene's active camera.
        active_data = self.new('cameras', 'NexaTestActiveData')
        active = self.new('objects', 'NexaTestActive', active_data)
        self.scene.collection.objects.link(active)
        self.scene.camera = active

        self.parent.location.x = 0.0
        self.parent.keyframe_insert(data_path='location', index=0, frame=1)
        self.owned.append(('actions', self.parent.animation_data.action))
        self.parent.location.x = 8.0
        self.parent.keyframe_insert(data_path='location', index=0, frame=9)

        with bpy.context.temp_override(scene=self.scene, view_layer=self.first):
            self.scene.frame_set(1, subframe=0.0)
            graph = bpy.context.evaluated_depsgraph_get()
            self.assertEqual(
                position(self.camera.evaluated_get(graph).matrix_world),
                [1.0, 2.0, 3.0],
            )

    @contextmanager
    def frame_nine(self, current, excluded):
        self.first.layer_collection.children[self.collection.name].exclude = excluded
        outer_scene, outer_layer = (
            (self.scene, self.first)
            if current else (self.default_scene, self.default_layer)
        )
        with bpy.context.temp_override(scene=outer_scene, view_layer=outer_layer):
            chosen = (
                bpy.context.view_layer
                if self.scene == bpy.context.scene
                else self.scene.view_layers[0]
            )
            with bpy.context.temp_override(scene=self.scene, view_layer=chosen):
                self.scene.frame_set(9, subframe=0.0)
                yield chosen, bpy.context.evaluated_depsgraph_get()

    def test_1_included_parent_motion(self):
        with self.frame_nine(current=True, excluded=False) as (layer, graph):
            packet = CAMERA_PACKET(self.scene, layer, self.camera, graph)
            self.assertEqual(packet[:8], b'NEXACAM1')
            world = struct.unpack('<16d', packet[-256:-128])
            self.assertEqual([world[3], world[7], world[11]], [9.0, 2.0, 3.0])
            self.assertEqual(
                position(self.parent.evaluated_get(graph).matrix_world),
                [8.0, 0.0, 0.0],
            )

    def assert_excluded_rejected(self, current):
        with self.frame_nine(current=current, excluded=True) as (layer, graph):
            # Returning any packet makes this assertion fail.
            with self.assertRaises(ValueError) as caught:
                CAMERA_PACKET(self.scene, layer, self.camera, graph)
            self.assertEqual(str(caught.exception), EXPECTED_ERROR)
            self.assertIsNone(layer.objects.get(self.camera.name))
            self.assertEqual(
                self.alternate.objects.get(self.camera.name), self.camera
            )
            self.assertEqual(
                position(self.parent.evaluated_get(graph).matrix_world),
                [8.0, 0.0, 0.0],
            )

    def test_2_excluded_current_layer(self):
        self.assert_excluded_rejected(current=True)

    def test_3_excluded_noncurrent_first_layer(self):
        self.assert_excluded_rejected(current=False)


if __name__ == '__main__':
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(CameraLayerTests)
    result = unittest.TextTestRunner(verbosity=2, failfast=True).run(suite)
    if not result.wasSuccessful():
        raise RuntimeError('Blender camera layer tests failed')
