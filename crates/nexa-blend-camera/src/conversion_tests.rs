use super::*;
use crate::convert::{ConvertOptions,convert_sample};
use crate::vmd::write_camera_vmd;
fn apply(v:[f64;3], a:[f64;3])->[f64;3] {
    let (s,c)=(-a[2]).sin_cos(); let v=[c*v[0]-s*v[1],s*v[0]+c*v[1],v[2]];
    let (s,c)=(-a[0]).sin_cos(); let v=[v[0],c*v[1]-s*v[2],s*v[1]+c*v[2]];
    let (s,c)=(-a[1]).sin_cos(); [c*v[0]+s*v[2],v[1],-s*v[0]+c*v[2]]
}
fn fixture(a:[f64;3],ortho:bool,fov:f64)->CameraSample {
    let mut world=[[0.0;4];4]; world[3][3]=1.0;
    for j in 0..3 {
        let mut axis=[0.0;3];axis[j]=1.0;
        let r=apply(axis,a);
        for (i,k) in [0,2,1].into_iter().enumerate() {world[i][j]=r[k]*if j==2 {-1.0}else{1.0};}
    }
    world[0][3]=1.25;world[1][3]=-2.0;world[2][3]=3.5;
    let fy=if ortho {0.25}else{1.0/(fov.to_radians()/2.0).tan()};
    let mut projection=[[0.0;4];4];
    projection[0][0]=fy/(16.0/9.0);projection[1][1]=fy;
    projection[2][2]=-1.0002;projection[2][3]=-0.20002;
    if ortho {projection[3][3]=1.0;}else{projection[3][2]=-1.0;}
    CameraSample {
        blender_version:[4,5,4],scene_name:"Scene".into(),view_layer_name:"Layer".into(),camera_name:"Camera".into(),
        frame:1,subframe:0.0,evaluated_frame:1.0,frame_start:1,frame_end:3,
        fps:24,fps_base:1.0,frame_map_old:100,frame_map_new:100,
        kind:if ortho {CameraKind::Orthographic}else{CameraKind::Perspective},
        sensor_fit:SensorFit::Auto,lens_mm:50.0,sensor_width_mm:36.0,sensor_height_mm:24.0,
        shift_x:0.0,shift_y:0.0,ortho_scale:8.0,clip_start:0.1,clip_end:1000.0,
        unit_scale:1.0,resolution:[1920,1080],resolution_percentage:100,pixel_aspect:[1.0,1.0],
        matrix_world:world,matrix_projection:projection,
    }
}
fn options()->ConvertOptions {ConvertOptions {units_per_meter:12.5,viewport:[1920,1080],allow_fov_approximation:false}}
fn dot(a:[f64;3],b:[f64;3])->f64 {a.into_iter().zip(b).map(|(a,b)|a*b).sum()}
fn verify_projection(s:&CameraSample,key:crate::vmd::CameraKey) {
    let angles=key.rotation.map(f64::from);
    let right=apply([1.0,0.0,0.0],angles); let up=apply([0.0,1.0,0.0],angles);let forward=apply([0.0,0.0,1.0],angles);
    let eye:[f64;3]=std::array::from_fn(|i|key.center[i] as f64+key.distance as f64*forward[i]);
    let original_eye=[12.5*s.matrix_world[0][3],12.5*s.matrix_world[2][3],12.5*s.matrix_world[1][3]];
    let map=[0,2,1];
    let old_right:[f64;3]=std::array::from_fn(|i|s.matrix_world[map[i]][0]);
    let old_up:[f64;3]=std::array::from_fn(|i|s.matrix_world[map[i]][1]);
    let old_forward:[f64;3]=std::array::from_fn(|i|-s.matrix_world[map[i]][2]);
    for [x,y,z] in [[0.75,-0.3,3.0],[-1.2,0.7,8.0],[0.4,1.1,15.0],[-0.2,-0.4,30.0]] {
        let offset:[f64;3]=std::array::from_fn(|i|12.5*(x*old_right[i]+y*old_up[i]+z*old_forward[i]));
        let delta=std::array::from_fn(|i|original_eye[i]+offset[i]-eye[i]);
        let (px,py)=if key.perspective {
            let fy=1.0/(f64::from(key.fov_deg).to_radians()/2.0).tan();
            (fy/(16.0/9.0)*dot(right,delta)/dot(forward,delta),fy*dot(up,delta)/dot(forward,delta))
        }else {
            // Native MMD orthographic distance specifies the viewport width.
            let width=f64::from(key.distance).abs();
            let height=width/(16.0/9.0);
            (2.0/width*dot(right,delta),2.0/height*dot(up,delta))
        };
        let (expected_x,expected_y)=if key.perspective {
            (s.matrix_projection[0][0]*x/z,s.matrix_projection[1][1]*y/z)
        }else{(s.matrix_projection[0][0]*x,s.matrix_projection[1][1]*y)};
        assert!((px-expected_x).abs()*960.0<0.5,"horizontal pixel error");
        assert!((py-expected_y).abs()*540.0<0.5,"vertical pixel error");
    }
}
#[test]
fn v142_geometry_and_independent_vmd_readback() {
    let mut previous=None;
    for a in [[0.0,0.0,0.0],[0.3,-0.9,0.7],[1.5707963267948966,0.4,-0.8],[-1.5707963267948966,-0.7,0.6],[0.2,3.13,0.4],[0.2,-3.13,0.4]] {
        for ortho in [false,true] {
            let s=fixture(a,ortho,40.0);
            let c=convert_sample(&s,0,options(),previous).unwrap();
            verify_projection(&s,c.key);
            previous=Some(c.rotation);
            let mut bytes=Vec::new();write_camera_vmd(&mut bytes,&[c.key]).unwrap();
            assert_eq!(bytes.len(),135);assert_eq!(&bytes[..25],b"Vocaloid Motion Data 0002");
            assert_eq!(&bytes[30..42],&[0x83,0x4a,0x83,0x81,0x83,0x89,0x81,0x45,0x8f,0xc6,0x96,0xbe]);
            let u=|p|u32::from_le_bytes(bytes[p..p+4].try_into().unwrap());
            let f=|p|f32::from_le_bytes(bytes[p..p+4].try_into().unwrap());
            assert_eq!((u(50),u(54),u(58)),(0,0,1));
            assert_eq!(u(62),0);
            let key=crate::vmd::CameraKey {frame:u(62),distance:f(66),center:[f(70),f(74),f(78)],
                rotation:[f(82),f(86),f(90)],fov_deg:u(118),perspective:bytes[122]==0};
            assert_eq!(bytes[122],u8::from(ortho));
            for block in bytes[94..118].chunks_exact(4) {assert_eq!(block,&[20,107,20,107]);}
            assert_eq!((u(123),u(127),u(131)),(0,0,0));
            verify_projection(&s,key);
        }
    }
}
#[test]
fn v142_conversion_limits_and_quantization() {
    let s=fixture([0.3,-0.4,0.7],false,40.35);
    assert!(convert_sample(&s,0,options(),None).is_err());
    let mut opt=options();opt.allow_fov_approximation=true;
    let c=convert_sample(&s,0,opt,None).unwrap();
    assert!(c.approximated);assert_eq!(c.key.fov_deg,40);
    assert!((c.exact_fov_deg-40.35).abs()<1e-8);assert!(c.edge_error_px[0]>1.0);
    let s=fixture([0.0;3],false,40.0);
    for which in 0..6 {
        let mut bad=s.clone();let mut opt=options();
        match which {
            0=>bad.shift_x=0.1,1=>bad.matrix_world[0][0]=2.0,
            2=>bad.matrix_world[0][1]=0.2,3=>bad.matrix_world[0][0]=-1.0,
            4=>opt.viewport=[100,100],_=>bad.matrix_projection[0][0]=f64::NAN,
        }
        assert!(convert_sample(&bad,0,opt,None).is_err());
    }
    let key=convert_sample(&s,2,options(),None).unwrap().key;
    let mut bytes=Vec::new();
    assert!(write_camera_vmd(&mut bytes,&[key,key]).is_err());assert!(bytes.is_empty());
}
#[test]
fn v142_range_container_boundaries() {
    // Reuse the pre-existing independently authored NEXACAM1 golden fixture.
    let (packet,_,_)=crate::tests::known_packet();
    let mut data=b"NEXARAN1".to_vec();
    data.extend_from_slice(&2u32.to_le_bytes());
    for _ in 0..2 {data.extend_from_slice(&(packet.len() as u32).to_le_bytes());data.extend_from_slice(&packet);}
    let decoded=crate::range::decode_range(&data).unwrap();assert_eq!(decoded.samples.len(),2);
    for end in 0..data.len() {assert!(crate::range::decode_range(&data[..end]).is_err());}
    let mut trailing=data.clone();trailing.push(0);assert!(crate::range::decode_range(&trailing).is_err());
    for count in [0u32,108002] {
        let mut bad=data.clone();bad[8..12].copy_from_slice(&count.to_le_bytes());
        assert!(crate::range::decode_range(&bad).is_err());
    }
}
