use super::*;
use std::cell::Cell;

const ID: u32 = 0xc123;
const OWN: isize = 0x0012_3456;
const OTHER: isize = 0x0065_4321;
type Hook = unsafe extern "system" fn(*mut Plugin, i32, *const c_void);
const HOOKS: [(Hook, i32); 2] = [(msg_proc, 2), (get_msg, 0)];

std::thread_local! { static LAUNCHES: Cell<usize> = const { Cell::new(0) }; }

pub(super) fn launch_result() -> Result<(), String> {
    LAUNCHES.with(|value| value.set(value.get() + 1));
    Ok(())
}
pub(super) fn reset_launches() { LAUNCHES.with(|value| value.set(0)); }
pub(super) fn launches() -> usize { LAUNCHES.with(Cell::get) }

fn plugin() -> Plugin {
    Plugin {
        table: &callbacks::VTABLE, window: AtomicIsize::new(OWN),
        popup: AtomicIsize::new(0), command: AtomicU32::new(ID),
        trace_slot: AtomicU32::new(0),
        trace_digest: AtomicU64::new(0),
    }
}
fn message(window: isize, message: u32, wparam: usize, lparam: isize) -> SdkMsg {
    SdkMsg { window: window as H, message, wparam, lparam, time: 17,
        point: MsgPoint { x: 19, y: 23 }, private: 29 }
}
fn deliver(plugin: &mut Plugin, hook: Hook, code: i32, message: &SdkMsg) {
    unsafe { hook(plugin as *mut Plugin, code, std::ptr::from_ref(message).cast::<c_void>()); }
}

#[test]
fn both_sdk_callbacks_dispatch_native_and_public_example_messages() {
    reset_launches();
    let mut plugin = plugin();
    for (hook, code) in HOOKS {
        for input in [message(OWN, 0x0111, ID as usize, 0),
            message(OTHER, 0x0202, (0x1234usize << 16) | ID as usize, OWN)] {
            let before = input;
            deliver(&mut plugin, hook, code, &input);
            assert_eq!(input, before);
        }
    }
    assert_eq!(launches(), 4);
    assert!(WORKER.lock().unwrap().is_none());
}

#[test]
fn queued_callbacks_ignore_negative_null_foreign_and_control_traffic() {
    reset_launches();
    let mut plugin = plugin();
    for (hook, code) in HOOKS {
        let invalid = std::ptr::NonNull::<SdkMsg>::dangling().as_ptr().cast::<c_void>() as *const c_void;
        unsafe {
            // Must reject the hook code before dereferencing this invalid pointer.
            hook(&mut plugin, -1, invalid);
            hook(&mut plugin, code, null());
        }
        for input in [
            message(OTHER, 0x0111, ID as usize, 0),
            message(OWN, 0x0111, (ID ^ 1) as usize, 0),
            message(OWN, 0x0111, (1usize << 16) | ID as usize, 0),
            message(OWN, 0x0111, ID as usize, OTHER),
            message(OTHER, 0x0202, ID as usize, OTHER),
            message(OWN, 0x0202, 1, 0x0028_0014),
            message(OWN, 0x0200, ID as usize, OWN),
            message(OWN, 0x0100, ID as usize, OWN),
            message(OWN, 0x0126, 0, OTHER),
        ] {
            let before = input;
            deliver(&mut plugin, hook, code, &input);
            assert_eq!(input, before);
        }
    }
    assert_eq!(launches(), 0);
    assert!(WORKER.lock().unwrap().is_none());
}

#[test]
fn sent_and_queued_duplicates_share_the_same_launch_boundary() {
    reset_launches();
    let mut plugin = plugin();
    let this = &mut plugin as *mut Plugin;
    let mut output = Pair { handled: 0xff, padding: [0xff; 7], result: -1 };
    let expected = &mut output as *mut Pair;
    let returned = unsafe { wnd_pair(this, expected, OWN as H, 0x0111, ID as usize, 0) };
    assert_eq!(returned, expected);
    assert_eq!(output.handled, 1);
    assert_eq!(output.padding, [0; 7]);
    assert_eq!(output.result, 0);
    let sent = Cwp { lparam: 0, wparam: ID as usize, message: 0x0111, window: OWN as H };
    unsafe { wnd_pointer(this, &sent); }
    let queued = message(OWN, 0x0111, ID as usize, 0);
    for (hook, code) in HOOKS { deliver(&mut plugin, hook, code, &queued); }
    assert_eq!(launches(), 4);
    assert_eq!(plugin.window.load(Ordering::Relaxed), OWN);
    assert_eq!(plugin.command.load(Ordering::Relaxed), ID);
    assert!(WORKER.lock().unwrap().is_none());
}
