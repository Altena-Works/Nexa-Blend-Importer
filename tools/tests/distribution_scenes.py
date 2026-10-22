"""Release checks beyond the original rig fixture, using real local Blender.

Creates disposable inputs only under a fresh caller-selected trial directory.
No existing input is saved or overwritten; no MMD automation is performed.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys


def generate(directory):
    import bpy
    bpy.ops.wm.read_factory_settings(use_empty=True)
    scene=bpy.context.scene
    scene.render.fps=24
    scene.frame_start=-2
    scene.frame_end=2
    mesh=bpy.data.meshes.new('Concave mesh')
    mesh.from_pydata([(0,0,0),(2,0,0),(2,2,0),(1,1,0),(0,2,0)],[],[(0,1,2,3,4)])
    obj=bpy.data.objects.new('日本語の長いモデル名 / movable',mesh)
    scene.collection.objects.link(obj)
    for frame,x in [(-2,0.0),(2,2.0)]:
        obj.location.x=x
        obj.keyframe_insert('location',frame=frame)
    for curve in obj.animation_data.action.fcurves:
        for key in curve.keyframe_points:key.interpolation='LINEAR'
    scene.frame_set(-2)
    bpy.ops.wm.save_as_mainfile(filepath=str(directory/'動作 24fps.blend'))
    obj.animation_data_clear()
    obj.location.x=0
    second=obj.copy()
    second.data=mesh.copy()
    second.name='second independent model'
    scene.collection.objects.link(second)
    second.location.x=3
    bpy.ops.wm.save_as_mainfile(filepath=str(directory/'静的 2 models.blend'))


def run(root,trial,blender):
    trial.mkdir()
    inputs=trial/'日本語 inputs'
    inputs.mkdir()
    subprocess.run([str(blender),'--background','--factory-startup','--disable-autoexec',
        '--python-exit-code','1','--python',str(Path(__file__).resolve()),'--','--generate',str(inputs)],
        check=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=60)
    worker=root/'target/x86_64-pc-windows-msvc/release/nexa-blend-importer.exe'
    def digest(path):return hashlib.sha256(path.read_bytes()).hexdigest()
    before={str(p):digest(p) for p in inputs.glob('*.blend')}
    reports=[]
    for name,source,selection,success in [('static',inputs/'静的 2 models.blend','model',True),
            ('animated',inputs/'動作 24fps.blend','model,animation',True),
            ('missing-camera',inputs/'静的 2 models.blend','camera',False)]:
        output=trial/name
        output.mkdir()
        sentinel=output/'keep.txt'
        sentinel.write_bytes(b'preexisting data')
        completed=subprocess.run([str(worker),'--import',str(source),'--blender',str(blender),
            '--selection',selection,'--output-parent',str(output),'--units','1',
            *(['--start','-2','--end','2'] if name=='animated' else [])],
            capture_output=True,timeout=620)
        (trial/f'{name}.stderr').write_bytes(completed.stderr)
        assert (completed.returncode==0)==success,(name,completed.returncode,completed.stderr)
        assert sentinel.read_bytes()==b'preexisting data'
        assert all(digest(Path(p))==value for p,value in before.items())
        published=list(output.glob('NexaImport-*'))
        assert len(published)==int(success),(name,published)
        report=dict(case=name,status='passed',expected_success=success)
        if success:
            sys.path.insert(0,str(root/'tools/tests'))
            from importer_output_fixture import read_pmx,read_vmd
            models=[read_pmx(p) for p in published[0].glob('*.pmx')]
            assert len(models)==(2 if name=='static' else 1)
            assert all(m['vertices'] for m in models)
            if name=='animated':
                motion=read_vmd(published[0]/'model_0001.motion.vmd')
                assert not motion['cameras'] and len(motion['bones'])==6
                for (bone,tick),key in motion['bones'].items():
                    expected=2*tick/5
                    assert abs(key['translation'][0]-expected)<1e-5,(bone,tick,key)
            report['models']=len(models)
        else:
            assert list(output.iterdir())==[sentinel]
        reports.append(report)
    result=dict(status='passed',cases=reports,input_hashes=before,input_unchanged=True)
    (trial/'result.json').write_text(json.dumps(result,ensure_ascii=False,indent=2),encoding='utf-8')
    print(json.dumps(result,ensure_ascii=False))


if __name__=='__main__':
    if '--generate' in sys.argv:
        generate(Path(sys.argv[sys.argv.index('--generate')+1]))
    else:
        parser=argparse.ArgumentParser(description=__doc__)
        parser.add_argument('--trial',type=Path,required=True)
        parser.add_argument('--blender',type=Path,required=True)
        args=parser.parse_args()
        run(Path(__file__).resolve().parents[2],args.trial,args.blender)
