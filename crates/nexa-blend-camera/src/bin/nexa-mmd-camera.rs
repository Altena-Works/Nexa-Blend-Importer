//! Independent Blender-to-MMD camera worker and native settings window.
use nexa_blend_camera::worker::{Options,convert};
use std::{ffi::OsStr,path::PathBuf,process::ExitCode};
const HELP:&str="nexa-mmd-camera --blender EXE --output CAMERA.vmd [--scene NAME] [--camera NAME] [--start F] [--end F] [--units-per-meter N] [--width W] [--height H] [--allow-fov-approximation] [--overwrite] INPUT.blend\nnexa-mmd-camera --gui";
fn run()->Result<(),String>{
    let args:Vec<_>=std::env::args_os().skip(1).collect();
    if args.len()==1&&args[0]==OsStr::new("--help"){println!("{HELP}");return Ok(());}
    #[cfg(windows)]
    if args.len()==1&&args[0]==OsStr::new("--detect-blender"){
        for candidate in nexa_blend_camera::blender_detection::detect(){println!("{}",candidate.label());}return Ok(());
    }
    let plugin=args.len()==2&&args[0]==OsStr::new("--gui")&&args[1]==OsStr::new("--plugin-result");
    if (args.len()==1&&args[0]==OsStr::new("--gui"))||plugin{
        #[cfg(windows)] {if let Some(options)=nexa_blend_camera::gui::show()? {
            match convert(&options){Ok(report)=>{
                if plugin {
                    use std::io::Write;
                    let path=options.output.to_str().ok_or("output path must be Unicode")?;
                    if path.len()+9>131_072{return Err("output path is too long for automatic loading".into());}
                    let mut out=std::io::stdout().lock();
                    out.write_all(b"NEXAVMD1\n").and_then(|_|out.write_all(path.as_bytes())).map_err(|e|e.to_string())?;
                }else{nexa_blend_camera::gui::message(&report,false);}
            },Err(e)=>{nexa_blend_camera::gui::message(&e,true);return Err(e);}}
        }else if plugin {use std::io::Write;std::io::stdout().write_all(b"NEXACANCEL1").map_err(|e|e.to_string())?;}
        return Ok(());}
        #[cfg(not(windows))] return Err("native GUI requires Windows".into());
    }
    let mut options=Options {blender:PathBuf::new(),source:PathBuf::new(),output:PathBuf::new(),scene:None,camera:None,
        start:None,end:None,units_per_meter:12.5,width:1920,height:1080,allow_fov_approximation:false,overwrite:false};
    let mut seen=std::collections::BTreeSet::new();let mut i=0;
    while i<args.len(){
        let flag=args[i].to_str().unwrap_or("");
        if flag=="--allow-fov-approximation"||flag=="--overwrite"{
            if !seen.insert(flag.to_owned()){return Err(format!("repeated {flag}"));}
            if flag=="--overwrite"{options.overwrite=true;}else{options.allow_fov_approximation=true;}
        }else if flag.starts_with("--"){
            if !seen.insert(flag.to_owned()){return Err(format!("repeated {flag}"));}
            i+=1;let value=args.get(i).ok_or_else(||format!("missing value for {flag}"))?;
            match flag{
                "--blender"=>options.blender=value.into(),"--output"=>options.output=value.into(),
                "--scene"=>options.scene=Some(value.to_str().ok_or("scene name must be Unicode")?.into()),
                "--camera"=>options.camera=Some(value.to_str().ok_or("camera name must be Unicode")?.into()),
                "--start"|"--end"|"--units-per-meter"=>{
                    let n=value.to_str().ok_or("number must be Unicode")?.parse::<f64>().map_err(|_|format!("invalid {flag}"))?;
                    match flag{"--start"=>options.start=Some(n),"--end"=>options.end=Some(n),_=>options.units_per_meter=n}
                }
                "--width"|"--height"=>{let n=value.to_str().ok_or("dimension must be Unicode")?.parse().map_err(|_|format!("invalid {flag}"))?;
                    if flag=="--width"{options.width=n;}else{options.height=n;}}
                _=>return Err(format!("unknown option {flag}\n{HELP}")),
            }
        }else{if !options.source.as_os_str().is_empty(){return Err("multiple inputs".into());}options.source=PathBuf::from(&args[i]);}
        i+=1;
    }
    if options.blender.as_os_str().is_empty()||options.output.as_os_str().is_empty()||options.source.as_os_str().is_empty(){return Err(HELP.into());}
    println!("{}",convert(&options)?);Ok(())
}
fn main()->ExitCode{match run(){Ok(())=>ExitCode::SUCCESS,Err(e)=>{eprintln!("nexa-mmd-camera: {e}");ExitCode::FAILURE}}}
