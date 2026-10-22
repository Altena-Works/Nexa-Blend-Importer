//! Bounded discovery of installed Blender paths; never executes candidates.
use std::{ffi::OsString,path::{Path,PathBuf},os::windows::ffi::{OsStrExt,OsStringExt}};
type H=*mut std::ffi::c_void;
#[derive(Clone,Debug)]
pub struct Candidate {pub path:PathBuf,pub version:Vec<u32>}
impl Candidate {
    pub fn label(&self)->String {
        let version=self.version.iter().map(u32::to_string).collect::<Vec<_>>().join(".");
        format!("Blender {} — {}",if version.is_empty(){"(version unknown)"}else{&version},self.path.display())
    }
}
#[link(name="advapi32")] unsafe extern "system" {
    fn RegOpenKeyExW(h:H,name:*const u16,options:u32,access:u32,out:*mut H)->i32;
    fn RegCloseKey(h:H)->i32;
    fn RegQueryValueExW(h:H,name:*const u16,reserved:*mut u32,kind:*mut u32,data:*mut u8,size:*mut u32)->i32;
    fn RegEnumKeyExW(h:H,index:u32,name:*mut u16,size:*mut u32,reserved:*mut u32,class:*mut u16,class_size:*mut u32,time:*mut std::ffi::c_void)->i32;
}
#[link(name="version")] unsafe extern "system" {
    fn GetFileVersionInfoSizeW(path:*const u16,handle:*mut u32)->u32;
    fn GetFileVersionInfoW(path:*const u16,handle:u32,size:u32,data:*mut std::ffi::c_void)->i32;
    fn VerQueryValueW(data:*const std::ffi::c_void,key:*const u16,value:*mut *mut std::ffi::c_void,size:*mut u32)->i32;
}
fn wide(s:&std::ffi::OsStr)->Vec<u16>{s.encode_wide().chain(Some(0)).collect()}
struct Key(H);
impl Drop for Key {fn drop(&mut self){unsafe{RegCloseKey(self.0);}}}
fn open(root:H,name:&str,view:u32)->Option<Key>{
    let mut h=std::ptr::null_mut();
    if unsafe{RegOpenKeyExW(root,wide(name.as_ref()).as_ptr(),0,0x20019|view,&mut h)}==0{Some(Key(h))}else{None}
}
fn value(h:H,name:&str)->Option<String>{
    let mut b=vec![0u16;16384];let mut bytes=(b.len()*2) as u32;let mut kind=0;
    if unsafe{RegQueryValueExW(h,wide(name.as_ref()).as_ptr(),std::ptr::null_mut(),&mut kind,b.as_mut_ptr().cast(),&mut bytes)}!=0
        ||kind!=1||bytes%2!=0||bytes<2{return None;}
    let n=bytes as usize/2;
    if b[n-1]!=0{return None;}
    String::from_utf16(&b[..n-1]).ok().filter(|s|!s.contains('\0'))
}
fn registered()->Vec<(PathBuf,Vec<u32>)>{
    let mut out=Vec::new();
    for root in [0x80000001u32,0x80000002u32]{for view in [0x100,0x200]{
        let root=root as i32 as isize as H;
        if let Some(k)=open(root,"SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths\\blender.exe",view){
            if let Some(path)=value(k.0,""){out.push((PathBuf::from(path.trim_matches('"')),Vec::new()));}
        }
        if let Some(k)=open(root,"SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",view){
            for index in 0..2048 {
                let mut name=[0u16;256];let mut n=name.len() as u32;
                let status=unsafe{RegEnumKeyExW(k.0,index,name.as_mut_ptr(),&mut n,std::ptr::null_mut(),std::ptr::null_mut(),std::ptr::null_mut(),std::ptr::null_mut())};
                if status==259{break;}if status!=0{continue;}
                let Ok(name)=String::from_utf16(&name[..n as usize]) else{continue;};
                let Some(app)=open(k.0,&name,0) else{continue;};
                let Some(name)=value(app.0,"DisplayName") else{continue;};
                if name!="Blender"&&!name.starts_with("Blender "){continue;}
                let version=parse_version(&value(app.0,"DisplayVersion").unwrap_or_default());
                if let Some(path)=value(app.0,"InstallLocation"){out.push((PathBuf::from(path.trim_matches('"')).join("blender.exe"),version.clone()));}
                if let Some(icon)=value(app.0,"DisplayIcon"){
                    let path=if let Some(s)=icon.strip_prefix('"'){s.split('"').next().unwrap_or("")}else{icon.rsplit_once(',').filter(|(_,n)|n.trim().parse::<i32>().is_ok()).map_or(icon.as_str(),|(p,_)|p)};
                    out.push((PathBuf::from(path),version));
                }
            }
        }
    }}out
}
fn parse_version(s:&str)->Vec<u32>{
    s.trim_start_matches("Blender ").split('.').take(4).map_while(|v|v.parse().ok()).collect()
}
fn file_version(path:&Path)->Option<Vec<u32>>{
    let path=wide(path.as_os_str());let mut unused=0;
    let size=unsafe{GetFileVersionInfoSizeW(path.as_ptr(),&mut unused)};
    if size==0||size>1_048_576{return None;}
    // u32 storage also provides alignment for VS_FIXEDFILEINFO.
    let mut data=vec![0u32;(size as usize).div_ceil(4)];let mut p=std::ptr::null_mut();let mut n=0;
    if unsafe{GetFileVersionInfoW(path.as_ptr(),0,size,data.as_mut_ptr().cast())}==0
        ||unsafe{VerQueryValueW(data.as_ptr().cast(),wide("\\".as_ref()).as_ptr(),&mut p,&mut n)}==0||n<52||p.is_null(){return None;}
    let v=unsafe{std::slice::from_raw_parts(p.cast::<u32>(),13)};
    if v[0]!=0xfeef04bd{return None;}
    let mut version=vec![v[2]>>16,v[2]&0xffff,v[3]>>16,v[3]&0xffff];
    while version.len()>2&&version.last()==Some(&0){version.pop();}Some(version)
}
fn collect(roots:&[PathBuf],mut paths:Vec<(PathBuf,Vec<u32>)>)->Vec<Candidate>{
    for root in roots {
        let Ok(entries)=std::fs::read_dir(root) else{continue;};
        for e in entries.take(128).flatten(){
            let name=e.file_name();let name=name.to_string_lossy();
            if name=="Blender"||name.starts_with("Blender "){paths.push((e.path().join("blender.exe"),parse_version(&name)));}
        }
    }
    let mut out:Vec<Candidate>=Vec::new();
    for (path,fallback) in paths{
        if !path.is_absolute()||!path.is_file()||!path.file_name().is_some_and(|s|s.eq_ignore_ascii_case("blender.exe")){continue;}
        let Ok(path)=std::fs::canonicalize(path) else{continue;};
        let units:Vec<u16>=path.as_os_str().encode_wide().collect();
        let path=if units.starts_with(&[92,92,63,92])&&units.get(5)==Some(&58){PathBuf::from(OsString::from_wide(&units[4..]))}else{path};
        let version=file_version(&path).unwrap_or(fallback);
        if let Some(existing)=out.iter_mut().find(|c|c.path.as_os_str().to_string_lossy().eq_ignore_ascii_case(&path.as_os_str().to_string_lossy())){
            if version>existing.version{existing.version=version;}continue;
        }
        out.push(Candidate{path,version});
    }
    out.sort_by(|a,b|b.version.cmp(&a.version).then_with(||a.path.cmp(&b.path)));out
}
pub fn detect()->Vec<Candidate>{
    let roots:Vec<_>=["ProgramW6432","ProgramFiles","ProgramFiles(x86)"].iter().filter_map(std::env::var_os)
        .map(|p:OsString|PathBuf::from(p).join("Blender Foundation")).collect();
    collect(&roots,registered())
}

#[cfg(test)] mod tests{
    use super::*;
    #[test] fn bounded_roots_find_versions_sort_numerically_and_deduplicate(){
        let root=std::env::temp_dir().join(format!("nexa-blender-detect-{}",std::process::id()));
        std::fs::create_dir(&root).unwrap();
        for name in ["Blender 4.9","Blender 4.10","Unrelated"]{std::fs::create_dir(root.join(name)).unwrap();std::fs::write(root.join(name).join("blender.exe"),[]).unwrap();}
        let found=collect(std::slice::from_ref(&root),vec![(root.join("Blender 4.9/blender.exe"),vec![4,9])]);
        assert_eq!(found.len(),2);assert_eq!(found[0].version,vec![4,10]);assert!(found[0].label().contains("Blender 4.10"));
        for name in ["Blender 4.9","Blender 4.10","Unrelated"]{std::fs::remove_file(root.join(name).join("blender.exe")).unwrap();std::fs::remove_dir(root.join(name)).unwrap();}
        std::fs::remove_dir(root).unwrap();
    }
    #[test] fn missing_and_wrong_executable_leave_no_default(){
        assert!(collect(&[PathBuf::from("Z:/missing-nexa-blender-root")],vec![(PathBuf::from("relative/blender.exe"),vec![]),(PathBuf::from("C:/Windows/notepad.exe"),vec![])]).is_empty());
    }
    #[test] fn version_parsing_preserves_numeric_order(){assert_eq!(parse_version("Blender 4.10"),vec![4,10]);assert_eq!(parse_version("4.5.4"),vec![4,5,4]);assert!(parse_version("unknown").is_empty());}
}
