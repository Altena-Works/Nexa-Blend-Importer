//! Exercise real subclass dispatch on an invisible, test-owned window only.
use super::*;
type WindowProc=unsafe extern "system" fn(H,u32,usize,isize)->isize;
#[repr(C)] struct Class {
    style:u32,proc:Option<WindowProc>,cls:i32,wnd:i32,instance:H,
    icon:H,cursor:H,background:H,menu:*const u16,name:*const u16,
}
#[link(name="user32")] unsafe extern "system" {
    fn RegisterClassW(c:*const Class)->u16;
    fn UnregisterClassW(c:*const u16,i:H)->i32;
    fn CreateWindowExW(ex:u32,c:*const u16,t:*const u16,s:u32,x:i32,y:i32,w:i32,height:i32,p:H,m:H,i:H,a:*mut c_void)->H;
    fn DefWindowProcW(h:H,m:u32,w:usize,l:isize)->isize;
    fn DestroyWindow(h:H)->i32;
    fn CreateMenu()->H;
    fn SetMenu(h:H,m:H)->i32;
    fn SendMessageW(h:H,m:u32,w:usize,l:isize)->isize;
    fn GetMenuStringW(m:H,id:u32,text:*mut u16,max:i32,flags:u32)->i32;
}
#[link(name="comctl32")] unsafe extern "system" {
    fn GetWindowSubclass(h:H,p:SubclassProc,id:usize,data:*mut usize)->i32;
}
unsafe extern "system" fn base(h:H,m:u32,w:usize,l:isize)->isize {
    if m==0x8001 || m==0x111 {return 0x7788;}
    unsafe{DefWindowProcW(h,m,w,l)}
}
struct Fixture {window:H,instance:H,class:Vec<u16>,plugin:Box<Plugin>}
#[test]
fn popup_language_tracks_open_and_preserves_command(){
    let f=Fixture::new("NexaNativeMenuLocaleTest");
    let popup=f.plugin.popup.load(Ordering::Relaxed) as H;
    let read=||{let mut text=[0u16;64];let n=unsafe{GetMenuStringW(popup,f.id() as u32,text.as_mut_ptr(),64,0)};String::from_utf16(&text[..n as usize]).unwrap()};
    unsafe{refresh_menu_language(popup,f.id() as u32,true);}
    assert_eq!(read(),"Import .blend...");
    unsafe{refresh_menu_language(popup,(f.id() ^ 1) as u32,false);}
    assert_eq!(read(),"Import .blend...");
    unsafe{SendMessageW(f.window,0x117,popup as usize,0);}
    assert_eq!(read(),"blendを取り込み");
    assert_eq!(unsafe{GetMenuItemID(popup,0)},f.id() as u32);
    assert_eq!(queued_menu_tests::launches(),0);
}
impl Fixture {
    fn new(name:&str)->Self {
        queued_menu_tests::reset_launches();
        let class=wide(name);
        let instance=unsafe{GetModuleHandleW(null())};
        let c=Class{style:0,proc:Some(base),cls:0,wnd:0,instance,icon:null_mut(),cursor:null_mut(),background:null_mut(),menu:null(),name:class.as_ptr()};
        assert_ne!(unsafe{RegisterClassW(&c)},0);
        let window=unsafe{CreateWindowExW(0,class.as_ptr(),class.as_ptr(),0,0,0,10,10,null_mut(),null_mut(),instance,null_mut())};
        assert!(!window.is_null());
        let menu=unsafe{CreateMenu()};assert!(!menu.is_null());
        assert_ne!(unsafe{SetMenu(window,menu)},0);
        let plugin=Box::new(Plugin{table:&callbacks::VTABLE,window:AtomicIsize::new(window as isize),popup:AtomicIsize::new(0),command:AtomicU32::new(0),trace_slot:AtomicU32::new(0),trace_digest:AtomicU64::new(0)});
        plugin.add_menu();
        assert_ne!(plugin.popup.load(Ordering::Relaxed),0);
        Self{window,instance,class,plugin}
    }
    fn id(&self)->usize{self.plugin.command.load(Ordering::Relaxed) as usize}
    fn attached(&self)->bool{let mut data=0;unsafe{GetWindowSubclass(self.window,native_menu,self.id(),&mut data)!=0}}
}
impl Drop for Fixture {
    fn drop(&mut self){
        self.plugin.remove_menu();
        unsafe{if IsWindow(self.window)!=0{DestroyWindow(self.window);}UnregisterClassW(self.class.as_ptr(),self.instance);}
    }
}
#[test]
fn native_command_reaches_launch_without_any_sdk_callback(){
    let f=Fixture::new("NexaNativeMenuDispatchTest");assert!(f.attached());
    assert_eq!(unsafe{SendMessageW(f.window,0x111,f.id(),0)},0);
    assert_eq!(queued_menu_tests::launches(),1);
    assert!(WORKER.lock().unwrap().is_none());
}
#[test]
fn unrelated_messages_forward_and_stop_detaches(){
    let f=Fixture::new("NexaNativeMenuDetachTest");
    let id=f.id();
    assert_eq!(unsafe{SendMessageW(f.window,0x8001,0,0)},0x7788);
    assert_eq!(unsafe{SendMessageW(f.window,0x111,f.id() ^ 1,0)},0x7788);
    assert_eq!(unsafe{SendMessageW(f.window,0x111,f.id(),1)},0x7788);
    assert_eq!(queued_menu_tests::launches(),0);
    f.plugin.remove_menu();
    let mut data=0;
    assert_eq!(unsafe{GetWindowSubclass(f.window,native_menu,id,&mut data)},0);
    assert_eq!(unsafe{SendMessageW(f.window,0x111,id,0)},0x7788);
    assert!(!handle_menu_command(&f.plugin,f.window,0x111,id,0));
    assert_eq!(queued_menu_tests::launches(),0);
}
#[test]
fn window_destruction_before_plugin_drop_is_safe(){
    let f=Fixture::new("NexaNativeMenuDestroyTest");assert!(f.attached());
    assert_ne!(unsafe{DestroyWindow(f.window)},0);
    assert_eq!(unsafe{IsWindow(f.window)},0);
    f.plugin.remove_menu();
    assert_eq!(queued_menu_tests::launches(),0);
}
