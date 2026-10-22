//! Native messages on a hidden, test-owned window; no MMD or user input.
use super::*;

#[link(name="user32")] unsafe extern "system" {
    fn PeekMessageW(message:*mut Msg,window:H,first:u32,last:u32,remove:u32)->i32;
}

#[test]
fn modal_reentry_preserves_events_and_defers_close_until_borrow_ends() {
    let context=Box::new(GuiContext {state:RefCell::new(State {
        language:Language::English,input:PathBuf::new(),details:false,busy:false,
        closing:false,cancelled:Arc::new(AtomicBool::new(false)),receiver:None,
        worker:None,options:None,
    }),close_requested:Cell::new(false)});
    let class=wide(&format!("NexaGuiReentryTest-{}",std::process::id()));
    let instance=unsafe{GetModuleHandleW(null())};
    let descriptor=Class{style:0,proc:Some(procedure),cls:0,wnd:0,instance,
        icon:null_mut(),cursor:null_mut(),background:null_mut(),menu:null(),name:class.as_ptr()};
    assert_ne!(unsafe{RegisterClassW(&descriptor)},0);
    let window=unsafe{CreateWindowExW(0,class.as_ptr(),class.as_ptr(),0,0,0,10,10,
        null_mut(),null_mut(),instance,null_mut())};
    assert!(!window.is_null());
    unsafe{SetWindowLongPtrW(window,-21,(&*context as *const GuiContext)as isize);}
    let (tx,rx)=mpsc::channel();
    let mut active=context.state.borrow_mut();
    active.receiver=Some(rx);
    tx.send(Event::Prepared(Err("preserved test event".into()))).unwrap();
    // These are synchronous nested messages, like a native modal loop.
    assert_eq!(unsafe{SendMessageW(window,0x113,1,0)},0);
    assert_eq!(unsafe{SendMessageW(window,0x111,RUN as usize,0)},0);
    assert!(matches!(active.receiver.as_ref().unwrap().try_recv(),
        Ok(Event::Prepared(Err(error))) if error=="preserved test event"));
    active.receiver=None;
    assert_eq!(unsafe{SendMessageW(window,0x10,0,0)},0);
    assert!(context.close_requested.get());
    assert_ne!(unsafe{IsWindow(window)},0);
    drop(active);
    // A normal dispatch after the modal borrow ends posts the deferred close.
    unsafe{SendMessageW(window,0x113,1,0);}
    let mut message:Msg=unsafe{zeroed()};
    assert_ne!(unsafe{PeekMessageW(&mut message,window,0x10,0x10,1)},0);
    assert_eq!(message.message,0x10);
    unsafe{DispatchMessageW(&message);}
    assert_eq!(unsafe{IsWindow(window)},0);
    assert!(!context.close_requested.get());
    assert_ne!(unsafe{UnregisterClassW(class.as_ptr(),instance)},0);
    unsafe{PeekMessageW(&mut message,null_mut(),0x12,0x12,1);}
}
