//! Native component selection and responsive, cancellable import.
#![cfg(windows)]
use crate::{camera, packet::ScenePacket, pipeline, source, FrameRange, ImportRequest, Language, Selection};
use std::{cell::{Cell, RefCell}, ffi::{c_void, OsString}, mem::{size_of, zeroed}, os::windows::ffi::OsStringExt,
    path::PathBuf, ptr::{null, null_mut}, sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc}, thread};
type H = *mut c_void;
type Proc = unsafe extern "system" fn(H,u32,usize,isize)->isize;
#[repr(C)] struct Class { style:u32, proc:Option<Proc>, cls:i32, wnd:i32, instance:H, icon:H, cursor:H, background:H, menu:*const u16, name:*const u16 }
#[repr(C)] struct Msg { hwnd:H, message:u32, w:usize, l:isize, time:u32, point:[i32;2], private:u32 }
#[repr(C)] struct OpenFile { size:u32, owner:H, instance:H, filter:*const u16, custom:*mut u16, max_custom:u32, index:u32,
    file:*mut u16, max_file:u32, title_file:*mut u16, max_title:u32, initial:*const u16, title:*const u16,
    flags:u32, offset:u16, extension:u16, default_ext:*const u16, data:isize,
    hook:Option<unsafe extern "system" fn(H,u32,usize,isize)->usize>, template:*const u16, reserved:*mut c_void, reserved_word:u32, flags_ex:u32 }
const _: () = { assert!(size_of::<Class>()==72); assert!(size_of::<Msg>()==48); assert!(size_of::<OpenFile>()==152); };
#[link(name="user32")] unsafe extern "system" {
    fn RegisterClassW(c:*const Class)->u16; fn UnregisterClassW(c:*const u16,i:H)->i32;
    fn CreateWindowExW(ex:u32,c:*const u16,t:*const u16,s:u32,x:i32,y:i32,w:i32,height:i32,p:H,m:H,i:H,a:*mut c_void)->H;
    fn DefWindowProcW(h:H,m:u32,w:usize,l:isize)->isize; fn DestroyWindow(h:H)->i32; fn IsWindow(h:H)->i32;
    fn SetWindowLongPtrW(h:H,i:i32,v:isize)->isize; fn GetWindowLongPtrW(h:H,i:i32)->isize;
    fn GetDlgItem(h:H,i:i32)->H; fn GetWindowTextLengthW(h:H)->i32; fn GetWindowTextW(h:H,b:*mut u16,n:i32)->i32;
    fn SetWindowTextW(h:H,s:*const u16)->i32; fn SendMessageW(h:H,m:u32,w:usize,l:isize)->isize;
    fn EnableWindow(h:H,on:i32)->i32; fn ShowWindow(h:H,show:i32)->i32; fn UpdateWindow(h:H)->i32;
    fn SetWindowPos(h:H,after:H,x:i32,y:i32,w:i32,height:i32,flags:u32)->i32;
    fn LoadCursorW(i:H,n:*const u16)->H; fn MessageBoxW(h:H,t:*const u16,c:*const u16,flags:u32)->i32;
    fn SetTimer(h:H,id:usize,ms:u32,callback:*const c_void)->usize; fn KillTimer(h:H,id:usize)->i32;
    fn GetMessageW(m:*mut Msg,h:H,lo:u32,hi:u32)->i32; fn TranslateMessage(m:*const Msg)->i32;
    fn DispatchMessageW(m:*const Msg)->isize; fn IsDialogMessageW(h:H,m:*mut Msg)->i32; fn PostQuitMessage(code:i32);
    fn PostMessageW(h:H,m:u32,w:usize,l:isize)->i32;
}
#[link(name="comdlg32")] unsafe extern "system" { fn GetOpenFileNameW(f:*mut OpenFile)->i32; fn CommDlgExtendedError()->u32; }
#[link(name="kernel32")] unsafe extern "system" { fn GetModuleHandleW(name:*const u16)->H; }
#[link(name="gdi32")] unsafe extern "system" { fn GetStockObject(index:i32)->H; }

const RUN:i32=1; const CANCEL:i32=2; const DETAILS:i32=3; const BROWSE:i32=4;
const BLENDER:i32=10; const MODEL:i32=11; const CAMERA:i32=12; const ANIMATION:i32=13;
const SCENE:i32=20; const REFERENCE:i32=21; const START:i32=22; const END:i32=23; const UNITS:i32=24;
const OUTPUT:i32=25; const APPROX:i32=26; const STATUS:i32=30; const SUMMARY:i32=31;
const ADVANCED:&[i32]=&[20,21,22,23,24,25,26,120,121,122,123,124,125];
fn wide(s:&str)->Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }
fn choose(language:Language,ja:&str,en:&str)->String { language.choose(ja,en).into() }
fn set(h:H,id:i32,text:&str) { unsafe { SetWindowTextW(GetDlgItem(h,id),wide(text).as_ptr()); } }
fn message(h:H,text:&str,error:bool) { unsafe { MessageBoxW(h,wide(text).as_ptr(),wide(crate::PRODUCT_NAME).as_ptr(),if error {0x10}else{0x40}); } }
pub fn error_message(language:Language,text:&str) {
    message(null_mut(),&format!("{}\n\n{text}",language.choose("取り込みを完了できませんでした。","Import could not be completed.")),true);
}
fn read(h:H,id:i32)->Result<String,String> { unsafe {
    let child=GetDlgItem(h,id); let length=GetWindowTextLengthW(child);
    if !(0..=32767).contains(&length) { return Err("text field is too long".into()); }
    let mut value=vec![0;length as usize+1]; GetWindowTextW(child,value.as_mut_ptr(),length+1);
    String::from_utf16(&value[..length as usize]).map_err(|_|"invalid Unicode in text field".into())
} }
fn checked(h:H,id:i32)->bool { unsafe { SendMessageW(GetDlgItem(h,id),0xf0,0,0)==1 } }
fn optional_number(h:H,id:i32)->Result<Option<f64>,String> {
    let text=read(h,id)?; if text.trim().is_empty() { return Ok(None); }
    text.trim().parse::<f64>().ok().filter(|x|x.is_finite()).map(Some).ok_or_else(||"expected a finite number".into())
}
fn control(h:H,class:&str,text:&str,id:i32,xy:[i32;4],style:u32)->Result<(),String> { unsafe {
    let [x,y,w,height]=xy;
    let child=CreateWindowExW(if class=="EDIT"{0x200}else{0},wide(class).as_ptr(),wide(text).as_ptr(),0x50000000|style,
        x,y,w,height,h,id as usize as H,GetModuleHandleW(null()),null_mut());
    if child.is_null() { return Err(format!("CreateWindowExW: {}",std::io::Error::last_os_error())); }
    SendMessageW(child,0x30,GetStockObject(17) as usize,1); Ok(())
} }

pub fn select_file(owner:H,extension:&str,title:&str)->Result<Option<PathBuf>,String> {
    let filter=wide(&format!("{extension} files\0*.{extension}\0")); let caption=wide(title); let ext=wide(extension);
    let mut buffer=vec![0u16;32768]; let mut dialog:OpenFile=unsafe{zeroed()};
    dialog.size=size_of::<OpenFile>() as u32; dialog.owner=owner; dialog.filter=filter.as_ptr(); dialog.index=1;
    dialog.file=buffer.as_mut_ptr(); dialog.max_file=buffer.len() as u32; dialog.title=caption.as_ptr();
    dialog.default_ext=ext.as_ptr(); dialog.flags=0x80000|0x1000|0x800|8;
    if unsafe{GetOpenFileNameW(&mut dialog)}!=0 {
        let end=buffer.iter().position(|n|*n==0).ok_or("unterminated file path")?;
        let path=PathBuf::from(OsString::from_wide(&buffer[..end]));
        if !path.extension().is_some_and(|e|e.eq_ignore_ascii_case(extension)) { return Err(format!("expected .{extension}")); }
        Ok(Some(path))
    } else { let error=unsafe{CommDlgExtendedError()}; if error==0 {Ok(None)} else {Err(format!("file dialog: {error:#x}"))} }
}

enum Event { Prepared(Result<ScenePacket,String>), Published(Result<pipeline::ImportResult,String>) }
struct State {
    language:Language, input:PathBuf, details:bool, busy:bool, closing:bool,
    cancelled:Arc<AtomicBool>, receiver:Option<mpsc::Receiver<Event>>, worker:Option<thread::JoinHandle<()>>,
    options:Option<pipeline::Options>,
}
impl Drop for State { fn drop(&mut self) { self.cancelled.store(true,Ordering::Release); if let Some(worker)=self.worker.take(){let _=worker.join();} } }
// Native modal dialogs pump messages synchronously. Never alias an active State
// borrow when a timer/command arrives during one of those nested message loops.
struct GuiContext { state:RefCell<State>, close_requested:Cell<bool> }
fn layout(h:H,state:&State) { unsafe {
    for id in ADVANCED { ShowWindow(GetDlgItem(h,*id),if state.details{5}else{0}); }
    let bottom=if state.details{530}else{285};
    SetWindowPos(GetDlgItem(h,STATUS),null_mut(),20,bottom-42,650,38,0x14);
    SetWindowPos(GetDlgItem(h,RUN),null_mut(),455,bottom,110,28,0x14);
    SetWindowPos(GetDlgItem(h,CANCEL),null_mut(),575,bottom,100,28,0x14);
    SetWindowPos(h,null_mut(),0,0,715,if state.details{610}else{365},0x16);
} }
fn busy(h:H,state:&mut State,on:bool) {
    state.busy=on;
    for id in [RUN,DETAILS,BROWSE,BLENDER,MODEL,CAMERA,ANIMATION,SCENE,REFERENCE,START,END,UNITS,OUTPUT,APPROX] {
        unsafe{EnableWindow(GetDlgItem(h,id),i32::from(!on));}
    }
}
fn options(h:H,state:&State)->Result<pipeline::Options,String> {
    let selection=Selection{model:checked(h,MODEL),camera:checked(h,CAMERA),animation:checked(h,ANIMATION)};
    selection.validate().map_err(|_|choose(state.language,"少なくとも1つの要素を選択してください。","Select at least one component."))?;
    let start=optional_number(h,START)?;let end=optional_number(h,END)?;
    let range=match(start,end){(None,None)=>None,(Some(start),Some(end)) if start<=end=>Some(FrameRange{start,end}),_=>return Err(choose(state.language,"開始・終了フレームを両方指定してください。","Specify both start and end frames in order."))};
    let units=optional_number(h,UNITS)?.filter(|n|*n>0.0).ok_or_else(||choose(state.language,"単位倍率は正の数です。","Unit scale must be positive."))?;
    let output=PathBuf::from(read(h,OUTPUT)?); if !output.is_absolute()||!output.is_dir(){return Err(choose(state.language,"出力先には既存の絶対フォルダを指定してください。","Choose an existing absolute output directory."));}
    let scene=read(h,SCENE)?;
    Ok(pipeline::Options{source:source::SourceOptions{blender:PathBuf::from(read(h,BLENDER)?),input:state.input.clone(),scratch_parent:output.clone(),
        scene:if scene.is_empty(){None}else{Some(scene)},request:ImportRequest{selection,reference_frame:optional_number(h,REFERENCE)?,frame_range:range,units_per_meter:units}},
        output_parent:output,allow_fov_approximation:checked(h,APPROX)})
}
fn start(h:H,state:&mut State)->Result<(),String> {
    let options=options(h,state)?; state.options=Some(options.clone());state.cancelled.store(false,Ordering::Release);
    let(tx,rx)=mpsc::channel();state.receiver=Some(rx);let flag=state.cancelled.clone();
    busy(h,state,true);set(h,STATUS,&choose(state.language,"Blenderでシーンを評価中です…","Evaluating the scene in Blender…"));
    state.worker=Some(thread::spawn(move||{let result=pipeline::prepare(&options,&flag);let _=tx.send(Event::Prepared(result));}));
    Ok(())
}
fn summary(result:&pipeline::ImportResult,language:Language)->String {
    let mut text=format!("{}\n{}\n\n{}\n",language.choose("取り込み用ファイルを保存しました。","Import files saved."),result.directory.display(),
        language.choose("MMDの標準操作で、次の順に読み込んでください。","Load these through MMD's standard operations in this order."));
    for model in &result.models {text.push_str(&format!("PMX: {}\n",result.directory.join(model).display()));}
    for(target,motion)in &result.motions {let target=if target.is_absolute(){target.clone()}else{result.directory.join(target)};
        text.push_str(&format!("VMD: {} → {}\n",result.directory.join(motion).display(),target.display()));}
    if let Some(camera)=&result.camera {text.push_str(&format!("{}: {}\n",language.choose("カメラVMD","Camera VMD"),result.directory.join(camera).display()));}
    text.push_str(&format!("\n{}\n",language.choose("カメラはカメラ・照明モードで読み込みます。先頭から配置する場合は現在フレームを0に戻してください。","Load the camera VMD in camera/light mode. Set the current frame to 0 to load from the beginning.")));
    for notice in &result.notices {text.push_str(&notice.text(language));text.push('\n');}
    text.push_str(language.choose(
        "\n上限：抽出データ512MiB、出力2GiB・4096ファイル、256モデル、モデルごと400万頂点・65535骨、画像一辺16384px、108001フレーム、Blender処理600秒。",
        "\nLimits: 512 MiB scene packet; 2 GiB / 4096 output files; 256 models; 4 million vertices / 65535 bones per model; 16384 px image edge; 108001 sampled frames; 600 seconds in Blender."));
    text
}
fn poll(h:H,state:&mut State) {
    let event=match state.receiver.as_ref().map(|rx|rx.try_recv()) {
        Some(Ok(event))=>event,
        Some(Err(mpsc::TryRecvError::Disconnected))=>{
            if let Some(worker)=state.worker.take(){let _=worker.join();}state.receiver=None;
            finish(h,state,Err("conversion worker stopped unexpectedly".into()));return;
        },
        _=>return,
    };
    if let Some(worker)=state.worker.take(){let _=worker.join();}state.receiver=None;
    match event {
        Event::Prepared(Ok(packet))=>{
            if state.cancelled.load(Ordering::Acquire){finish(h,state,Err("conversion cancelled".into()));return;}
            let Some(options)=state.options.clone()else{finish(h,state,Err("missing import options".into()));return;};
            let mut targets=Vec::new();
            if options.source.request.selection.animation_only(){for group in &packet.groups {
                let title=format!("{}: {}",state.language.choose("対象PMXを選択","Choose matching PMX"),group.name);
                match select_file(h,"pmx",&title){Ok(Some(path))=>targets.push(path),Ok(None)=>{finish(h,state,Err("conversion cancelled".into()));return;},Err(e)=>{finish(h,state,Err(e));return;}}
            }}
            let(tx,rx)=mpsc::channel();state.receiver=Some(rx);let flag=state.cancelled.clone();
            set(h,STATUS,&choose(state.language,"選択した要素を変換中です…","Converting the selected components…"));
            state.worker=Some(thread::spawn(move||{let result=pipeline::publish(&packet,&options,&targets,&flag);let _=tx.send(Event::Published(result));}));
        },
        Event::Prepared(Err(error))=>finish(h,state,Err(error)),
        Event::Published(result)=>finish(h,state,result),
    }
}
fn finish(h:H,state:&mut State,result:Result<pipeline::ImportResult,String>) {
    busy(h,state,false);
    match result {Ok(result)=>{set(h,STATUS,&choose(state.language,"完了しました。","Completed."));message(h,&summary(&result,state.language),false);},
        Err(error)=>{set(h,STATUS,&choose(state.language,"完了していません。既存ファイルは変更していません。","Not completed. Existing files were not changed."));
            if error!="conversion cancelled"{message(h,&format!("{}\n\n{error}",state.language.choose("取り込みを完了できませんでした。詳細：","Import could not be completed. Details:")),true);}}}
    if state.closing {unsafe{DestroyWindow(h);}}
}
unsafe extern "system" fn procedure(h:H,msg:u32,w:usize,l:isize)->isize {
    if msg==0x82 {unsafe{SetWindowLongPtrW(h,-21,0);}return unsafe{DefWindowProcW(h,msg,w,l)};}
    if msg==2 {unsafe{KillTimer(h,1);PostQuitMessage(0);}return 0;}
    if msg!=0x111 && msg!=0x113 && msg!=0x10 {return unsafe{DefWindowProcW(h,msg,w,l)};}
    if msg==0x111 && (w>>16)!=0 {return unsafe{DefWindowProcW(h,msg,w,l)};}
    let pointer=unsafe{GetWindowLongPtrW(h,-21)} as *const GuiContext;
    if pointer.is_null(){return unsafe{DefWindowProcW(h,msg,w,l)};}
    let context=unsafe{&*pointer};
    let Ok(mut state)=context.state.try_borrow_mut() else {
        if msg==0x10 {context.close_requested.set(true);}
        return 0;
    };
    dispatch(h,msg,w,&mut state);
    drop(state);
    if context.close_requested.replace(false) {unsafe{PostMessageW(h,0x10,0,0);}}
    0
}
fn dispatch(h:H,msg:u32,w:usize,state:&mut State) {
    match msg {
        0x113=>poll(h,state),
        0x10=>{if state.busy{state.closing=true;state.cancelled.store(true,Ordering::Release);set(h,STATUS,&choose(state.language,"取消処理中です…","Cancelling…"));}else{unsafe{DestroyWindow(h);}}},
        0x111=>match (w&0xffff)as i32 {
            RUN if !state.busy=>if let Err(error)=start(h,state){message(h,&error,true);},
            CANCEL=>{if state.busy{state.cancelled.store(true,Ordering::Release);set(h,STATUS,&choose(state.language,"取消処理中です…","Cancelling…"));}else{unsafe{DestroyWindow(h);}}},
            DETAILS if !state.busy=>{state.details=!state.details;layout(h,state);},
            BROWSE if !state.busy=>match select_file(h,"exe",state.language.choose("Blenderを選択","Choose Blender")) {Ok(Some(path))=>set(h,BLENDER,&path.to_string_lossy()),Ok(None)=>{},Err(e)=>message(h,&e,true)},
            _=>{}
        },_=>{}
    }
}

pub fn run(language:Language)->Result<(),String> {
    let Some(input)=select_file(null_mut(),"blend",language.choose("blendを取り込み","Import .blend"))?else{return Ok(());};
    let parent=input.parent().ok_or("input has no parent")?.to_path_buf();
    let context=Box::new(GuiContext{state:RefCell::new(State{language,input,details:false,busy:false,closing:false,cancelled:Arc::new(AtomicBool::new(false)),receiver:None,worker:None,options:None}),close_requested:Cell::new(false)});
    let state=context.state.borrow_mut();
    let class=wide(&format!("NexaBlendImporter-{}",std::process::id()));let instance=unsafe{GetModuleHandleW(null())};
    let descriptor=Class{style:0,proc:Some(procedure),cls:0,wnd:0,instance,icon:null_mut(),cursor:unsafe{LoadCursorW(null_mut(),32512usize as *const u16)},background:6usize as H,menu:null(),name:class.as_ptr()};
    if unsafe{RegisterClassW(&descriptor)}==0 {return Err(format!("RegisterClassW: {}",std::io::Error::last_os_error()));}
    let window=unsafe{CreateWindowExW(0,class.as_ptr(),wide(crate::PRODUCT_NAME).as_ptr(),0x00ca0000,160,120,715,365,null_mut(),null_mut(),instance,null_mut())};
    if window.is_null(){unsafe{UnregisterClassW(class.as_ptr(),instance);}return Err(format!("CreateWindowExW: {}",std::io::Error::last_os_error()));}
    let built=(||->Result<(),String>{
        control(window,"STATIC",&state.input.to_string_lossy(),100,[20,15,655,32],0)?;
        control(window,"STATIC",&choose(language,"出力：入力ファイル隣の新規NexaImportフォルダー","Output: a new NexaImport folder beside the input"),SUMMARY,[20,52,655,28],0)?;
        for(id,x,ja,en)in[(MODEL,20,"モデル","Model"),(CAMERA,230,"カメラ","Camera"),(ANIMATION,440,"アニメーション","Animation")]{
            control(window,"BUTTON",&choose(language,ja,en),id,[x,100,200,26],0x10000|3)?;unsafe{SendMessageW(GetDlgItem(window,id),0xf1,1,0);}}
        control(window,"STATIC",&choose(language,"Blender（自動検出・手動指定）","Blender (detected or manually selected)"),101,[20,138,655,22],0)?;
        control(window,"COMBOBOX","",BLENDER,[20,163,545,180],0x10000|0x200000|2)?;
        let candidates=camera::blender_detection::detect();for candidate in &candidates{unsafe{SendMessageW(GetDlgItem(window,BLENDER),0x143,0,wide(&candidate.path.to_string_lossy()).as_ptr()as isize);}}
        if !candidates.is_empty(){unsafe{SendMessageW(GetDlgItem(window,BLENDER),0x14e,0,0);}}
        control(window,"BUTTON",&choose(language,"参照…","Browse…"),BROWSE,[575,162,100,26],0x10000)?;
        control(window,"BUTTON",&choose(language,"詳細","Details"),DETAILS,[20,203,100,26],0x10000)?;
        for(label,id,ja,en,y,value)in[(120,SCENE,"シーン（空欄＝active）","Scene (blank = active)",244,String::new()),(121,REFERENCE,"基準フレーム（空欄＝保存時）","Reference frame (blank = saved)",277,String::new()),
            (122,START,"開始フレーム（空欄＝scene範囲）","Start (blank = scene range)",310,String::new()),(123,END,"終了フレーム","End frame",343,String::new()),
            (124,UNITS,"MMD単位 / m","MMD units / m",376,"12.5".into()),(125,OUTPUT,"出力先フォルダー","Output parent directory",409,parent.to_string_lossy().into_owned())]{
            control(window,"STATIC",&choose(language,ja,en),label,[20,y,290,23],0)?;control(window,"EDIT",&value,id,[315,y-2,360,25],0x10000|0x80)?;}
        control(window,"BUTTON",&choose(language,"非整数視野角の近似を許可","Allow approximation of non-integer camera FOV"),APPROX,[20,444,655,26],0x10000|3)?;
        control(window,"STATIC","",STATUS,[20,243,655,38],0)?;
        control(window,"BUTTON",&choose(language,"取り込み","Import"),RUN,[455,285,110,28],0x10000|1)?;
        control(window,"BUTTON",&choose(language,"取消","Cancel"),CANCEL,[575,285,100,28],0x10000)?;
        unsafe{SetWindowLongPtrW(window,-21,(&*context as *const GuiContext)as isize);}
        layout(window,&state);if unsafe{SetTimer(window,1,100,null())}==0{return Err("SetTimer failed".into());}
        unsafe{ShowWindow(window,5);UpdateWindow(window);}Ok(())
    })();
    drop(state);
    let result=if built.is_ok(){let mut message:Msg=unsafe{zeroed()};loop{let next=unsafe{GetMessageW(&mut message,null_mut(),0,0)};
        if next==0{break Ok(());}if next<0{break Err(format!("GetMessageW: {}",std::io::Error::last_os_error()));}
        if unsafe{IsDialogMessageW(window,&mut message)}==0{unsafe{TranslateMessage(&message);DispatchMessageW(&message);}}
    }}else{built};
    let mut state=context.state.borrow_mut();
    state.cancelled.store(true,Ordering::Release);if let Some(worker)=state.worker.take(){let _=worker.join();}
    unsafe{if IsWindow(window)!=0{DestroyWindow(window);}UnregisterClassW(class.as_ptr(),instance);}result
}

#[cfg(test)] mod lifecycle_tests;
