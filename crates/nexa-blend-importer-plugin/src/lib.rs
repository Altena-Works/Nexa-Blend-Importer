//! Pure Rust adapter for the pinned Windows x64 MMDPlugin DLL3 C++ interface.
#![cfg(all(windows,target_arch="x86_64"))]
use std::{ffi::{c_void,c_char,OsString},os::windows::{ffi::OsStringExt,process::CommandExt},path::PathBuf,
    ptr::{null,null_mut},process::{Child,Command,Stdio},sync::{Mutex,atomic::{AtomicIsize,AtomicU32,AtomicU64,Ordering}}};
type H=*mut c_void;
#[repr(transparent)] struct VTable([*const c_void;248]);
// Entries point only to immutable extern-system functions.
unsafe impl Sync for VTable {}
#[repr(C)] pub struct Plugin {
    table:*const VTable,window:AtomicIsize,popup:AtomicIsize,command:AtomicU32,
    trace_slot:AtomicU32,trace_digest:AtomicU64,
}
// All notification paths share one child, including the native window callback.
static WORKER:Mutex<Option<Child>>=Mutex::new(None);
type SubclassProc=unsafe extern "system" fn(H,u32,usize,isize,usize,usize)->isize;
#[repr(C)] struct Creation {adapter:u32,device_type:i32,window:H,behavior:u32}
#[repr(C)] struct Cwp {lparam:isize,wparam:usize,message:u32,window:H}
#[repr(C)] struct Pair {handled:u8,padding:[u8;7],result:isize}
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MsgPoint { x: i32, y: i32 }
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SdkMsg {
    window: H, message: u32, wparam: usize, lparam: isize,
    time: u32, point: MsgPoint, private: u32,
}
const _: () = {
    assert!(std::mem::size_of::<MsgPoint>() == 8);
    assert!(std::mem::align_of::<MsgPoint>() == 4);
    assert!(std::mem::offset_of!(MsgPoint, x) == 0);
    assert!(std::mem::offset_of!(MsgPoint, y) == 4);
    assert!(std::mem::size_of::<SdkMsg>() == 48);
    assert!(std::mem::align_of::<SdkMsg>() == 8);
    assert!(std::mem::offset_of!(SdkMsg, window) == 0);
    assert!(std::mem::offset_of!(SdkMsg, message) == 8);
    assert!(std::mem::offset_of!(SdkMsg, wparam) == 16);
    assert!(std::mem::offset_of!(SdkMsg, lparam) == 24);
    assert!(std::mem::offset_of!(SdkMsg, time) == 32);
    assert!(std::mem::offset_of!(SdkMsg, point) == 36);
    assert!(std::mem::offset_of!(SdkMsg, private) == 44);
};
const _:()={assert!(std::mem::size_of::<Creation>()==24);assert!(std::mem::size_of::<Cwp>()==32);assert!(std::mem::size_of::<Pair>()==16);};
#[link(name="user32")] unsafe extern "system" {
    fn GetMenu(h:H)->H;fn CreatePopupMenu()->H;fn AppendMenuW(m:H,f:u32,id:usize,t:*const u16)->i32;
    fn GetMenuItemCount(m:H)->i32;fn GetMenuItemID(m:H,p:i32)->u32;fn GetSubMenu(m:H,p:i32)->H;
    fn RemoveMenu(m:H,p:u32,f:u32)->i32;fn DestroyMenu(m:H)->i32;fn DrawMenuBar(h:H)->i32;
    fn ModifyMenuW(m:H,p:u32,f:u32,id:usize,t:*const u16)->i32;
    fn RegisterWindowMessageW(s:*const u16)->u32;fn IsWindow(h:H)->i32;fn MessageBoxW(h:H,t:*const u16,c:*const u16,f:u32)->i32;
    fn GetWindowThreadProcessId(h:H,p:*mut u32)->u32;
}
#[link(name="comctl32")] unsafe extern "system" {
    fn SetWindowSubclass(h:H,p:SubclassProc,id:usize,data:usize)->i32;
    fn RemoveWindowSubclass(h:H,p:SubclassProc,id:usize)->i32;
    fn DefSubclassProc(h:H,m:u32,w:usize,l:isize)->isize;
}
#[link(name="kernel32")] unsafe extern "system" {
    fn GetModuleHandleW(n:*const u16)->H;fn GetProcAddress(h:H,n:*const u8)->*mut c_void;
    fn GetModuleHandleExW(f:u32,n:*const u16,out:*mut H)->i32;fn GetModuleFileNameW(h:H,b:*mut u16,n:u32)->u32;
    fn GetCurrentThreadId()->u32;
}
mod callbacks;
mod menu_command;
#[cfg(test)] mod queued_menu_tests;
#[cfg(test)] mod native_menu_tests;
fn wide(s:&str)->Vec<u16>{s.encode_utf16().chain(Some(0)).collect()}
unsafe fn record(this:*mut Plugin,slot:u32,args:&[u64]){
    #[cfg(feature="abi-audit")] {
        let mut digest=0xcbf29ce484222325u64;
        for value in args {digest=digest.wrapping_mul(0x100000001b3).wrapping_add(*value);}
        unsafe {(*this).trace_slot.store(slot,Ordering::Relaxed);(*this).trace_digest.store(digest,Ordering::Relaxed);}
    }
    #[cfg(not(feature="abi-audit"))] {let _=(this,slot,args);}
}
unsafe fn device_window(device:*mut c_void)->H{
    let module=unsafe{GetModuleHandleW(wide("MMDPlugin.dll").as_ptr())};
    if !module.is_null(){
        let address=unsafe{GetProcAddress(module,c"getHWND".as_ptr().cast())};
        if !address.is_null(){
            let get:unsafe extern "C" fn()->H=unsafe{std::mem::transmute(address)};
            let window=unsafe{get()};if !window.is_null(){return window;}
        }
    }
    if device.is_null(){return null_mut();}
    let table=unsafe{*(device as *const *const *const c_void)};
    let get:unsafe extern "system" fn(*mut c_void,*mut Creation)->i32=unsafe{std::mem::transmute(*table.add(9))};
    let mut creation=Creation{adapter:0,device_type:0,window:null_mut(),behavior:0};
    if unsafe{get(device,&mut creation)}<0{return null_mut();}
    creation.window
}
unsafe fn contains_id(menu:H,id:u32,depth:u32)->bool{
    if depth>32{return true;}
    let count=unsafe{GetMenuItemCount(menu)};
    for index in 0..count {
        if unsafe{GetMenuItemID(menu,index)}==id{return true;}
        let child=unsafe{GetSubMenu(menu,index)};
        if !child.is_null()&&unsafe{contains_id(child,id,depth+1)}{return true;}
    }false
}
impl Plugin {
    fn remove_menu(&self){
        let window=self.window.load(Ordering::Relaxed) as H;
        let id=self.command.swap(0,Ordering::Relaxed);
        let popup=self.popup.swap(0,Ordering::Relaxed) as H;
        if popup.is_null(){return;}
        unsafe {
            if IsWindow(window)!=0 {
                if GetWindowThreadProcessId(window,null_mut())==GetCurrentThreadId(){
                    RemoveWindowSubclass(window,native_menu,id as usize);
                }
                let menu=GetMenu(window);
                for index in 0..GetMenuItemCount(menu) {
                    if GetSubMenu(menu,index)==popup {RemoveMenu(menu,index as u32,0x400);break;}
                }
                DrawMenuBar(window);
                DestroyMenu(popup);
            }
        }
    }
    fn add_menu(&self){
        if self.popup.load(Ordering::Relaxed)!=0{return;}
        let window=self.window.load(Ordering::Relaxed) as H;
        unsafe {
            if window.is_null()||IsWindow(window)==0{return;}
            let menu=GetMenu(window);if menu.is_null(){return;}
            let id=RegisterWindowMessageW(wide("Nexa.BlendImporter.Export.v1").as_ptr());
            if id==0||id>0xffff||contains_id(menu,id,0){return;}
            // No Plugin pointer is retained by Windows. Keep callback code alive
            // until process exit, even if teardown cannot remove the subclass.
            let mut module=null_mut();
            if GetWindowThreadProcessId(window,null_mut())!=GetCurrentThreadId()
                || GetModuleHandleExW(5,native_menu as *const () as *const u16,&mut module)==0
                || SetWindowSubclass(window,native_menu,id as usize,id as usize)==0 {
                MessageBoxW(window,wide("Nexa Blend Importer: メニュー通知を接続できませんでした。").as_ptr(),wide("Nexa Blend Importer").as_ptr(),0x10);
                return;
            }
            let popup=CreatePopupMenu();if popup.is_null(){RemoveWindowSubclass(window,native_menu,id as usize);return;}
            if AppendMenuW(popup,0,id as usize,wide(if is_english(){"Import .blend..."}else{"blendを取り込み"}).as_ptr())==0
                || AppendMenuW(menu,0x10,popup as usize,wide("Nexa Blend Importer").as_ptr())==0 {
                DestroyMenu(popup);RemoveWindowSubclass(window,native_menu,id as usize);return;
            }
            self.command.store(id,Ordering::Relaxed);self.popup.store(popup as isize,Ordering::Relaxed);DrawMenuBar(window);
        }
    }
    fn launch(&self)->Result<(),String>{
        launch_worker(self.window.load(Ordering::Relaxed) as H)
    }
}
fn launch_worker(_window:H)->Result<(),String>{
    let mut worker=WORKER.lock().map_err(|_|"worker state unavailable")?;
    if let Some(child)=worker.as_mut(){if child.try_wait().map_err(|e|e.to_string())?.is_none(){return Ok(());}}
    let mut module=null_mut();
    if unsafe{GetModuleHandleExW(6,create3 as *const () as *const u16,&mut module)}==0{return Err("cannot locate plugin module".into());}
    let mut path=vec![0u16;32768];let count=unsafe{GetModuleFileNameW(module,path.as_mut_ptr(),path.len() as u32)} as usize;
    if count==0||count>=path.len(){return Err("cannot locate plugin directory".into());}path.truncate(count);
    let mut exe=PathBuf::from(OsString::from_wide(&path));exe.set_file_name("nexa-blend-importer.exe");
    *worker=Some(Command::new(&exe).args(["--gui","--language",if is_english(){"en"}else{"ja"}]).creation_flags(0x08000000)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e|format!("{}: {e}",exe.display()))?);
    Ok(())
}
fn is_english()->bool{
    let h=unsafe{GetModuleHandleW(null())};
    let p=unsafe{GetProcAddress(h,c"ExpGetEnglishMode".as_ptr().cast())};
    if p.is_null(){false}else{let get:unsafe extern "C" fn()->bool=unsafe{std::mem::transmute(p)};unsafe{get()}}
}
fn show_error(window:H,error:&str){
    let message=if is_english(){format!("Could not start Nexa Blend Importer.\n{error}")}else{format!("Nexa Blend Importerを起動できませんでした。\n{error}")};
    unsafe{MessageBoxW(window,wide(&message).as_ptr(),wide("Nexa Blend Importer").as_ptr(),0x10);}
}
unsafe fn refresh_menu_language(popup:H,command:u32,english:bool){
    // Only update our single-item popup, never another plugin's menu.
    if command==0 || popup.is_null() || unsafe{GetMenuItemCount(popup)}!=1
        || unsafe{GetMenuItemID(popup,0)}!=command {return;}
    let label=wide(if english{"Import .blend..."}else{"blendを取り込み"});
    unsafe{ModifyMenuW(popup,command,0,command as usize,label.as_ptr());}
}
unsafe extern "system" fn native_menu(h:H,m:u32,w:usize,l:isize,id:usize,data:usize)->isize{
    if m==0x82 {
        unsafe{RemoveWindowSubclass(h,native_menu,id);}

    }else if m==0x117 && id==data {
        unsafe{refresh_menu_language(w as H,data as u32,is_english());}
    }else if m==0x111 && id==data && menu_command::matches(data as u32,h as isize,h as isize,m,w,l)
        && unsafe{contains_id(GetMenu(h),data as u32,0)} {
        #[cfg(not(test))]
        let result=launch_worker(h);
        #[cfg(test)]
        let result=queued_menu_tests::launch_result();
        if let Err(e)=result{show_error(h,&e);}
        return 0;
    }
    unsafe{DefSubclassProc(h,m,w,l)}
}
impl Drop for Plugin {fn drop(&mut self){self.remove_menu();}}
unsafe extern "system" fn deleting_destructor(this:*mut Plugin,flags:u32)->*mut Plugin{
    if !this.is_null(){if flags&1!=0{unsafe{drop(Box::from_raw(this));}}else{unsafe{std::ptr::drop_in_place(this);}}}this
}
unsafe extern "system" fn title(this:*mut Plugin)->*const c_char{unsafe{record(this,239,&[]);}c"Nexa Blend Importer".as_ptr()}
unsafe extern "system" fn start(this:*mut Plugin){unsafe{record(this,240,&[]);(*this).add_menu();}}
unsafe extern "system" fn stop(this:*mut Plugin){unsafe{record(this,241,&[]);(*this).remove_menu();}}
fn handle_menu_command(plugin: &Plugin, h: H, m: u32, w: usize, l: isize) -> bool {
    let id = plugin.command.load(Ordering::Relaxed);
    let own = plugin.window.load(Ordering::Relaxed);
    if !menu_command::matches(id, own, h as isize, m, w, l) {
        return false;
    }
    #[cfg(not(test))]
    let result = plugin.launch();
    #[cfg(test)]
    let result = queued_menu_tests::launch_result();
    if let Err(e) = result {
        show_error(h,&e);
    }
    true
}
// MSVC member return ABI: RCX=this, RDX=the caller's return buffer.
unsafe extern "system" fn wnd_pair(this:*mut Plugin,out:*mut Pair,h:H,m:u32,w:usize,l:isize)->*mut Pair{
    unsafe {
        record(this,242,&[h as usize as u64,m as u64,w as u64,l as u64]);
        out.write(Pair{handled:0,padding:[0;7],result:0});
    }
    let handled = handle_menu_command(unsafe { &*this }, h, m, w, l);
    unsafe { (*out).handled = u8::from(handled); }
    out
}
unsafe extern "system" fn wnd_pointer(this:*mut Plugin,param:*const Cwp){
    unsafe{record(this,243,&[param as usize as u64]);}
    if param.is_null(){return;}
    let p=unsafe{&*param};let plugin=unsafe{&*this};
    let _ = handle_menu_command(plugin, p.window, p.message, p.wparam, p.lparam);
}
unsafe fn dispatch_queued_menu(this:*mut Plugin,code:i32,param:*const c_void){
    if code < 0 || param.is_null(){return;}
    let message=unsafe{&*param.cast::<SdkMsg>()};
    let plugin=unsafe{&*this};
    let _=handle_menu_command(plugin,message.window,message.message,message.wparam,message.lparam);
}
unsafe extern "system" fn msg_proc(this:*mut Plugin,c:i32,p:*const c_void){
    unsafe{record(this,244,&[c as u32 as u64,p as usize as u64]);dispatch_queued_menu(this,c,p);}
}
unsafe extern "system" fn mouse_proc(this:*mut Plugin,w:usize,p:*const c_void){unsafe{record(this,245,&[w as u64,p as usize as u64]);}}
unsafe extern "system" fn get_msg(this:*mut Plugin,c:i32,p:*const c_void){
    unsafe{record(this,246,&[c as u32 as u64,p as usize as u64]);dispatch_queued_menu(this,c,p);}
}
unsafe extern "system" fn keyboard(this:*mut Plugin,w:usize,l:isize){unsafe{record(this,247,&[w as u64,l as u64]);}}
#[unsafe(no_mangle)] pub extern "C" fn version()->i32{3}
#[unsafe(no_mangle)] pub unsafe extern "C" fn create3(device:*mut c_void)->*mut Plugin{
    Box::into_raw(Box::new(Plugin {table:&callbacks::VTABLE,window:AtomicIsize::new(unsafe{device_window(device)} as isize),
        popup:AtomicIsize::new(0),command:AtomicU32::new(0),trace_slot:AtomicU32::new(0),trace_digest:AtomicU64::new(0)}))
}
#[unsafe(no_mangle)] pub unsafe extern "C" fn destroy3(p:*mut Plugin){if !p.is_null(){unsafe{drop(Box::from_raw(p));}}}
#[cfg(feature="abi-audit")] #[unsafe(no_mangle)] pub unsafe extern "C" fn nexa_abi_slot(p:*const Plugin)->u32{unsafe{(*p).trace_slot.load(Ordering::Relaxed)}}
#[cfg(feature="abi-audit")] #[unsafe(no_mangle)] pub unsafe extern "C" fn nexa_abi_digest(p:*const Plugin)->u64{unsafe{(*p).trace_digest.load(Ordering::Relaxed)}}
#[cfg(feature="abi-audit")] #[unsafe(no_mangle)] pub unsafe extern "C" fn nexa_abi_window(p:*mut Plugin,h:H){unsafe{(*p).window.store(h as isize,Ordering::Relaxed);}}
