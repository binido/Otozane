//! macOS menu bar icon: shows the service state, switches the processing
//! mode, shows the window and quits. Closing the window only hides it.

use crate::client::Mode;
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::NSApplication;
use objc2_foundation::{NSAppleEventDescriptor, NSAppleEventManager};
use otozane_core::protocol::{CaptureState, Client, Status};
use std::cell::RefCell;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

struct Items {
    _icon: TrayIcon,
    _reopen: Retained<ReopenHandler>,
    state: MenuItem,
    modes: Vec<(Mode, CheckMenuItem)>,
    show: MenuItem,
    quit: MenuItem,
    /// Mode last shown, `None` while the service is not reachable.
    mode: Option<Mode>,
}

thread_local! {
    // Menu items are not Send, AppKit calls back on the main thread only.
    static ITEMS: RefCell<Option<Items>> = const { RefCell::new(None) };
}

/// Adds the icon to the menu bar. Call on the main thread once the event loop runs.
pub fn spawn() -> Result<(), String> {
    let state = MenuItem::new("Connecting", false, None);
    let modes: Vec<_> = Mode::ALL
        .iter()
        .map(|(m, label)| (*m, CheckMenuItem::new(*label, false, false, None)))
        .collect();
    let show = MenuItem::new("Show Otozane", true, None);
    let quit = MenuItem::new("Quit Otozane", true, None);
    let menu = Menu::new();
    menu.append(&state).map_err(|e| e.to_string())?;
    menu.append(&PredefinedMenuItem::separator())
        .map_err(|e| e.to_string())?;
    for (_, item) in &modes {
        menu.append(item).map_err(|e| e.to_string())?;
    }
    menu.append(&PredefinedMenuItem::separator())
        .map_err(|e| e.to_string())?;
    menu.append(&show).map_err(|e| e.to_string())?;
    menu.append(&quit).map_err(|e| e.to_string())?;

    // Black on transparent, macOS recolours it for a light or dark menu bar.
    let png = include_bytes!("../assets/menubar.png");
    let icon = eframe::icon_data::from_png_bytes(png).map_err(|e| e.to_string())?;
    let icon = Icon::from_rgba(icon.rgba, icon.width, icon.height).map_err(|e| e.to_string())?;
    let tray = TrayIconBuilder::new()
        .with_icon_templated(icon)
        .with_tooltip("Otozane")
        .with_menu(Box::new(menu))
        .build()
        .map_err(|e| e.to_string())?;
    let reopen = ReopenHandler::install();
    ITEMS.with_borrow_mut(|i| {
        *i = Some(Items {
            _icon: tray,
            _reopen: reopen,
            state,
            modes,
            show,
            quit,
            mode: None,
        })
    });
    MenuEvent::set_event_handler(Some(|e: MenuEvent| {
        ITEMS.with_borrow_mut(|i| {
            if let Some(i) = i {
                i.clicked(&e);
            }
        })
    }));
    Ok(())
}

impl Items {
    fn clicked(&mut self, e: &MenuEvent) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        if e.id == *self.show.id() {
            show(&app);
        } else if e.id == *self.quit.id() {
            app.terminate(None);
        } else if let Some(&(mode, _)) = self.modes.iter().find(|(_, i)| e.id == *i.id()) {
            let from = self.mode.unwrap_or(mode);
            self.set_mode(Some(mode));
            // Off the main thread, the socket round trip can block.
            std::thread::spawn(move || {
                if let Ok(mut c) = Client::connect() {
                    for req in mode.requests(from) {
                        let _ = c.request(&req);
                    }
                }
            });
        }
    }

    fn set_mode(&mut self, mode: Option<Mode>) {
        self.mode = mode;
        // AppKit toggles a clicked check item itself, so every item is set.
        for (m, item) in &self.modes {
            item.set_checked(Some(*m) == mode);
            item.set_enabled(mode.is_some());
        }
    }
}

define_class!(
    /// Receives the reopen Apple event, sent when the Dock icon of the running app is clicked.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OtozaneReopenHandler"]
    struct ReopenHandler;

    impl ReopenHandler {
        #[unsafe(method(handleReopen:withReplyEvent:))]
        fn handle_reopen(&self, _event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
            show(&NSApplication::sharedApplication(self.mtm()));
        }
    }
);

impl ReopenHandler {
    /// Replaces the reopen handler. winit's app delegate does not answer
    /// reopen, so without it a Dock click leaves a closed window closed.
    fn install() -> Retained<Self> {
        // kCoreEventClass ('aevt') and kAEReopenApplication ('rapp').
        const CORE_EVENT_CLASS: u32 = u32::from_be_bytes(*b"aevt");
        const REOPEN_APPLICATION: u32 = u32::from_be_bytes(*b"rapp");
        let mtm = MainThreadMarker::new().expect("menu bar is set up on the main thread");
        let handler: Retained<Self> = unsafe { msg_send![Self::alloc(mtm), init] };
        // SAFETY: the selector matches the method above, the event manager
        // does not retain the handler, so Items keeps it alive.
        unsafe {
            NSAppleEventManager::sharedAppleEventManager()
                .setEventHandler_andSelector_forEventClass_andEventID(
                    &handler,
                    sel!(handleReopen:withReplyEvent:),
                    CORE_EVENT_CLASS,
                    REOPEN_APPLICATION,
                );
        }
        handler
    }
}

fn show(app: &NSApplication) {
    for w in app.windows() {
        w.makeKeyAndOrderFront(None);
    }
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
}

/// Keeps the menu in step with the window's status and turns a window close
/// into hiding the app. Call every frame.
// ponytail: the menu refreshes only while the window draws frames, a mode
// changed by another client while hidden shows up on the next show.
pub fn on_frame(ctx: &egui::Context, status: Option<&Status>) {
    if ctx.input(|i| i.viewport().close_requested())
        && let Some(mtm) = MainThreadMarker::new()
    {
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        // Only the window goes away. A hidden app would be unhidden by the
        // click on its menu bar icon, which then closes the menu.
        for w in NSApplication::sharedApplication(mtm).windows() {
            w.orderOut(None);
        }
    }
    ITEMS.with_borrow_mut(|i| {
        let Some(i) = i else { return };
        let (text, mode) = match status {
            Some(st) => (label(st), Some(Mode::of(st))),
            None => ("Service stopped", None),
        };
        i.state.set_text(text);
        if mode != i.mode {
            i.set_mode(mode);
        }
    });
}

fn label(st: &Status) -> &'static str {
    match st.capture {
        CaptureState::Running(_) if !st.processing_enabled => "Bypassed",
        CaptureState::Running(_) => "Processing",
        CaptureState::Muted => "Muted",
        CaptureState::Error(_) => "Audio error",
        _ => "Waiting for input",
    }
}
