#![windows_subsystem = "windows"]
mod config;
mod gui;
mod trim;
mod update;

use config::Config;
use std::cell::RefCell;
use std::ptr::{null, null_mut};
use windows_sys::w;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::Threading::{CreateMutexW, Sleep};
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const WM_TRAY: u32 = WM_APP + 1;
const RUN_KEY: *const u16 = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VAL: *const u16 = w!("edgemin-lite");
const APPROVED_KEY: *const u16 = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run");

static ICO: &[u8] = include_bytes!("../assets/edgemin-lite.ico");

/// Icon from the embedded .ico, using the directory entry closest to `px` (PNG-compressed entries).
pub fn app_icon(px: i32) -> HICON {
    let rd = |o: usize| u16::from_le_bytes([ICO[o], ICO[o + 1]]) as usize;
    let rd32 = |o: usize| u32::from_le_bytes([ICO[o], ICO[o + 1], ICO[o + 2], ICO[o + 3]]) as usize;
    let best = (0..rd(4))
        .map(|i| 6 + 16 * i)
        .min_by_key(|&e| ((if ICO[e] == 0 { 256 } else { ICO[e] as i32 }) - px).abs())
        .unwrap();
    let (len, off) = (rd32(e_len(best)), rd32(best + 12));
    unsafe { CreateIconFromResourceEx(ICO[off..off + len].as_ptr(), len as u32, 1, 0x30000, px, px, 0) }
}

fn e_len(e: usize) -> usize {
    e + 8
}

/// Last measured memory (bytes, before trimming) shown in the settings window.
#[derive(Clone, Copy, Default)]
struct Live {
    core: u64,
    procs: u32,
    peak: u64,
    ext: u64,
    ext_peak: u64,
}

fn live() -> Live {
    with(|s| s.live)
}

struct State {
    cfg: Config,
    nid: NOTIFYICONDATAW,
    taskbar_created: u32,
    live: Live,
    update: Option<update::Release>, // newest release found, if newer than this build
    busy: bool,                      // a check/download thread is running
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(s.borrow_mut().as_mut().unwrap()))
}

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Command line stored in the Run key: the quoted path of this exe.
fn run_command() -> Option<String> {
    std::env::current_exe().ok().map(|e| format!("\"{}\"", e.display()))
}

/// True only if the Run value points at this exe and Task Manager hasn't disabled it.
fn autostart_enabled() -> bool {
    let Some(want) = run_command() else { return false };
    unsafe {
        let mut buf = [0u16; 1024];
        let mut len = (buf.len() * 2) as u32;
        if RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VAL, RRF_RT_REG_SZ, null_mut(), buf.as_mut_ptr().cast(), &mut len)
            != ERROR_SUCCESS
        {
            return false;
        }
        let got = String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]);
        if !got.eq_ignore_ascii_case(&want) {
            return false;
        }
        let mut b = [0u8; 12];
        let mut blen = 12u32;
        let r = RegGetValueW(HKEY_CURRENT_USER, APPROVED_KEY, RUN_VAL, RRF_RT_REG_BINARY, null_mut(), b.as_mut_ptr().cast(), &mut blen);
        !(r == ERROR_SUCCESS && matches!(b[0], 3 | 7))
    }
}

fn set_autostart(on: bool) {
    unsafe {
        // Drops a Task Manager "disabled" flag; absence means enabled.
        RegDeleteKeyValueW(HKEY_CURRENT_USER, APPROVED_KEY, RUN_VAL);
        match run_command() {
            Some(cmd) if on => {
                let d = w(&cmd);
                RegSetKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VAL, REG_SZ, d.as_ptr().cast(), (d.len() * 2) as u32);
            }
            _ => {
                RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VAL);
            }
        }
    }
}

fn run_browser(args: Option<&str>) {
    let (path, exe) = with(|s| (s.cfg.browser_path.clone(), s.cfg.core_exe().to_string()));
    let p = std::path::Path::new(&path);
    if !p.exists() || trim::process_exists(&exe) {
        return;
    }
    let dir = p.parent().map(|d| w(&d.to_string_lossy()));
    let a = args.map(w);
    unsafe {
        ShellExecuteW(
            null_mut(),
            w!("open"),
            w(&path).as_ptr(),
            a.as_ref().map_or(null(), |a| a.as_ptr()),
            dir.as_ref().map_or(null(), |d| d.as_ptr()),
            SW_SHOWNORMAL,
        );
    }
}

fn tick(hwnd: HWND) {
    let (stats, enabled) = with(|s| (trim::run_tick(&s.cfg), s.cfg.enabled));
    let mb = |b: u64| b / 1048576;
    let mut tip = String::new();
    if !enabled {
        tip.push_str("[paused] ");
    }
    if let Some(c) = stats.first() {
        tip.push_str(&format!("edgemin-lite \u{2014} {}: {} MB ({} procs)", c.name, mb(c.bytes), c.procs));
    }
    for g in stats.iter().skip(1).filter(|g| g.procs > 0) {
        tip.push_str(&format!(" | {}: {} MB", g.name, mb(g.bytes)));
    }
    with(|s| {
        let core = stats.first().map_or(0, |g| g.bytes);
        let ext = stats.iter().skip(1).map(|g| g.bytes).sum();
        s.live = Live {
            core,
            procs: stats.first().map_or(0, |g| g.procs),
            peak: s.live.peak.max(core),
            ext,
            ext_peak: s.live.ext_peak.max(ext),
        };
    });
    let t: Vec<u16> = tip.encode_utf16().take(127).collect();
    with(|s| unsafe {
        s.nid.hWnd = hwnd;
        s.nid.uFlags = NIF_TIP;
        s.nid.szTip = [0; 128];
        s.nid.szTip[..t.len()].copy_from_slice(&t);
        Shell_NotifyIconW(NIM_MODIFY, &s.nid);
        s.nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    });
}

fn reload(hwnd: HWND) {
    let cfg = config::load();
    let ms = cfg.interval_ms;
    with(|s| {
        s.cfg = cfg;
        s.live = Live::default();
    });
    unsafe {
        KillTimer(hwnd, 1);
        SetTimer(hwnd, 1, ms, None);
    }
    tick(hwnd);
}

const WM_UPDATE: u32 = WM_APP + 3;

/// Results of the background update threads, delivered to the UI thread through `WM_UPDATE`.
enum Msg {
    Checked { manual: bool, res: Result<Option<update::Release>, String> },
    Downloaded(Result<std::path::PathBuf, String>),
}

fn post(hwnd: HWND, m: Msg) {
    let p = Box::into_raw(Box::new(m));
    unsafe {
        if PostMessageW(hwnd, WM_UPDATE, 0, p as LPARAM) == 0 {
            drop(Box::from_raw(p));
        }
    }
}

fn msgbox(text: &str, flags: u32) -> i32 {
    unsafe { MessageBoxW(null_mut(), w(text).as_ptr(), w!("edgemin-lite"), flags | MB_SETFOREGROUND) }
}

fn balloon(hwnd: HWND, msg: &str) {
    with(|s| unsafe {
        s.nid.hWnd = hwnd;
        s.nid.uFlags = NIF_INFO;
        s.nid.dwInfoFlags = NIIF_INFO;
        let (t, m) = (w("edgemin-lite"), w(msg));
        s.nid.szInfoTitle = [0; 64];
        s.nid.szInfoTitle[..t.len().min(63)].copy_from_slice(&t[..t.len().min(63)]);
        s.nid.szInfo = [0; 256];
        let n = m.len().min(255);
        s.nid.szInfo[..n].copy_from_slice(&m[..n]);
        Shell_NotifyIconW(NIM_MODIFY, &s.nid);
        s.nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        s.nid.szInfo = [0; 256];
    });
}

/// Starts a background check. Automatic runs are silent unless a new version is found.
fn check_updates(hwnd: HWND, manual: bool) {
    let Some(repo) = with(|s| {
        let ok = !s.busy && (manual || (s.cfg.update_check && !s.cfg.update_repo.is_empty()));
        if ok {
            s.busy = true;
        }
        ok.then(|| s.cfg.update_repo.clone())
    }) else {
        return;
    };
    let h = hwnd as usize;
    std::thread::spawn(move || post(h as HWND, Msg::Checked { manual, res: update::check(&repo) }));
}

fn begin_install(hwnd: HWND) {
    let Some(rel) = with(|s| if s.busy { None } else { s.update.clone() }) else { return };
    let ask = format!(
        "Update edgemin-lite v{} to {}?\n\nThe download is verified against the published SHA-256, then the app restarts.",
        update::VERSION, rel.version
    );
    if msgbox(&ask, MB_YESNO | MB_ICONQUESTION) != IDYES {
        return;
    }
    with(|s| s.busy = true);
    let h = hwnd as usize;
    std::thread::spawn(move || post(h as HWND, Msg::Downloaded(update::download(&rel))));
}

unsafe fn on_update(hwnd: HWND, lp: LPARAM) {
    let m = *Box::from_raw(lp as *mut Msg);
    with(|s| s.busy = false);
    match m {
        Msg::Checked { manual, res: Ok(Some(rel)) } => {
            let v = rel.version.clone();
            let fresh = with(|s| {
                let f = s.update.as_ref().map(|r| &r.version) != Some(&rel.version);
                s.update = Some(rel);
                f
            });
            if manual {
                begin_install(hwnd);
            } else if fresh {
                balloon(hwnd, &format!("Version {v} is available. Right-click the tray icon to update."));
            }
        }
        Msg::Checked { manual, res: Ok(None) } => {
            with(|s| s.update = None);
            if manual {
                msgbox(&format!("edgemin-lite v{} is up to date.", update::VERSION), MB_ICONINFORMATION);
            }
        }
        Msg::Checked { manual, res: Err(e) } => {
            if manual {
                msgbox(&format!("Update check failed: {e}"), MB_ICONERROR);
            }
        }
        Msg::Downloaded(Ok(staged)) => match update::install(&staged) {
            Ok(()) => {
                DestroyWindow(hwnd);
            }
            Err(e) => {
                msgbox(&format!("Update failed: {e}"), MB_ICONERROR);
            }
        },
        Msg::Downloaded(Err(e)) => {
            msgbox(&format!("Update failed: {e}"), MB_ICONERROR);
        }
    }
}

fn show_menu(hwnd: HWND) {
    let (enabled, core, can_run, can_safe) = with(|s| {
        let ok = std::path::Path::new(&s.cfg.browser_path).exists();
        (s.cfg.enabled, s.cfg.core_exe().to_string(), ok, ok && s.cfg.safemode_args().is_some())
    });
    let auto = autostart_enabled();
    let (upd, busy) = with(|s| (s.update.as_ref().map(|r| r.version.clone()), s.busy));
    let on = |c: bool, grey: bool| (if c { MF_CHECKED } else { 0 }) | if grey { MF_GRAYED } else { 0 };
    unsafe {
        let m = CreatePopupMenu();
        AppendMenuW(m, MF_STRING | on(enabled, false), 1, w!("Trimming enabled"));
        AppendMenuW(m, MF_STRING | on(false, !can_run), 2, w(&format!("Run {core}")).as_ptr());
        AppendMenuW(m, MF_STRING | on(false, !can_safe), 3, w(&format!("Run {core} in safe mode")).as_ptr());
        AppendMenuW(m, MF_SEPARATOR, 0, null());
        AppendMenuW(m, MF_STRING, 8, w!("Settings..."));
        AppendMenuW(m, MF_STRING, 4, w!("Edit config"));
        AppendMenuW(m, MF_STRING, 5, w!("Reload config"));
        AppendMenuW(m, MF_STRING | on(auto, false), 6, w!("Start with Windows"));
        if let Some(v) = &upd {
            AppendMenuW(m, MF_STRING, 10, w(&format!("Update to {v}...")).as_ptr());
        }
        AppendMenuW(m, MF_STRING | on(false, busy), 9, w!("Check for updates"));
        AppendMenuW(m, MF_SEPARATOR, 0, null());
        AppendMenuW(m, MF_STRING, 7, w!("Exit"));
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        SetForegroundWindow(hwnd);
        let id = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, null());
        DestroyMenu(m);
        match id {
            1 => {
                with(|s| s.cfg.enabled = !s.cfg.enabled);
                tick(hwnd);
            }
            2 => run_browser(None),
            3 => {
                let a = with(|s| s.cfg.safemode_args().map(String::from));
                run_browser(a.as_deref());
            }
            4 => {
                let p = w(&config::path().to_string_lossy());
                ShellExecuteW(null_mut(), w!("open"), w!("notepad.exe"), p.as_ptr(), null(), SW_SHOWNORMAL);
            }
            5 => reload(hwnd),
            8 => gui::open(hwnd, with(|s| s.cfg.clone())),
            6 => set_autostart(!autostart_enabled()),
            7 => {
                DestroyWindow(hwnd);
            }
            9 => check_updates(hwnd, true),
            10 => begin_install(hwnd),
            _ => {}
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER if wp == 2 => check_updates(hwnd, false),
        WM_TIMER => tick(hwnd),
        WM_UPDATE => on_update(hwnd, lp),
        gui::WM_RELOAD => reload(hwnd),
        WM_TRAY if matches!(lp as u32, WM_RBUTTONUP | WM_LBUTTONUP) => show_menu(hwnd),
        WM_DESTROY => {
            with(|s| Shell_NotifyIconW(NIM_DELETE, &s.nid));
            KillTimer(hwnd, 1);
            PostQuitMessage(0);
        }
        _ if msg == with(|s| s.taskbar_created) => {
            with(|s| Shell_NotifyIconW(NIM_ADD, &s.nid));
        }
        _ => return DefWindowProcW(hwnd, msg, wp, lp),
    }
    0
}

fn main() {
    unsafe {
        // After an update the new exe starts with --wait: the old instance still owns the mutex for a moment.
        let mut retries = if std::env::args().any(|a| a == "--wait") { 50 } else { 0 };
        loop {
            let mx = CreateMutexW(null(), 0, w!("Local\\edgemin-lite"));
            if GetLastError() != ERROR_ALREADY_EXISTS {
                break;
            }
            CloseHandle(mx);
            if retries == 0 {
                return;
            }
            retries -= 1;
            Sleep(100);
        }
        update::cleanup();
        let cfg = config::load();
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.uID = 1;
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_TRAY;
        nid.hIcon = app_icon(GetSystemMetrics(SM_CXSMICON));
        let tip = w("edgemin-lite");
        nid.szTip[..tip.len()].copy_from_slice(&tip);
        let (start, ms) = (cfg.start_browser, cfg.interval_ms);
        let tc = RegisterWindowMessageW(w!("TaskbarCreated"));
        STATE.with(|s| {
            *s.borrow_mut() = Some(State { cfg, nid, taskbar_created: tc, live: Live::default(), update: None, busy: false })
        });

        let hinst = GetModuleHandleW(null());
        let class = w!("edgemin-lite");
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(wndproc);
        wc.hInstance = hinst;
        wc.lpszClassName = class;
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(0, class, class, 0, 0, 0, 0, 0, HWND_MESSAGE, null_mut(), hinst, null());
        if hwnd.is_null() {
            return;
        }
        with(|s| {
            s.nid.hWnd = hwnd;
            Shell_NotifyIconW(NIM_ADD, &s.nid);
        });
        if start {
            run_browser(None);
        }
        SetTimer(hwnd, 1, ms, None);
        tick(hwnd);
        SetTimer(hwnd, 2, 86_400_000, None); // daily update check
        check_updates(hwnd, false);
        let mut m: MSG = std::mem::zeroed();
        while GetMessageW(&mut m, null_mut(), 0, 0) > 0 {
            if gui::pre_translate(&m) {
                continue;
            }
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
}
