//! Blender evaluated camera to MMD/VMD coordinates (column-vector convention).
use crate::{CameraKind, CameraSample};
use crate::vmd::CameraKey;

#[derive(Clone, Copy, Debug)]
pub struct ConvertOptions {
    /// MMD coordinate units per physical metre; Blender unit_scale is applied once.
    pub units_per_meter: f64,
    pub viewport: [u32; 2],
    pub allow_fov_approximation: bool,
}
#[derive(Clone, Debug)]
pub struct Conversion {
    pub key: CameraKey,
    pub eye: [f64; 3],
    pub rotation: [f64; 3],
    pub exact_fov_deg: f64,
    pub edge_error_px: [f64; 2],
    pub approximated: bool,
    pub required_aspect: f64,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ConvertError(pub &'static str);
impl std::fmt::Display for ConvertError {
    fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {f.write_str(self.0)}
}
impl std::error::Error for ConvertError {}
fn fail<T>(s:&'static str)->Result<T,ConvertError>{Err(ConvertError(s))}
fn dot(a:[f64;3],b:[f64;3])->f64 {a.iter().zip(b).map(|(x,y)|x*y).sum()}
fn cross(a:[f64;3],b:[f64;3])->[f64;3] {
    [a[1]*b[2]-a[2]*b[1],a[2]*b[0]-a[0]*b[2],a[0]*b[1]-a[1]*b[0]]
}
fn normalize(a:[f64;3])->[f64;3] { let n=dot(a,a).sqrt(); a.map(|x|x/n) }
fn near(a:f64,b:f64)->bool {(a-b).abs()<=1e-5}
fn unwrap(value:f64, previous:f64)->f64 {
    value + ((previous-value)/std::f64::consts::TAU).round()*std::f64::consts::TAU
}
fn angles(r:[[f64;3];3], previous:Option<[f64;3]>)->[f64;3] {
    let c=r[1][0].hypot(r[1][1]);
    let mut result=if c<=1e-10 {
        let x=std::f64::consts::FRAC_PI_2.copysign(r[1][2]);
        let z=previous.map_or(0.0,|p|p[2]);
        [x,r[2][0].atan2(r[0][0])-r[1][2].signum()*z,z]
    } else {
        [r[1][2].atan2(c),(-r[0][2]).atan2(r[2][2]),(-r[1][0]).atan2(r[1][1])]
    };
    if let Some(previous)=previous {
        result=std::array::from_fn(|i|unwrap(result[i],previous[i]));
        let alternate=[std::f64::consts::PI-result[0],result[1]+std::f64::consts::PI,result[2]+std::f64::consts::PI];
        let alternate=std::array::from_fn(|i|unwrap(alternate[i],previous[i]));
        let distance=|a:[f64;3]| (0..3).map(|i|(a[i]-previous[i]).powi(2)).sum::<f64>();
        if c>1e-10 && distance(alternate)<distance(result) {result=alternate;}
    }
    result
}
fn f32_checked(value:f64)->Result<f32,ConvertError> {
    let result=value as f32;
    if !result.is_finite() {return fail("camera value cannot be represented as VMD f32");}
    Ok(result)
}
pub fn convert_sample(sample:&CameraSample, frame:u32, options:ConvertOptions,
    previous:Option<[f64;3]>) -> Result<Conversion,ConvertError>
{
    let k=options.units_per_meter*sample.unit_scale;
    if !k.is_finite() || k<=0.0 || options.viewport.contains(&0) {
        return fail("positive finite units and nonzero viewport are required");
    }
    if previous.is_some_and(|p|p.iter().any(|v|!v.is_finite())) {return fail("invalid previous rotation");}
    if sample.matrix_world.iter().flatten().chain(sample.matrix_projection.iter().flatten()).any(|v|!v.is_finite()) {
        return fail("non-finite camera matrix");
    }
    let w=sample.matrix_world;
    if !(0..4).all(|i|near(w[3][i],if i==3 {1.0}else{0.0})) {return fail("non-affine camera matrix");}
    let axes:[ [f64;3];3]=std::array::from_fn(|j|std::array::from_fn(|i|w[i][j]));
    if !(0..3).all(|i|near(dot(axes[i],axes[i]),1.0))
        || !(0..3).all(|i|(0..i).all(|j|near(dot(axes[i],axes[j]),0.0)))
        || !near(dot(cross(axes[0],axes[1]),axes[2]),1.0) {
        return fail("scaled, sheared or reflected camera transforms are unsupported");
    }
    // Remove only the already bounded floating-point orthogonality noise.
    let right=normalize(axes[0]);
    let up=normalize(std::array::from_fn(|i|axes[1][i]-dot(right,axes[1])*right[i]));
    let back=cross(right,up);
    let b=[right,up,back];
    let map=[0,2,1];
    let r:[[f64;3];3]=std::array::from_fn(|i|std::array::from_fn(|j|b[j][map[i]]*if j==2 {-1.0}else{1.0}));
    let eye=[k*w[0][3],k*w[2][3],k*w[1][3]];
    if sample.shift_x!=0.0 || sample.shift_y!=0.0 {return fail("lens shift/off-axis projection cannot be represented by VMD");}
    let p=sample.matrix_projection;
    let perspective=sample.kind==CameraKind::Perspective;
    for (i,row) in p.iter().enumerate() {
        for (j,value) in row.iter().enumerate() {
            let allowed=matches!((i,j),(0,0)|(1,1)|(2,2)|(2,3))
                || if perspective {(i,j)==(3,2)}else{(i,j)==(3,3)};
            if !allowed && value.abs()>1e-6 {return fail("unsupported off-axis camera projection");}
        }
    }
    if p[0][0]<=0.0 || p[1][1]<=0.0 || p[2][2]>=0.0
        || if perspective {!near(p[3][2],-1.0)}else{!near(p[3][3],1.0)} {
        return fail("unsupported camera projection matrix");
    }
    let aspect=p[1][1]/p[0][0];
    let output_aspect=options.viewport[0] as f64/options.viewport[1] as f64;
    if !aspect.is_finite() || (aspect/output_aspect-1.0).abs()>1e-5 {
        return fail("output width/height does not match the evaluated Blender camera aspect");
    }
    let rotation=angles(r,previous);
    let exact_fov_deg=if perspective {2.0*(1.0/p[1][1]).atan().to_degrees()}else{30.0};
    let fov=exact_fov_deg.round();
    if !(1.0..=135.0).contains(&fov) {return fail("supported MMD vertical FOV is 1..135 degrees");}
    let approximated=perspective && (fov-exact_fov_deg).abs()>1e-4;
    if approximated && !options.allow_fov_approximation {
        return fail("noninteger VMD FOV requires explicit approximation permission");
    }
    // Backported from Nexa Blend Camera v1.0.0: MMD orthographic distance uses width.
    let distance=if perspective {0.0}else{-k*2.0/p[0][0]};
    let center: [f64;3]=std::array::from_fn(|i|eye[i]-distance*r[i][2]);
    let written_fy=1.0/(fov.to_radians()/2.0).tan();
    let edge_error_px=if perspective {[
        options.viewport[0] as f64/2.0*((written_fy/output_aspect)/p[0][0]-1.0).abs(),
        options.viewport[1] as f64/2.0*(written_fy/p[1][1]-1.0).abs(),
    ]}else{[0.0;2]};
    let key=CameraKey {
        frame,distance:f32_checked(distance)?,
        center:[f32_checked(center[0])?,f32_checked(center[1])?,f32_checked(center[2])?],
        rotation:[f32_checked(rotation[0])?,f32_checked(rotation[1])?,f32_checked(rotation[2])?],
        fov_deg:fov as u32,perspective,
    };
    Ok(Conversion {key,eye,rotation,exact_fov_deg,edge_error_px,approximated,required_aspect:aspect})
}
