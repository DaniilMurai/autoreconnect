use std::time::Duration;
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoopBuilder};
#[cfg(target_os = "macos")]
use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};

enum Ev { Status(String) }

fn main() {
    let mut el = EventLoopBuilder::<Ev>::with_user_event().build();
    #[cfg(target_os = "macos")]
    el.set_activation_policy(ActivationPolicy::Accessory);
    let proxy = el.create_proxy();

    let menu = Menu::new();
    let status = MenuItem::new("Status: starting", false, None);
    let pause = CheckMenuItem::new("Pause", true, false, None);
    let quit = MenuItem::new("Quit", true, None);
    menu.append(&status).unwrap();
    menu.append(&pause).unwrap();
    menu.append(&PredefinedMenuItem::separator()).unwrap();
    for i in 1..=5 { menu.append(&MenuItem::new(format!("12:0{i} log entry {i}"), false, None)).unwrap(); }
    menu.append(&PredefinedMenuItem::separator()).unwrap();
    menu.append(&quit).unwrap();

    let mut rgba = Vec::new();
    for y in 0..32 { for x in 0..32 {
        let d = (x as i32 - 16).pow(2) + (y as i32 - 16).pow(2);
        if d < 200 { rgba.extend_from_slice(&[0, 160, 255, 255]); } else { rgba.extend_from_slice(&[0, 0, 0, 0]); }
    }}
    let icon = Icon::from_rgba(rgba, 32, 32).unwrap();
    let _tray = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("spike").with_icon(icon).build().unwrap();

    std::thread::spawn(move || {
        let mut n = 0;
        loop { std::thread::sleep(Duration::from_secs(5)); n += 1;
            let _ = proxy.send_event(Ev::Status(format!("Status: tick {n}"))); }
    });
    let quit_id = quit.id().clone();
    el.run(move |event, _, cf| {
        *cf = ControlFlow::Wait;
        if let Event::UserEvent(Ev::Status(s)) = event { status.set_text(&s); eprintln!("{s}"); }
        if let Ok(e) = MenuEvent::receiver().try_recv() {
            if e.id == quit_id { *cf = ControlFlow::Exit; }
            else { eprintln!("pause checked={}", pause.is_checked()); }
        }
    });
}
