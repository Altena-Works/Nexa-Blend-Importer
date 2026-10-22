//! Small native settings window for the independent camera worker.
use crate::worker::Options;
use std::{ffi::{c_void,OsString},mem::{size_of,zeroed},os::windows::ffi::OsStringExt,path::{Path,PathBuf},ptr::{null,null_mut}};
type H=*mut c_void;
type Proc=unsafe extern "system" fn(H,u32,usize,isize)->isize;
#[repr(C)] struct Class {style:u32,proc:Option<Proc>,cls:i32,wnd:i32,instance:H,icon:H,cursor:H,background:H,menu:*const u16,name:*const u16}
#[repr(C)] struct Msg {hwnd:H,message:u32,w:usize,l:isize,time:u32,point:[i32;2],private:u32}
#[repr(C)] struct Rect {left:i32,top:i32,right:i32,bottom:i32}
#[repr(C)] struct OpenFile {
    size:u32,owner:H,instance:H,filter:*const u16,custom:*mut u16,max_custom:u32,index:u32,
    file:*mut u16,max_file:u32,title_file:*mut u16,max_title:u32,initial:*const u16,title:*const u16,
    flags:u32,offset:u16,extension:u16,default_ext:*const u16,data:isize,
    hook:Option<unsafe extern "system" fn(H,u32,usize,isize)->usize>,template:*const u16,reserved:*mut c_void,reserved_word:u32,flags_ex:u32,
}
const _:()={assert!(size_of::<Class>()==72);assert!(size_of::<Msg>()==48);assert!(size_of::<OpenFile>()==152);};
#[link(name="user32")] unsafe extern "system" {
    fn RegisterClassW(c:*const Class)->u16; fn UnregisterClassW(c:*const u16,i:H)->i32;
    fn CreateWindowExW(ex:u32,c:*const u16,t:*const u16,s:u32,x:i32,y:i32,w:i32,h:i32,p:H,m:H,i:H,a:*mut c_void)->H;
    fn DefWindowProcW(h:H,m:u32,w:usize,l:isize)->isize;
    fn SetWindowLongPtrW(h:H,i:i32,v:isize)->isize; fn GetWindowLongPtrW(h:H,i:i32)->isize;
    fn AdjustWindowRectEx(r:*mut Rect,s:u32,m:i32,e:u32)->i32;
    fn ShowWindow(h:H,c:i32)->i32; fn UpdateWindow(h:H)->i32;fn IsWindow(h:H)->i32;fn DestroyWindow(h:H)->i32;
    fn PostQuitMessage(c:i32);fn GetMessageW(m:*mut Msg,h:H,a:u32,b:u32)->i32;
    fn TranslateMessage(m:*const Msg)->i32;fn DispatchMessageW(m:*const Msg)->isize;fn IsDialogMessageW(h:H,m:*mut Msg)->i32;
    fn GetDlgItem(h:H,i:i32)->H;fn GetWindowTextLengthW(h:H)->i32;fn GetWindowTextW(h:H,t:*mut u16,n:i32)->i32;
    fn SetWindowTextW(h:H,t:*const u16)->i32;fn SendMessageW(h:H,m:u32,w:usize,l:isize)->isize;
    fn SetFocus(h:H)->H;fn LoadCursorW(i:H,n:*const u16)->H;
    fn MessageBoxW(h:H,t:*const u16,c:*const u16,f:u32)->i32;
}
#[link(name="kernel32")] unsafe extern "system" {fn GetModuleHandleW(n:*const u16)->H;}
#[link(name="gdi32")] unsafe extern "system" {fn GetStockObject(i:i32)->H;}
#[link(name="comdlg32")] unsafe extern "system" {
    fn GetOpenFileNameW(o:*mut OpenFile)->i32;fn GetSaveFileNameW(o:*mut OpenFile)->i32;fn CommDlgExtendedError()->u32;
}
struct State {base:PathBuf,value:Option<Options>,blenders:Vec<crate::blender_detection::Candidate>}
struct Window {hwnd:H,instance:H,class:Vec<u16>}
impl Drop for Window {fn drop(&mut self){unsafe {if IsWindow(self.hwnd)!=0 {DestroyWindow(self.hwnd);}UnregisterClassW(self.class.as_ptr(),self.instance);}}}
fn wide(s:&str)->Vec<u16>{s.encode_utf16().chain(Some(0)).collect()}
fn error(api:&str)->String{format!("{api}: {}",std::io::Error::last_os_error())}
fn show_message(owner:H,text:&str,failed:bool){unsafe{MessageBoxW(owner,wide(text).as_ptr(),wide("Nexa Blend Camera").as_ptr(),if failed{0x10}else{0x40});}}
pub fn message(text:&str,failed:bool){show_message(null_mut(),text,failed);}
fn control(h:H,c:&str,t:&str,id:i32,b:[i32;4],s:u32)->Result<(),String>{
    let [x,y,w,height]=b;
    let child=unsafe{CreateWindowExW(if c=="EDIT"{0x200}else{0},wide(c).as_ptr(),wide(t).as_ptr(),0x50000000|s,x,y,w,height,h,id as usize as H,GetModuleHandleW(null()),null_mut())};
    if child.is_null(){return Err(error("CreateWindowExW(control)"));}
    unsafe{SendMessageW(child,0x30,GetStockObject(17) as usize,1);if c=="EDIT"{SendMessageW(child,0xc5,32767,0);}}
    Ok(())
}
fn read(h:H,id:i32)->Vec<u16>{unsafe{
    let child=GetDlgItem(h,id);let length=GetWindowTextLengthW(child).max(0) as usize;
    let mut value=vec![0;length+1];let copied=GetWindowTextW(child,value.as_mut_ptr(),value.len() as i32);
    value.truncate(copied.max(0) as usize);value
}}
fn text(h:H,id:i32)->Result<String,String>{String::from_utf16(&read(h,id)).map_err(|_|"Invalid Unicode in text field".into())}
fn optional(h:H,id:i32)->Result<Option<String>,String>{let s=text(h,id)?;Ok(if s.is_empty(){None}else{Some(s)})}
fn number(h:H,id:i32,name:&str)->Result<Option<f64>,String>{
    let s=text(h,id)?;if s.trim().is_empty(){return Ok(None);}
    let n=s.trim().parse::<f64>().map_err(|_|format!("{name}: 数値を入力してください"))?;
    if !n.is_finite(){return Err(format!("{name}: 有限の数値を入力してください"));}Ok(Some(n))
}
fn dimension(h:H,id:i32)->Result<u32,String>{text(h,id)?.trim().parse::<u32>().ok().filter(|n|(1..=16384).contains(n)).ok_or("出力寸法は1〜16384の整数です".into())}
fn checked(h:H,id:i32)->bool{unsafe{SendMessageW(GetDlgItem(h,id),0xf0,0,0)==1}}
fn ext(p:&Path,e:&str)->bool{p.extension().and_then(|v|v.to_str()).is_some_and(|v|v.eq_ignore_ascii_case(e))}
fn collect(h:H,base:&Path)->Result<Options,String>{
    let path=|id|->Result<PathBuf,String>{let v=read(h,id);if v.is_empty(){return Err("ファイルを選択してください".into());}
        let p=PathBuf::from(OsString::from_wide(&v));Ok(if p.is_absolute(){p}else{base.join(p)})};
    let blender=path(10)?;let source=path(11)?;let output=path(12)?;
    if !blender.is_file()||!ext(&blender,"exe"){return Err("Blenderの実行ファイルを選択してください".into());}
    if !source.is_file()||!ext(&source,"blend"){return Err("入力には.blendファイルを選択してください".into());}
    if !ext(&output,"vmd")||!output.parent().is_some_and(|p|p.is_dir()){return Err("既存フォルダー内の.vmd出力先を指定してください".into());}
    let start=number(h,15,"開始")?;let end=number(h,16,"終了")?;
    if start.zip(end).is_some_and(|(a,b)|a>b){return Err("開始は終了以下にしてください".into());}
    let units_per_meter=number(h,17,"単位倍率")?.filter(|v|*v>0.0).ok_or("単位倍率は正数です")?;
    Ok(Options{blender,source,output,scene:optional(h,13)?,camera:optional(h,14)?,start,end,units_per_meter,
        width:dimension(h,18)?,height:dimension(h,19)?,allow_fov_approximation:checked(h,20),overwrite:checked(h,21)})
}
fn select_file(h:H,id:i32,current:&[u16])->Result<Option<PathBuf>,String>{
    let mut file=vec![0u16;32768];
    if current.len()>=file.len(){return Err("ファイル名が長すぎます".into());}file[..current.len()].copy_from_slice(&current);
    let filter=wide(match id{10=>"Blender executable\0*.exe\0All files\0*.*\0",11=>"Blender file\0*.blend\0",_=>"VMD camera\0*.vmd\0"});
    let extension=wide(match id{10=>"exe",11=>"blend",_=>"vmd"});
    let mut o:OpenFile=unsafe{zeroed()};o.size=size_of::<OpenFile>() as u32;o.owner=h;o.filter=filter.as_ptr();o.index=1;
    o.file=file.as_mut_ptr();o.max_file=file.len() as u32;o.default_ext=extension.as_ptr();
    o.flags=0x0208080c|if id==12{0x10000}else{0x1000};
    let ok=unsafe{if id==12{GetSaveFileNameW(&mut o)}else{GetOpenFileNameW(&mut o)}};
    if ok==0{let code=unsafe{CommDlgExtendedError()};return if code==0{Ok(None)}else{Err(format!("File dialog: 0x{code:08x}"))};}
    let end=file.iter().position(|v|*v==0).ok_or("File dialog returned an unterminated path")?;
    let path=PathBuf::from(OsString::from_wide(&file[..end]));
    if id==11&&(!path.is_file()||!ext(&path,"blend")){return Err("入力には.blendファイルを選択してください".into());}
    Ok(Some(path))
}
fn set_path(h:H,id:i32,path:&Path)->Result<(),String>{
    let value:Vec<u16>=std::os::windows::ffi::OsStrExt::encode_wide(path.as_os_str()).chain(Some(0)).collect();
    if unsafe{SetWindowTextW(GetDlgItem(h,id),value.as_ptr())}==0{return Err(error("SetWindowTextW"));}Ok(())
}
fn browse(h:H,id:i32)->Result<(),String>{
    if let Some(path)=select_file(h,id,&read(h,id))?{set_path(h,id,&path)?;}Ok(())
}
fn fill_blenders(h:H,candidates:&[crate::blender_detection::Candidate])->Result<(),String>{
    if let Some(first)=candidates.first(){
        for candidate in candidates{unsafe{SendMessageW(GetDlgItem(h,22),0x143,0,wide(&candidate.label()).as_ptr() as isize);}}
        unsafe{SendMessageW(GetDlgItem(h,22),0x160,700,0);SendMessageW(GetDlgItem(h,22),0x14e,0,0);}
        set_path(h,10,&first.path)?;
    }else{unsafe{SendMessageW(GetDlgItem(h,22),0x143,0,wide("未検出・手動指定").as_ptr() as isize);SendMessageW(GetDlgItem(h,22),0x14e,0,0);}}
    Ok(())
}
unsafe extern "system" fn procedure(h:H,m:u32,w:usize,l:isize)->isize{
    if m==2{unsafe{PostQuitMessage(0);}return 0;}
    if m==0x111&&(w&0xffff)==22&&(w>>16)==1{
        let state=unsafe{GetWindowLongPtrW(h,-21)} as *mut State;
        let selected=unsafe{SendMessageW(GetDlgItem(h,22),0x147,0,0)};
        if !state.is_null()&&selected>=0{
            if let Some(path)=unsafe{(&(*state).blenders).get(selected as usize)}.map(|c|c.path.clone()){
                if let Err(e)=set_path(h,10,&path){show_message(h,&e,true);}
            }
        }return 0;
    }
    if m==0x111&&(w>>16)==0{
        match (w&0xffff) as i32{
            id@110..=112=>{if let Err(e)=browse(h,id-100){show_message(h,&e,true);}return 0;}
            1=>{let state=unsafe{GetWindowLongPtrW(h,-21)} as *mut State;
                if !state.is_null(){let base=unsafe{(*state).base.clone()};match collect(h,&base){
                    Ok(v)=>unsafe{(*state).value=Some(v);DestroyWindow(h);},Err(e)=>show_message(h,&e,true)}}return 0;}
            2=>{unsafe{DestroyWindow(h);}return 0;}_=>{}
        }
    }
    unsafe{DefWindowProcW(h,m,w,l)}
}
pub fn show()->Result<Option<Options>,String>{
    let Some(source)=select_file(null_mut(),11,&[])? else{return Ok(None);};
    show_settings(&source)
}
fn show_settings(source:&Path)->Result<Option<Options>,String>{
    let mut state=Box::new(State{base:std::env::current_dir().map_err(|e|e.to_string())?,value:None,blenders:crate::blender_detection::detect()});
    let name=wide("NexaMmdCameraWorkerWindow");let instance=unsafe{GetModuleHandleW(null())};
    if instance.is_null(){return Err(error("GetModuleHandleW"));}
    let class=Class{style:0,proc:Some(procedure),cls:0,wnd:0,instance,icon:null_mut(),cursor:unsafe{LoadCursorW(null_mut(),32512usize as _)},background:16usize as H,menu:null(),name:name.as_ptr()};
    if unsafe{RegisterClassW(&class)}==0{return Err(error("RegisterClassW"));}
    let style=0x00ca0000;let ex=0x00010000;
    let mut r=Rect{left:0,top:0,right:760,bottom:512};
    if unsafe{AdjustWindowRectEx(&mut r,style,0,ex)}==0{let e=error("AdjustWindowRectEx");unsafe{UnregisterClassW(name.as_ptr(),instance);}return Err(e);}
    let hwnd=unsafe{CreateWindowExW(ex,name.as_ptr(),wide("Nexa Blend Camera").as_ptr(),style,i32::MIN,i32::MIN,r.right-r.left,r.bottom-r.top,null_mut(),null_mut(),instance,null_mut())};
    if hwnd.is_null(){let e=error("CreateWindowExW");unsafe{UnregisterClassW(name.as_ptr(),instance);}return Err(e);}
    let _window=Window{hwnd,instance,class:name};let pointer=&mut *state as *mut State as isize;
    unsafe{SetWindowLongPtrW(hwnd,-21,pointer);}
    if unsafe{GetWindowLongPtrW(hwnd,-21)}!=pointer{return Err("GUI state setup failed".into());}
    for (row,(id,label)) in [(10,"Blender実行ファイル (.exe)"),(11,"入力 (.blend)"),(12,"カメラ出力 (.vmd)")].into_iter().enumerate(){
        let y=16+row as i32*58;control(hwnd,"STATIC",label,0,[20,y,720,20],0)?;
        control(hwnd,"EDIT","",id,[20,y+22,if id==10{435}else{610},26],0x10080)?;control(hwnd,"BUTTON","参照...",id+100,[640,y+21,100,28],0x10000)?;
    }
    control(hwnd,"COMBOBOX","",22,[465,38,165,220],0x10003)?;
    fill_blenders(hwnd,&state.blenders)?;
    set_path(hwnd,11,source)?;
    set_path(hwnd,12,&source.with_extension("camera.vmd"))?;
    for (id,label,x,y,width,value) in [
        (13,"シーン名（空欄で現在のシーン）",20,198,340,""),(14,"カメラ名（空欄で切替も評価）",390,198,350,""),
        (15,"開始（空欄でシーン設定）",20,262,165,""),(16,"終了（空欄でシーン設定）",205,262,165,""),
        (17,"MMD単位 / メートル",390,262,350,"12.5"),(18,"出力幅 (px)",20,326,165,"1920"),(19,"出力高さ (px)",205,326,165,"1080")]{
        control(hwnd,"STATIC",label,0,[x,y,width,20],0)?;control(hwnd,"EDIT",value,id,[x,y+22,width,26],0x10080)?;
    }
    control(hwnd,"STATIC","MMD側も同じ出力寸法に設定してください",0,[390,351,350,22],0)?;
    control(hwnd,"BUTTON","整数FOVへの近似を許可（誤差を結果に表示）",20,[20,394,720,24],0x10003)?;
    control(hwnd,"BUTTON","既存の出力ファイルを上書き",21,[20,423,720,24],0x10003)?;
    control(hwnd,"BUTTON","変換",1,[525,466,100,30],0x10001)?;control(hwnd,"BUTTON","キャンセル",2,[640,466,100,30],0x10000)?;
    unsafe{ShowWindow(hwnd,5);UpdateWindow(hwnd);SetFocus(GetDlgItem(hwnd,10));}
    let mut msg:Msg=unsafe{zeroed()};
    loop {match unsafe{GetMessageW(&mut msg,null_mut(),0,0)}{
        -1=>return Err(error("GetMessageW")),0=>break,
        _=>unsafe{if IsDialogMessageW(hwnd,&mut msg)==0{TranslateMessage(&msg);DispatchMessageW(&msg);}}
    }}
    Ok(state.value.take())
}

#[cfg(test)] mod detection_gui_tests{
    use super::*;
    #[test] fn hidden_controls_prefill_switch_and_allow_manual_path(){
        // Only a private, invisible test window; no input injection into user apps.
        let name=wide("NexaBlenderDetectionControlsTest");let instance=unsafe{GetModuleHandleW(null())};
        let class=Class{style:0,proc:Some(procedure),cls:0,wnd:0,instance,icon:null_mut(),cursor:null_mut(),background:16usize as H,menu:null(),name:name.as_ptr()};
        assert_ne!(unsafe{RegisterClassW(&class)},0);
        let h=unsafe{CreateWindowExW(0,name.as_ptr(),name.as_ptr(),0,0,0,760,512,null_mut(),null_mut(),instance,null_mut())};assert!(!h.is_null());
        let mut state=Box::new(State{base:PathBuf::from("C:/"),value:None,blenders:vec![
            crate::blender_detection::Candidate{path:PathBuf::from("C:/Blender 4.5/blender.exe"),version:vec![4,5]},
            crate::blender_detection::Candidate{path:PathBuf::from("C:/Blender 4.2/blender.exe"),version:vec![4,2]},
        ]});
        let window=Window{hwnd:h,instance,class:name};unsafe{SetWindowLongPtrW(h,-21,&mut *state as *mut State as isize);}
        control(h,"EDIT","",10,[20,38,435,26],0x10080).unwrap();control(h,"COMBOBOX","",22,[465,38,165,220],0x10003).unwrap();
        fill_blenders(h,&state.blenders).unwrap();assert_eq!(text(h,10).unwrap(),"C:/Blender 4.5/blender.exe");
        unsafe{SendMessageW(GetDlgItem(h,22),0x14e,1,0);SendMessageW(h,0x111,(1<<16)|22,0);}
        assert_eq!(text(h,10).unwrap(),"C:/Blender 4.2/blender.exe");
        set_path(h,10,Path::new("C:/Portable/blender.exe")).unwrap();assert_eq!(text(h,10).unwrap(),"C:/Portable/blender.exe");
        unsafe{SendMessageW(GetDlgItem(h,22),0x14b,0,0);}fill_blenders(h,&[]).unwrap();
        assert_eq!(text(h,10).unwrap(),"C:/Portable/blender.exe");
        drop(window);
    }
}
