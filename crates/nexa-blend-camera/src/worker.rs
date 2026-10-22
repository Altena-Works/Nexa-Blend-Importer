//! Standalone camera conversion pipeline; no solver or MMD memory access.
use crate::{convert::{ConvertOptions,convert_sample},range::{RangeRequest,import_range},vmd::write_camera_vmd};
use std::{fs::{self,OpenOptions},io::Write,path::PathBuf,time::{SystemTime,UNIX_EPOCH}};
#[derive(Clone,Debug)]
pub struct Options {
    pub blender:PathBuf,pub source:PathBuf,pub output:PathBuf,
    pub scene:Option<String>,pub camera:Option<String>,pub start:Option<f64>,pub end:Option<f64>,
    pub units_per_meter:f64,pub width:u32,pub height:u32,pub allow_fov_approximation:bool,pub overwrite:bool,
}
fn inspect_output(options:&Options)->Result<(),String>{
    match fs::symlink_metadata(&options.output) {
        Ok(m) if m.file_type().is_symlink() || !m.is_file()=>Err("output must be a regular file, not a symlink".into()),
        Ok(_) if !options.overwrite=>Err("output exists; choose a new name or explicitly enable overwrite".into()),
        Ok(_)=>Ok(()),Err(e) if e.kind()==std::io::ErrorKind::NotFound=>Ok(()),Err(e)=>Err(e.to_string()),
    }
}
pub fn convert(options:&Options)->Result<String,String>{
    if !options.blender.is_file() || !options.source.is_file(){return Err("Blender and input .blend must be existing files".into());}
    if !options.source.extension().and_then(|s|s.to_str()).is_some_and(|s|s.eq_ignore_ascii_case("blend")){
        return Err("input must have a .blend extension".into());
    }
    if !options.output.extension().and_then(|s|s.to_str()).is_some_and(|s|s.eq_ignore_ascii_case("vmd")){
        return Err("output must have a .vmd extension".into());
    }
    if !options.units_per_meter.is_finite() || options.units_per_meter<=0.0
        || !(1..=16384).contains(&options.width)||!(1..=16384).contains(&options.height){
        return Err("positive units and output dimensions in 1..16384 are required".into());
    }
    inspect_output(options)?;
    let nonce=SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e|e.to_string())?.as_nanos();
    let folder=std::env::temp_dir().join(format!("nexa-camera-{}-{nonce}",std::process::id()));
    fs::create_dir(&folder).map_err(|e|e.to_string())?;
    let packet=folder.join("range.nexacam");
    let range=import_range(&options.blender,&options.source,&packet,RangeRequest {
        start:options.start,end:options.end,scene:options.scene.as_deref(),camera:options.camera.as_deref(),
    }).map_err(|e|format!("{e}\nDiagnostic directory: {}",folder.display()))?;
    let mut previous=None;let mut keys=Vec::with_capacity(range.samples.len());
    let mut max_error=[0.0f64;2];let mut approximated=0;let mut cuts=0;
    let unit_scale=range.samples[0].unit_scale;
    for (index,sample) in range.samples.iter().enumerate(){
        if sample.unit_scale!=unit_scale{return Err("animated unit scale is unsupported".into());}
        if index>0&&sample.camera_name!=range.samples[index-1].camera_name{cuts+=1;}
        let c=convert_sample(sample,index as u32,ConvertOptions {units_per_meter:options.units_per_meter,
            viewport:[options.width,options.height],allow_fov_approximation:options.allow_fov_approximation},previous)
            .map_err(|e|format!("output frame {index}, camera {:?}: {e}",sample.camera_name))?;
        previous=Some(c.rotation);keys.push(c.key);
        if c.approximated {approximated+=1;}
        for (i,error) in c.edge_error_px.into_iter().enumerate(){max_error[i]=max_error[i].max(error);}
    }
    // Validate/serialize before creating an output file.
    let mut data=Vec::new();write_camera_vmd(&mut data,&keys).map_err(|e|e.to_string())?;
    let parent=options.output.parent().filter(|p|!p.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));
    let temporary=parent.join(format!(".nexa-camera-{}-{nonce}.tmp",std::process::id()));
    let mut file=OpenOptions::new().write(true).create_new(true).open(&temporary).map_err(|e|e.to_string())?;
    file.write_all(&data).and_then(|_|file.sync_all()).map_err(|e|format!("write failed; temporary output retained at {}: {e}",temporary.display()))?;
    drop(file);
    inspect_output(options)?;
    #[cfg(windows)] {
        use std::os::windows::ffi::OsStrExt;
        #[link(name="kernel32")] unsafe extern "system" {fn MoveFileExW(a:*const u16,b:*const u16,flags:u32)->i32;}
        let a:Vec<_>=temporary.as_os_str().encode_wide().chain(Some(0)).collect();
        let b:Vec<_>=options.output.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe{MoveFileExW(a.as_ptr(),b.as_ptr(),8|if options.overwrite{1}else{0})}==0{return Err(format!("publish output: {}",std::io::Error::last_os_error()));}
    }
    #[cfg(not(windows))] {
        if !options.overwrite {fs::hard_link(&temporary,&options.output).map_err(|e|e.to_string())?;fs::remove_file(&temporary).map_err(|e|e.to_string())?;}
        else {fs::rename(&temporary,&options.output).map_err(|e|e.to_string())?;}
    }
    let first=&range.samples[0];let last=range.samples.last().unwrap();
    let source_duration=((last.frame-first.frame) as f64+last.subframe-first.subframe)/range.input_fps;
    let extension=(keys.len()-1) as f64/30.0-source_duration;
    Ok(format!("Camera VMD: {}\n{} keys at 30 fps; {} active-camera cuts\nMMD output: {} x {} pixels\nMMD units / metre: {}\nInteger-FOV approximation: {} keys, maximum edge error [{:.4}, {:.4}] pixels\nEndpoint extension: {:.6} seconds\nClipping planes are not stored in VMD. Camera cuts are sampled at integer MMD frames; subframe discontinuity is not guaranteed.\nSelect camera/light mode in MMD, then load this VMD using the standard motion loader.\nDiagnostic directory: {}",
        options.output.display(),keys.len(),cuts,options.width,options.height,options.units_per_meter,
        approximated,max_error[0],max_error[1],extension,folder.display()))
}
