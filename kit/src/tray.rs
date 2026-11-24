//! Tray icons on a plain Win32 message loop (DESIGN.md 6) – no egui, no GPU, 0 % CPU while idle. An icon is
//! either the one compiled into the exe or drawn from text with GDI (the temperature numbers).

use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU32, Ordering};

use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::ipc::{sandbox, wide};

const WM_TRAY: u32 = WM_APP + 1;
const WM_WAKE: u32 = WM_APP + 2;
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;

pub enum Event {
    /// A click on the icon with this uid.
    LeftClick(u32),
    /// The user wants the menu; return its entries. The chosen id comes back as `Chosen`.
    Menu,
    Chosen(u32),
    /// `Waker::wake` was called from another thread – or Explorer restarted and the icons have to be set again.
    Wake,
    /// Windows ends the session (shutdown, restart, log off) – the process ends right after; what it holds must be
    /// saved now.
    EndSession,
}

pub enum Item {
    Entry { id: u32, label: String, enabled: bool },
    Sub { label: String, items: Vec<Item> },
    Separator,
}

impl Item {
    pub fn entry(id: u32, label: impl Into<String>) -> Item {
        Item::Entry { id, label: label.into(), enabled: true }
    }
    /// A line that only shows something.
    pub fn info(label: impl Into<String>) -> Item {
        Item::Entry { id: 0, label: label.into(), enabled: false }
    }
}

/// What an icon shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Icon<'a> {
    /// The icon compiled into the exe (`kit::build::manifest()`).
    Program,
    /// 1–3 characters in a colour (0x00BBGGRR) on transparent ground, optionally with a stripe under them
    /// (0x00RRGGBB).
    Text(&'a str, u32, Option<u32>),
}

/// Wakes the tray thread from any thread (posts a message; the loop sleeps otherwise).
#[derive(Clone, Copy)]
pub struct Waker(isize);
impl Waker {
    pub fn wake(&self) {
        unsafe { PostMessageW(self.0 as HWND, WM_WAKE, 0, 0) };
    }
}

pub struct Tray {
    hwnd: HWND,
    /// The icons Windows shows for us: uid, and what it shows (icon and tooltip, as text to compare).
    icons: Vec<(u32, String)>,
}

type Handler = Box<dyn FnMut(&mut Tray, Event) -> Vec<Item>>;
thread_local! {
    static HANDLER: std::cell::RefCell<Option<(Tray, Handler)>> = const { std::cell::RefCell::new(None) };
}

fn nid(hwnd: HWND, uid: u32) -> NOTIFYICONDATAW {
    let mut n: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    n.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    n.hWnd = hwnd;
    n.uID = uid;
    n
}

fn copy_tip(dst: &mut [u16], s: &str) {
    let w: Vec<u16> = s.encode_utf16().take(dst.len() - 1).collect();
    dst[..w.len()].copy_from_slice(&w);
    dst[w.len()] = 0;
}

impl Tray {
    /// Shows (or updates) icon `uid`. The uid is the icon's fixed identity – Windows remembers "show this icon"
    /// per exe path and uid (DESIGN.md 6). The tooltip holds at most 127 characters. Setting what the icon shows
    /// already costs nothing, so a caller may simply set everything again whenever something changed.
    pub fn set_icon(&mut self, uid: u32, icon: Icon, tip: &str) {
        if !sandbox().is_empty() {
            return; // tests show nothing
        }
        let shows = format!("{icon:?}\n{tip}");
        let known = self.icons.iter().position(|(u, _)| *u == uid);
        if known.is_some_and(|i| self.icons[i].1 == shows) {
            return;
        }
        let mut n = nid(self.hwnd, uid);
        n.uFlags = NIF_ICON | NIF_TIP | NIF_MESSAGE | NIF_SHOWTIP;
        n.uCallbackMessage = WM_TRAY;
        n.hIcon = match icon {
            Icon::Program => program_icon(unsafe { GetSystemMetrics(SM_CXSMICON) }),
            Icon::Text(text, color, stripe) => text_icon(text, color, stripe),
        };
        copy_tip(&mut n.szTip, tip);
        unsafe {
            match known {
                None => {
                    if Shell_NotifyIconW(NIM_ADD, &n) != 0 {
                        n.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                        Shell_NotifyIconW(NIM_SETVERSION, &n);
                        self.icons.push((uid, shows));
                    }
                }
                Some(i) => {
                    Shell_NotifyIconW(NIM_MODIFY, &n);
                    self.icons[i].1 = shows;
                }
            }
            // Windows keeps its own copy.
            DestroyIcon(n.hIcon);
        }
    }

    pub fn remove_icon(&mut self, uid: u32) {
        if !self.icons.iter().any(|(u, _)| *u == uid) {
            return;
        }
        let n = nid(self.hwnd, uid);
        unsafe { Shell_NotifyIconW(NIM_DELETE, &n) };
        self.icons.retain(|(u, _)| *u != uid);
    }

    /// Removes every icon and ends `run_loop`.
    pub fn quit(&mut self) {
        let uids: Vec<u32> = self.icons.iter().map(|(u, _)| *u).collect();
        for uid in uids {
            self.remove_icon(uid);
        }
        unsafe { PostQuitMessage(0) };
    }
}

/// The exe's own icon (resource 1) in the given size – for the tray and for a window's title bar.
pub fn program_icon(size: i32) -> HICON {
    unsafe {
        let icon = LoadImageW(GetModuleHandleW(null()), 1 as *const u16, IMAGE_ICON, size, size, LR_DEFAULTCOLOR);
        if icon.is_null() { LoadIconW(null_mut(), IDI_APPLICATION) } else { icon }
    }
}

/// A 32×32 icon showing `text` (1–3 characters) in `color` (0x00BBGGRR) on transparent ground.
pub fn text_icon(text: &str, color: u32, stripe: Option<u32>) -> HICON {
    unsafe {
        let size = 32;
        let screen = GetDC(null_mut());
        let dc = CreateCompatibleDC(screen);
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = size;
        bi.bmiHeader.biHeight = -size;
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        let mut bits: *mut core::ffi::c_void = null_mut();
        let color_bmp = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        let old = SelectObject(dc, color_bmp);
        let px = std::slice::from_raw_parts_mut(bits as *mut u32, (size * size) as usize);
        px.fill(0);
        let h = if text.chars().count() <= 2 { -30 } else { -22 };
        let face = wide("Segoe UI");
        let font = CreateFontW(h, 0, 0, 0, FW_BOLD as i32, 0, 0, 0, DEFAULT_CHARSET as u32, 0, 0, ANTIALIASED_QUALITY as u32, 0, face.as_ptr());
        let oldf = SelectObject(dc, font);
        SetBkMode(dc, TRANSPARENT as i32);
        // Draw white, then use the brightness as alpha and colour the pixel: smooth edges without ClearType fringes.
        SetTextColor(dc, 0x00FFFFFF);
        // With a stripe the number moves up a little to make room for it.
        let mut r = RECT { left: 0, top: 0, right: size, bottom: if stripe.is_some() { size - 4 } else { size } };
        let w: Vec<u16> = text.encode_utf16().collect();
        DrawTextW(dc, w.as_ptr(), w.len() as i32, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOCLIP);
        GdiFlush();
        let (cr, cg, cb) = (color & 0xff, (color >> 8) & 0xff, (color >> 16) & 0xff);
        for p in px.iter_mut() {
            let a = (*p & 0xff).max((*p >> 8) & 0xff).max((*p >> 16) & 0xff);
            *p = (a << 24) | ((cr * a / 255) << 16) | ((cg * a / 255) << 8) | (cb * a / 255);
        }
        // The stripe: the bottom 4 rows, with a pixel of room on each side and rounded ends (BGRA in memory).
        if let Some(rgb) = stripe {
            for y in size - 4..size {
                let inset = if y == size - 4 || y == size - 1 { 5 } else { 4 };
                for x in inset..size - inset {
                    px[(y * size + x) as usize] = 0xff00_0000 | (rgb & 0x00ff_ffff);
                }
            }
        }
        SelectObject(dc, oldf);
        DeleteObject(font);
        SelectObject(dc, old);
        let mask = CreateBitmap(size, size, 1, 1, null());
        let info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color_bmp };
        let icon = CreateIconIndirect(&info);
        DeleteObject(mask);
        DeleteObject(color_bmp);
        DeleteDC(dc);
        ReleaseDC(null_mut(), screen);
        icon
    }
}

fn build_menu(items: &[Item]) -> HMENU {
    unsafe {
        let m = CreatePopupMenu();
        for it in items {
            match it {
                Item::Entry { id, label, enabled } => {
                    let w = wide(label);
                    let flags = MF_STRING | if *enabled { 0 } else { MF_GRAYED };
                    AppendMenuW(m, flags, *id as usize, w.as_ptr());
                }
                Item::Sub { label, items } => {
                    let w = wide(label);
                    AppendMenuW(m, MF_POPUP, build_menu(items) as usize, w.as_ptr());
                }
                Item::Separator => {
                    AppendMenuW(m, MF_SEPARATOR, 0, null());
                }
            }
        }
        m
    }
}

fn dispatch(ev: Event) -> Vec<Item> {
    HANDLER.with(|h| match h.try_borrow_mut() {
        Ok(mut h) => match h.as_mut() {
            Some((tray, f)) => f(tray, ev),
            None => vec![],
        },
        // The handler is busy (a message arrived while it showed something modal): this one is dropped.
        Err(_) => vec![],
    })
}

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY => {
            // With NOTIFYICON_VERSION_4 the low word of lParam is the event.
            match (lp & 0xffff) as u32 {
                // A click with the mouse, or Enter on the icon when it was reached with the keyboard.
                WM_LBUTTONUP | NIN_KEYSELECT => {
                    // …and the high word the icon's uid.
                    dispatch(Event::LeftClick(((lp >> 16) & 0xffff) as u32));
                }
                WM_CONTEXTMENU => {
                    let items = dispatch(Event::Menu);
                    let menu = build_menu(&items);
                    let mut pt = POINT { x: 0, y: 0 };
                    unsafe {
                        GetCursorPos(&mut pt);
                        // Without this the menu would not close on a click beside it.
                        SetForegroundWindow(hwnd);
                        let id = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, pt.x, pt.y, 0, hwnd, null());
                        DestroyMenu(menu);
                        if id > 0 {
                            dispatch(Event::Chosen(id as u32));
                        }
                    }
                }
                _ => {}
            }
            0
        }
        WM_WAKE => {
            dispatch(Event::Wake);
            0
        }
        // Yes, the session may end – nothing here asks the user.
        WM_QUERYENDSESSION => 1,
        WM_ENDSESSION => {
            if wp != 0 {
                dispatch(Event::EndSession);
            }
            0
        }
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            // Explorer restarted: every icon has to be added again.
            HANDLER.with(|h| {
                if let Ok(mut h) = h.try_borrow_mut() {
                    if let Some((tray, _)) = h.as_mut() {
                        tray.icons.clear();
                    }
                }
            });
            dispatch(Event::Wake);
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// The hidden window of this process' tray thread, for `forget`.
static WINDOW: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// Removes icon `uid` at once, from any thread – for a process that is about to end: an icon that is not removed
/// stays in the taskbar until the mouse moves over it.
pub fn forget(uid: u32) {
    let hwnd = WINDOW.load(Ordering::Relaxed);
    if hwnd != 0 {
        let n = nid(hwnd as HWND, uid);
        unsafe { Shell_NotifyIconW(NIM_DELETE, &n) };
    }
}

/// Creates the hidden window and returns a waker; then call `run_loop` on the same thread. Split in two so the
/// caller can hand the waker to other threads before the loop starts.
pub fn create(handler: impl FnMut(&mut Tray, Event) -> Vec<Item> + 'static) -> Waker {
    unsafe {
        let inst = GetModuleHandleW(null());
        let class = wide("WindowsToolboxTray");
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: inst, lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
        RegisterClassW(&wc);
        TASKBAR_CREATED.store(RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()), Ordering::Relaxed);
        // A normal (never shown) top-level window, not a message-only one: only those receive TaskbarCreated.
        let hwnd = CreateWindowExW(0, class.as_ptr(), class.as_ptr(), 0, 0, 0, 0, 0, null_mut(), null_mut(), inst, null());
        WINDOW.store(hwnd as isize, Ordering::Relaxed);
        HANDLER.with(|h| *h.borrow_mut() = Some((Tray { hwnd, icons: vec![] }, Box::new(handler))));
        Waker(hwnd as isize)
    }
}

/// Sleeps in the Windows message loop until `Tray::quit`.
pub fn run_loop() {
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
