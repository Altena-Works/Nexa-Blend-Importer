use nexa_blend_importer::{pipeline, source, FrameRange, ImportRequest, Language, Selection};
use std::{path::PathBuf, process::ExitCode, sync::atomic::AtomicBool};

fn selection(text:&str)->Result<Selection,String> {
    let mut s=Selection{model:false,camera:false,animation:false};
    for part in text.split(',') { match part {
        "model" if !s.model=>s.model=true,"camera" if !s.camera=>s.camera=true,
        "animation" if !s.animation=>s.animation=true,_=>return Err("invalid or duplicate component".into())
    }}s.validate().map_err(|e|e.to_string())?;Ok(s)
}
fn number(value:&str)->Result<f64,String> {
    value.parse::<f64>().ok().filter(|n|n.is_finite()).ok_or_else(||"expected a finite number".into())
}
fn parse(args:&[String])->Result<(pipeline::Options,Vec<PathBuf>),String> {
    let mut input=None;let mut blender=None;let mut output=None;let mut scene=None;
    let mut request=ImportRequest::default();let mut start=None;let mut end=None;let mut approximate=false;
    let mut targets=Vec::new();let mut used=std::collections::BTreeSet::new();let mut index=0;
    while index<args.len(){let key=args[index].as_str();
        if key!="--target-pmx" && !used.insert(key){return Err(format!("duplicate option: {key}"));}
        if key=="--allow-fov-approximation"{approximate=true;index+=1;continue;}
        index+=1;let value=args.get(index).ok_or_else(||format!("missing value: {key}"))?;
        match key {
            "--import"=>input=Some(PathBuf::from(value)),"--blender"=>blender=Some(PathBuf::from(value)),
            "--output-parent"=>output=Some(PathBuf::from(value)),"--scene"=>scene=Some(value.clone()),
            "--selection"=>request.selection=selection(value)?,"--reference-frame"=>request.reference_frame=Some(number(value)?),
            "--start"=>start=Some(number(value)?),"--end"=>end=Some(number(value)?),"--units"=>request.units_per_meter=number(value)?,
            "--target-pmx"=>targets.push(PathBuf::from(value)),"--language"=>{Language::parse(value).ok_or("language must be ja or en")?;},
            _=>return Err(format!("unknown option: {key}")),
        }index+=1;
    }
    request.frame_range=match(start,end){(None,None)=>None,(Some(start),Some(end))if start<=end=>Some(FrameRange{start,end}),_=>return Err("specify both --start and --end in order".into())};
    let input=input.ok_or("--import is required")?;
    let input=std::fs::canonicalize(input).map_err(|e|format!("input: {e}"))?;
    let parent=input.parent().ok_or("input has no parent")?.to_path_buf();
    let output=std::fs::canonicalize(output.unwrap_or(parent)).map_err(|e|format!("output parent: {e}"))?;
    let blender=match blender {
        Some(path)=>path,
        None=>{
            #[cfg(windows)]
            {nexa_blend_importer::camera::blender_detection::detect().first().map(|c|c.path.clone()).ok_or("Blender was not detected; specify --blender")?}
            #[cfg(not(windows))]
            {return Err("specify --blender".into());}
        }
    };
    let blender=std::fs::canonicalize(blender).map_err(|e|format!("Blender: {e}"))?;
    Ok((pipeline::Options{source:source::SourceOptions{blender,input,scratch_parent:output.clone(),scene,request},output_parent:output,allow_fov_approximation:approximate},targets))
}
pub fn run(args:&[String],language:Language)->ExitCode {
    let result=parse(args).and_then(|(options,targets)|pipeline::run(&options,&targets,&AtomicBool::new(false)));
    match result {
        Ok(result)=>{
            println!("{}\n{}",language.choose("取り込み用ファイルを保存しました。","Import files saved."),result.directory.display());
            println!("{}",language.choose("MMDでPMX→対象モデルmotion→cameraの順に標準読込みしてください。","Use MMD's standard loader in PMX → model motion → camera order."));
            for model in result.models{println!("PMX: {}",result.directory.join(model).display());}
            for(target,motion)in result.motions{let target=if target.is_absolute(){target}else{result.directory.join(target)};println!("VMD: {} -> {}",result.directory.join(motion).display(),target.display());}
            if let Some(camera)=result.camera{println!("Camera: {}",result.directory.join(camera).display());}
            for notice in result.notices{println!("{}",notice.text(language));}ExitCode::SUCCESS
        },
        Err(error)=>{eprintln!("{}\n{error}",language.choose("取り込みを完了できませんでした。","Import could not be completed."));ExitCode::FAILURE}
    }
}
