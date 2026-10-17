use crate::config::{self, Config};
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use windows_sys::w;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, EndPaint, FillRect, GetDC, GetDeviceCaps,
    GetStockObject, GetSysColorBrush, ReleaseDC, SetBkColor, SetTextColor, COLOR_BTNFACE, PAINTSTRUCT,
    WHITE_BRUSH,
};
use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::Dialogs::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows_sys::Win32::UI::Shell::{ExtractIconW, ShellExecuteW};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// Posted to the owner window after a successful save.
pub const WM_RELOAD: u32 = WM_APP + 2;
const WM_CHANGED: u32 = WM_USER + 1;

const OK: i32 = 1; // IDOK / IDCANCEL so IsDialogMessage maps Enter/Esc
const CANCEL: i32 = 2;
const PATH: i32 = 101;
const BROWSE: i32 = 102;
const ENABLED: i32 = 103;
const LIMIT_ON: i32 = 104;
const LIMIT: i32 = 105;
const INTERVAL: i32 = 106;
const START: i32 = 107;
const EXT_ON: i32 = 108;
const EXT_BTN: i32 = 109;
const ASTART: i32 = 110;
const NAME: i32 = 111;
const COMPANY: i32 = 112;
const ICON: i32 = 113;
const USAGE: i32 = 114;
const COUNT: i32 = 115;
const EXT_USE: i32 = 116;
const STATUS: i32 = 117;
const HINT: i32 = 200; // gray text
const TITLE: i32 = 201;
const M_OPEN: i32 = 301;
const M_CLOSE: i32 = 302;
const M_ABOUT: i32 = 303;
const X_EDIT: i32 = 401;

// Layout in 96-dpi units; scaled by `sc`.
const W: i32 = 500;
const H: i32 = 612;
const HEADER_LINE: i32 = 92;
const BAR: i32 = 540;
const STATUS_LINE: i32 = 586;

thread_local! {
    static WIN: Cell<HWND> = const { Cell::new(null_mut()) };
    static XWIN: Cell<HWND> = const { Cell::new(null_mut()) };
    static CTX: RefCell<Option<(Config, HWND)>> = const { RefCell::new(None) };
    static EXT_LIST: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static DPI: Cell<i32> = const { Cell::new(96) };
    static FONTS: Cell<[usize; 3]> = const { Cell::new([0; 3]) }; // [body, large, italic]
    static LOADING: Cell<bool> = const { Cell::new(true) };
    static ICONS: Cell<[usize; 2]> = const { Cell::new([0; 2]) }; // [browser icon, header icon]
    static REGISTERED: Cell<bool> = const { Cell::new(false) };
}

pub fn hwnd() -> HWND {
    WIN.with(Cell::get)
}

/// Lets Tab/Enter/Esc work in our windows; called from the main message loop.
pub fn pre_translate(m: &MSG) -> bool {
    for h in [XWIN.with(Cell::get), hwnd()] {
        if !h.is_null() && unsafe { IsDialogMessageW(h, m) } != 0 {
            return true;
        }
    }
    false
}

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn sc(v: i32) -> i32 {
    v * DPI.with(Cell::get) / 96
}

unsafe fn ctl(p: HWND, class: *const u16, text: &str, style: u32, ex: u32, r: [i32; 4], id: i32, font: usize) -> HWND {
    let [x, y, cw, ch] = r.map(sc);
    let h = CreateWindowExW(
        ex, class, w(text).as_ptr(), WS_CHILD | WS_VISIBLE | style,
        x, y, cw, ch, p, id as usize as HMENU, GetModuleHandleW(null()), null(),
    );
    SendMessageW(h, WM_SETFONT, FONTS.with(Cell::get)[font], 1);
    h
}

unsafe fn text(h: HWND, id: i32) -> String {
    let mut b = [0u16; 1040];
    let n = GetWindowTextW(GetDlgItem(h, id), b.as_mut_ptr(), b.len() as i32);
    String::from_utf16_lossy(&b[..n.max(0) as usize])
}

/// Avoids redundant repaints (the live stats refresh every second).
unsafe fn set_text(h: HWND, id: i32, s: &str) {
    if text(h, id) != s {
        SetWindowTextW(GetDlgItem(h, id), w(s).as_ptr());
    }
}

unsafe fn checked(h: HWND, id: i32) -> bool {
    SendMessageW(GetDlgItem(h, id), BM_GETCHECK, 0, 0) == 1 // BST_CHECKED
}

fn exe_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// (product name + version, company) from the exe's version resource.
unsafe fn version_info(path: &str) -> Option<(String, String, String)> {
    let p = w(path);
    let n = GetFileVersionInfoSizeW(p.as_ptr(), null_mut());
    if n == 0 {
        return None;
    }
    let mut buf = vec![0u8; n as usize];
    if GetFileVersionInfoW(p.as_ptr(), 0, n, buf.as_mut_ptr().cast()) == 0 {
        return None;
    }
    let query = |sub: &str| -> Option<(*mut c_void, u32)> {
        let (mut ptr, mut len) = (null_mut(), 0u32);
        (VerQueryValueW(buf.as_ptr().cast(), w(sub).as_ptr(), &mut ptr, &mut len) != 0 && len > 0).then_some((ptr, len))
    };
    let (lang, cp) = match query("\\VarFileInfo\\Translation") {
        Some((p, l)) if l >= 4 => {
            let t = *(p as *const [u16; 2]);
            (t[0], t[1])
        }
        _ => (0x409, 0x4b0),
    };
    let field = |k: &str| {
        query(&format!("\\StringFileInfo\\{lang:04x}{cp:04x}\\{k}")).map(|(p, l)| {
            String::from_utf16_lossy(std::slice::from_raw_parts(p as *const u16, (l as usize).saturating_sub(1)))
        })
    };
    let name = field("ProductName").or_else(|| field("FileDescription")).unwrap_or_default();
    Some((name.trim().to_string(), field("ProductVersion").unwrap_or_default(), field("CompanyName").unwrap_or_default()))
}

/// Fills the "Load Browser" box for the exe path currently in PATH.
unsafe fn refresh_browser(h: HWND) {
    let path = text(h, PATH);
    let exe = exe_name(&path).to_string();
    let (product, version, company) = version_info(&path).unwrap_or_else(|| (exe.clone(), String::new(), String::new()));
    set_text(h, NAME, &format!("{product} {version}").trim_end().to_string());
    set_text(h, COMPANY, &company);
    set_text(h, HINT_PATH_LABEL, &format!("Executable Path ({exe}):"));
    set_text(h, START, &format!("Start {product} when edgemin-lite starts"));
    let old = ICONS.with(Cell::get)[0];
    let icon = ExtractIconW(GetModuleHandleW(null()), w(&path).as_ptr(), 0);
    let icon = if (icon as usize) > 1 { icon } else { null_mut() };
    SendMessageW(GetDlgItem(h, ICON), STM_SETICON, icon as WPARAM, 0);
    ICONS.with(|i| {
        let mut a = i.get();
        a[0] = icon as usize;
        i.set(a);
    });
    if old != 0 {
        DestroyIcon(old as _);
    }
}
const HINT_PATH_LABEL: i32 = 118;

fn mb(b: u64) -> String {
    format!("{:.2} MB", b as f64 / 1048576.0)
}

unsafe fn refresh_live(h: HWND) {
    let l = crate::live();
    set_text(h, USAGE, &format!("{} / {}", mb(l.core), mb(l.peak)));
    set_text(h, COUNT, &l.procs.to_string());
    set_text(h, EXT_USE, &format!("{} / {}", mb(l.ext), mb(l.ext_peak)));
}

unsafe fn status(h: HWND) {
    let i = text(h, INTERVAL);
    let s = if !checked(h, ENABLED) {
        "Memory reduction is turned off.".to_string()
    } else if checked(h, LIMIT_ON) {
        format!("Reduce every {i} milliseconds if usage over {} MB.", text(h, LIMIT))
    } else {
        format!("Reduce every {i} milliseconds.")
    };
    set_text(h, STATUS, &s);
}

unsafe fn sync(h: HWND) {
    EnableWindow(GetDlgItem(h, INTERVAL), checked(h, ENABLED) as i32);
    EnableWindow(GetDlgItem(h, LIMIT_ON), checked(h, ENABLED) as i32);
    EnableWindow(GetDlgItem(h, LIMIT), (checked(h, ENABLED) && checked(h, LIMIT_ON)) as i32);
    EnableWindow(GetDlgItem(h, EXT_BTN), checked(h, EXT_ON) as i32);
}

unsafe fn combo(h: HWND, id: i32, items: &[u32], value: u64, r: [i32; 4]) {
    let c = ctl(h, w!("COMBOBOX"), "", CBS_DROPDOWN as u32 | WS_VSCROLL | WS_TABSTOP, 0, r, id, 0);
    for v in items {
        SendMessageW(c, CB_ADDSTRING, 0, w(&v.to_string()).as_ptr() as LPARAM);
    }
    SetWindowTextW(c, w(&value.to_string()).as_ptr());
}

unsafe fn build(h: HWND, c: &Config) {
    let check = |id, t: &str, on: bool, r| {
        ctl(h, w!("BUTTON"), t, BS_AUTOCHECKBOX as u32 | WS_TABSTOP, 0, r, id, 0);
        SendMessageW(GetDlgItem(h, id), BM_SETCHECK, on as WPARAM, 0);
    };
    let label = |t: &str, id, style, r| ctl(h, w!("STATIC"), t, style, 0, r, id, 0);
    let group = |t: &str, r| ctl(h, w!("BUTTON"), t, BS_GROUPBOX as u32, 0, r, 0, 2);
    const RIGHT: u32 = 2; // SS_RIGHT
    const ICON_STYLE: u32 = 3; // SS_ICON

    EXT_LIST.with(|e| *e.borrow_mut() = c.extended.clone());

    let app_icon = ctl(h, w!("STATIC"), "", ICON_STYLE, 0, [20, 16, 48, 48], 0, 0);
    let hdr = crate::app_icon(sc(48));
    SendMessageW(app_icon, STM_SETICON, hdr as WPARAM, 0);
    ICONS.with(|i| {
        let mut a = i.get();
        a[1] = hdr as usize;
        i.set(a);
    });
    let title = label("edgemin-lite", TITLE, 0, [84, 14, 396, 28]);
    SendMessageW(title, WM_SETFONT, FONTS.with(Cell::get)[1], 1);
    label(
        "Select the browser executable. edgemin-lite detects its processes from the file name and empties their working set when memory use goes over the limit.",
        0, 0, [84, 42, 396, 44],
    );

    group("Load Browser", [10, 102, 480, 230]);
    label("", ICON, ICON_STYLE, [24, 126, 40, 40]);
    label("", NAME, 0, [76, 126, 400, 20]);
    label("", COMPANY, 0, [76, 148, 400, 18]);
    label("", HINT_PATH_LABEL, 0, [24, 176, 450, 16]);
    ctl(h, w!("STATIC"), &c.browser_path, 0, 0, [24, 194, 330, 36], PATH, 0);
    label("Usage | Peak:", HINT, RIGHT, [24, 246, 120, 18]);
    label("", USAGE, 0, [152, 246, 200, 18]);
    label("Process Count:", HINT, RIGHT, [24, 268, 120, 18]);
    label("", COUNT, 0, [152, 268, 200, 18]);
    label("Extended Usage:", HINT, RIGHT, [24, 296, 120, 18]);
    label("", EXT_USE, 0, [152, 296, 200, 18]);
    ctl(h, w!("BUTTON"), "Browse", WS_TABSTOP, 0, [372, 234, 100, 30], BROWSE, 0);

    group("Options", [10, 344, 480, 184]);
    check(ENABLED, "Reduce memory every :", c.enabled, [24, 368, 190, 20]);
    combo(h, INTERVAL, &[100, 250, 500, 1000, 2000, 5000, 10000], c.interval_ms as u64, [220, 364, 110, 200]);
    label("milliseconds", 0, 0, [340, 368, 120, 18]);
    check(LIMIT_ON, "Only reduce memory if usage is over :", c.limit_enabled, [24, 398, 240, 20]);
    combo(h, LIMIT, &[10, 25, 50, 100, 200, 300, 500, 1000, 2000], c.limit_mb, [270, 394, 90, 200]);
    label("MB", 0, 0, [370, 398, 60, 18]);
    check(START, "", c.start_browser, [24, 428, 450, 20]);
    check(EXT_ON, "Enable extended processes", c.extended_enabled, [24, 458, 240, 20]);
    ctl(h, w!("BUTTON"), "Extended Processes", WS_TABSTOP, 0, [290, 450, 180, 32], EXT_BTN, 0);
    check(ASTART, "Start edgemin-lite when Windows starts", crate::autostart_enabled(), [24, 492, 440, 20]);

    let save = ctl(h, w!("BUTTON"), "Save", BS_DEFPUSHBUTTON as u32 | WS_TABSTOP, 0, [398, 548, 92, 30], OK, 0);
    EnableWindow(save, 0); // enabled by the first change
    ctl(h, w!("BUTTON"), "Cancel", WS_TABSTOP, 0, [298, 548, 92, 30], CANCEL, 0);
    label("", STATUS, 0, [12, 592, 476, 18]);
    refresh_browser(h);
    refresh_live(h);
    sync(h);
    status(h);
}

unsafe fn paint(h: HWND) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let dc = BeginPaint(h, &mut ps);
    let line = CreateSolidBrush(0x00E0E0E0);
    let row = |top: i32, bottom: i32| RECT { left: 0, top: sc(top), right: sc(W), bottom: sc(bottom) };
    FillRect(dc, &row(BAR, H), GetSysColorBrush(COLOR_BTNFACE as i32));
    FillRect(dc, &row(BAR, BAR + 1), line);
    FillRect(dc, &row(STATUS_LINE, STATUS_LINE + 1), line);
    FillRect(dc, &row(HEADER_LINE, HEADER_LINE + 1), line);
    DeleteObject(line);
    EndPaint(h, &ps);
}

unsafe fn save(h: HWND) {
    let Some((mut c, owner)) = CTX.with(|x| x.borrow().clone()) else { return };
    let path = text(h, PATH);
    if !path.trim().is_empty() {
        c.browser_path = path.trim().to_string();
    }
    c.enabled = checked(h, ENABLED);
    c.limit_enabled = checked(h, LIMIT_ON);
    if let Ok(n) = text(h, LIMIT).trim().parse() {
        c.limit_mb = n;
    }
    if let Ok(n) = text(h, INTERVAL).trim().parse::<u32>() {
        c.interval_ms = n.max(100);
    }
    c.start_browser = checked(h, START);
    c.extended_enabled = checked(h, EXT_ON);
    c.extended = EXT_LIST.with(|e| e.borrow().clone());
    if c.save().is_err() {
        MessageBoxW(h, w!("Could not write the config file."), w!("edgemin-lite"), MB_ICONERROR);
        return;
    }
    if checked(h, ASTART) != crate::autostart_enabled() {
        crate::toggle_autostart();
    }
    PostMessageW(owner, WM_RELOAD, 0, 0);
    DestroyWindow(h);
}

unsafe fn browse(h: HWND) -> bool {
    let mut buf = [0u16; 520];
    let cur = w(&text(h, PATH));
    buf[..cur.len().min(519)].copy_from_slice(&cur[..cur.len().min(519)]);
    let mut o: OPENFILENAMEW = std::mem::zeroed();
    o.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    o.hwndOwner = h;
    o.lpstrFilter = w!("Programs (*.exe)\0*.exe\0All files\0*.*\0");
    o.lpstrFile = buf.as_mut_ptr();
    o.nMaxFile = buf.len() as u32;
    o.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST;
    if GetOpenFileNameW(&mut o) == 0 {
        return false;
    }
    SetWindowTextW(GetDlgItem(h, PATH), buf.as_ptr());
    refresh_browser(h);
    true
}

fn color_for(id: i32) -> u32 {
    match id {
        HINT | COMPANY | HINT_PATH_LABEL => 0x00666666,
        PATH => 0x00C07000,    // RGB(0,112,192)
        USAGE => 0x000000C8,   // red
        EXT_USE => 0x00008000, // green
        STATUS => 0x00555555,
        _ => 0,
    }
}

unsafe fn ctl_color(wp: WPARAM, lp: LPARAM) -> LRESULT {
    let dc = wp as _;
    let id = GetDlgCtrlID(lp as HWND);
    SetTextColor(dc, color_for(id));
    if id == STATUS {
        SetBkColor(dc, windows_sys::Win32::Graphics::Gdi::GetSysColor(COLOR_BTNFACE));
        return GetSysColorBrush(COLOR_BTNFACE as i32) as LRESULT;
    }
    SetBkColor(dc, 0x00FFFFFF);
    GetStockObject(WHITE_BRUSH) as LRESULT
}

unsafe extern "system" fn proc_(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            if let Some((c, _)) = CTX.with(|x| x.borrow().clone()) {
                build(h, &c);
            }
        }
        WM_PAINT => paint(h),
        WM_TIMER => refresh_live(h),
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => return ctl_color(wp, lp),
        WM_CHANGED => {
            EnableWindow(GetDlgItem(h, OK), 1);
            sync(h);
            status(h);
        }
        WM_COMMAND => {
            let (id, note) = ((wp & 0xFFFF) as i32, ((wp >> 16) & 0xFFFF) as u32);
            match id {
                OK => save(h),
                CANCEL | M_CLOSE => {
                    DestroyWindow(h);
                }
                BROWSE => {
                    if browse(h) {
                        PostMessageW(h, WM_CHANGED, 0, 0);
                    }
                }
                EXT_BTN => open_ext(h),
                M_OPEN => {
                    let p = w(&config::path().to_string_lossy());
                    ShellExecuteW(null_mut(), w!("open"), w!("notepad.exe"), p.as_ptr(), null(), SW_SHOWNORMAL);
                }
                M_ABOUT => {
                    MessageBoxW(
                        h,
                        w!("edgemin-lite\n\nTiny tray app that empties the working set of a browser's processes.\nBehavior modeled on Rizonesoft Edgemin; MIT licensed."),
                        w!("About edgemin-lite"),
                        MB_ICONINFORMATION,
                    );
                }
                _ if LOADING.with(Cell::get) => {}
                // BN_CLICKED = 0, CBN_SELCHANGE = 1, CBN_EDITCHANGE = 5
                ENABLED | LIMIT_ON | START | EXT_ON | ASTART if note == 0 => {
                    PostMessageW(h, WM_CHANGED, 0, 0);
                }
                INTERVAL | LIMIT if note == 1 || note == 5 => {
                    PostMessageW(h, WM_CHANGED, 0, 0); // posted so the combo text is updated first
                }
                _ => {}
            }
        }
        WM_CLOSE => {
            DestroyWindow(h);
        }
        WM_DESTROY => {
            KillTimer(h, 1);
            WIN.with(|w| w.set(null_mut()));
            for f in FONTS.with(Cell::get) {
                DeleteObject(f as _);
            }
            for ic in ICONS.with(Cell::get) {
                if ic != 0 {
                    DestroyIcon(ic as _);
                }
            }
            ICONS.with(|i| i.set([0; 2]));
        }
        _ => return DefWindowProcW(h, msg, wp, lp),
    }
    0
}

// ---- extended processes editor (owned, modal) ----

unsafe fn open_ext(owner: HWND) {
    let hinst = GetModuleHandleW(null());
    let class = w!("edgemin-lite-ext");
    let mut wc: WNDCLASSW = std::mem::zeroed();
    wc.lpfnWndProc = Some(xproc);
    wc.hInstance = hinst;
    wc.lpszClassName = class;
    wc.hCursor = LoadCursorW(null_mut(), IDC_ARROW);
    wc.hbrBackground = GetStockObject(WHITE_BRUSH) as _;
    RegisterClassW(&wc);
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
    let mut r = RECT { left: 0, top: 0, right: sc(380), bottom: sc(300) };
    AdjustWindowRect(&mut r, style, 0);
    let (ww, wh) = (r.right - r.left, r.bottom - r.top);
    let mut o: RECT = std::mem::zeroed();
    GetWindowRect(owner, &mut o);
    let (x, y) = (o.left + (o.right - o.left - ww) / 2, o.top + (o.bottom - o.top - wh) / 2);
    let h = CreateWindowExW(0, class, w!("Extended Processes"), style, x, y, ww, wh, owner, null_mut(), hinst, null());
    if h.is_null() {
        return;
    }
    XWIN.with(|x| x.set(h));
    EnableWindow(owner, 0);
    ShowWindow(h, SW_SHOW);
    SetFocus(GetDlgItem(h, X_EDIT));
}

unsafe extern "system" fn xproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            ctl(h, w!("STATIC"), "Other processes to trim along with the browser, one executable name per line (for example msedgewebview2.exe).", 0, 0, [16, 12, 348, 40], 0, 0);
            let lines = EXT_LIST.with(|e| e.borrow().join("\r\n"));
            ctl(
                h, w!("EDIT"), &lines,
                (ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN) as u32 | WS_VSCROLL | WS_TABSTOP,
                WS_EX_CLIENTEDGE, [16, 58, 348, 180], X_EDIT, 0,
            );
            ctl(h, w!("BUTTON"), "OK", BS_DEFPUSHBUTTON as u32 | WS_TABSTOP, 0, [182, 252, 86, 30], OK, 0);
            ctl(h, w!("BUTTON"), "Cancel", WS_TABSTOP, 0, [278, 252, 86, 30], CANCEL, 0);
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => return ctl_color(wp, lp),
        WM_COMMAND => match (wp & 0xFFFF) as i32 {
            OK => {
                let list: Vec<String> = text(h, X_EDIT)
                    .split(['\r', '\n', ','])
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect();
                EXT_LIST.with(|e| *e.borrow_mut() = list);
                PostMessageW(GetWindow(h, GW_OWNER), WM_CHANGED, 0, 0);
                DestroyWindow(h);
            }
            CANCEL => {
                DestroyWindow(h);
            }
            _ => {}
        },
        WM_CLOSE => {
            DestroyWindow(h);
        }
        WM_DESTROY => {
            let owner = GetWindow(h, GW_OWNER);
            XWIN.with(|x| x.set(null_mut()));
            EnableWindow(owner, 1);
            SetForegroundWindow(owner);
        }
        _ => return DefWindowProcW(h, msg, wp, lp),
    }
    0
}

pub fn open(owner: HWND, cfg: Config) {
    unsafe {
        if !hwnd().is_null() {
            SetForegroundWindow(hwnd());
            return;
        }
        LOADING.with(|l| l.set(true));
        CTX.with(|x| *x.borrow_mut() = Some((cfg, owner)));
        let dc = GetDC(null_mut());
        let dpi = GetDeviceCaps(dc, 88); // LOGPIXELSX
        ReleaseDC(null_mut(), dc);
        DPI.with(|d| d.set(dpi.max(96)));
        let font = |pt: i32, weight: i32, italic: u32| {
            CreateFontW(-(pt * dpi / 72), 0, 0, 0, weight, italic, 0, 0, 1, 0, 0, 5, 0, w!("Segoe UI")) as usize
        };
        FONTS.with(|f| f.set([font(9, 400, 0), font(14, 400, 0), font(9, 400, 1)]));

        let hinst = GetModuleHandleW(null());
        let class = w!("edgemin-lite-settings");
        if !REGISTERED.with(Cell::get) {
            // process-lifetime class icon: created once with the class
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(proc_);
            wc.hInstance = hinst;
            wc.hIcon = crate::app_icon(GetSystemMetrics(SM_CXICON));
            wc.lpszClassName = class;
            wc.hCursor = LoadCursorW(null_mut(), IDC_ARROW);
            wc.hbrBackground = GetStockObject(WHITE_BRUSH) as _;
            REGISTERED.with(|r| r.set(RegisterClassW(&wc) != 0));
        }

        let file = CreatePopupMenu();
        AppendMenuW(file, MF_STRING, M_OPEN as usize, w!("Open config file"));
        AppendMenuW(file, MF_STRING, M_CLOSE as usize, w!("Close"));
        let help = CreatePopupMenu();
        AppendMenuW(help, MF_STRING, M_ABOUT as usize, w!("About"));
        let bar = CreateMenu();
        AppendMenuW(bar, MF_POPUP, file as usize, w!("File"));
        AppendMenuW(bar, MF_POPUP, help as usize, w!("Help"));

        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let mut r = RECT { left: 0, top: 0, right: sc(W), bottom: sc(H) };
        AdjustWindowRect(&mut r, style, 1);
        let (ww, wh) = (r.right - r.left, r.bottom - r.top);
        let (x, y) = ((GetSystemMetrics(SM_CXSCREEN) - ww) / 2, (GetSystemMetrics(SM_CYSCREEN) - wh) / 2);
        let h = CreateWindowExW(0, class, w!("edgemin-lite settings"), style, x.max(0), y.max(0), ww, wh, null_mut(), bar, hinst, null());
        if h.is_null() {
            return;
        }
        WIN.with(|w| w.set(h));
        LOADING.with(|l| l.set(false));
        SetTimer(h, 1, 1000, None);
        ShowWindow(h, SW_SHOW);
        SetForegroundWindow(h);
        SetFocus(GetDlgItem(h, BROWSE));
    }
}
