//! BlazeCursor: shake the mouse left/right quickly and the pointer grows so you can find it.
//! Lives in the system tray; right-click the tray icon to quit.
//!
//! Fully event-driven: mouse movement arrives as raw input, and the animation timer only runs
//! while the cursor is enlarged, so the app does no work at all while the mouse is still.
#![windows_subsystem = "windows"]

use std::collections::VecDeque;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use windows_sys::core::PCWSTR;
use windows_sys::w;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::System::Threading::*;
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::Input::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

// --- Tuning -----------------------------------------------------------------
const ANIM_TICK_MS: i64 = 7; // update interval while the cursor is resizing (~144 Hz)
const HOLD_TICK_MS: i64 = 50; // check interval while fully grown, waiting for the shake to end
const WINDOW: Duration = Duration::from_millis(500); // reversals older than this are forgotten
const REVERSALS_TO_START: usize = 4; // direction changes needed to trigger
const MAX_SCALE: f32 = 4.0; // biggest cursor = base size * MAX_SCALE
const FRAMES: usize = 32; // cursor sizes between normal and max
const GROW_MS: f32 = 70.0; // easing time constant while growing
const SHRINK_MS: f32 = 120.0; // easing time constant while shrinking

const WM_TRAY: u32 = WM_APP + 1;
const ID_QUIT: usize = 1;
const IDI_APP: PCWSTR = 1 as PCWSTR; // icon resource id, see assets/blazecursor.rc
const MOUSE_MOVE_ABSOLUTE: u16 = 1; // RAWMOUSE.usFlags: tablets, remote desktop, VMs

/// System cursors we enlarge, with their names under HKCU\Control Panel\Cursors.
const CURSORS: [(u32, PCWSTR); 3] = [
    (OCR_NORMAL, w!("Arrow")),
    (OCR_IBEAM, w!("IBeam")),
    (OCR_HAND, w!("Hand")),
];

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

fn main() {
    unsafe {
        // One instance only.
        CreateMutexW(null(), 0, w!("BlazeCursor.SingleInstance"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        restore_cursors(); // in case a previous run was killed while enlarged

        let hinst = GetModuleHandleW(null());
        let class = w!("BlazeCursorWnd");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: class,
            ..zeroed()
        };
        RegisterClassW(&wc);
        TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);
        let hwnd = CreateWindowExW(0, class, w!("BlazeCursor"), 0, 0, 0, 0, 0, null_mut(), null_mut(), hinst, null());

        tray(hwnd, NIM_ADD);

        // Mouse movement from every device, even though our window is never focused.
        let mouse = RAWINPUTDEVICE { usUsagePage: 0x01, usUsage: 0x02, dwFlags: RIDEV_INPUTSINK, hwndTarget: hwnd };
        RegisterRawInputDevices(&mouse, 1, size_of::<RAWINPUTDEVICE>() as u32);

        // A high-resolution waitable timer gives ~1 ms precision without changing the global
        // timer resolution. It is only armed while the cursor is enlarged.
        let timer = CreateWaitableTimerExW(null(), null(), CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS);
        let mut ticking = false;

        let mut shake = Shake::new();
        let mut msg: MSG = zeroed();
        'run: loop {
            if MsgWaitForMultipleObjects(1, &timer, 0, INFINITE, QS_ALLINPUT) == WAIT_OBJECT_0 {
                ticking = false;
                if let Some(ms) = shake.tick() {
                    arm(timer, ms);
                    ticking = true;
                }
            }
            while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT {
                    break 'run;
                }
                if msg.message == WM_INPUT {
                    if let Some(raw_dx) = raw_mouse_dx(msg.lParam) {
                        shake.on_move(raw_dx);
                        if shake.busy() && !ticking {
                            arm(timer, 1);
                            ticking = true;
                        }
                    }
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        tray(hwnd, NIM_DELETE);
        restore_cursors();
    }
}

/// Schedule the next timer wake-up `ms` milliseconds from now.
unsafe fn arm(timer: HANDLE, ms: i64) {
    let due = -ms * 10_000; // negative = relative, in 100 ns units
    SetWaitableTimer(timer, &due, 0, None, null(), 0);
}

/// Horizontal movement reported by a WM_INPUT mouse event (0 for absolute-position devices).
unsafe fn raw_mouse_dx(lp: LPARAM) -> Option<i32> {
    let mut raw: RAWINPUT = zeroed();
    let mut size = size_of::<RAWINPUT>() as u32;
    let header = size_of::<RAWINPUTHEADER>() as u32;
    if GetRawInputData(lp as HRAWINPUT, RID_INPUT, (&mut raw as *mut RAWINPUT).cast(), &mut size, header) == u32::MAX
        || raw.header.dwType != RIM_TYPEMOUSE
    {
        return None;
    }
    let m = raw.data.mouse;
    Some(if m.usFlags as u16 & MOUSE_MOVE_ABSOLUTE != 0 { 0 } else { m.lLastX })
}

struct Shake {
    base: i32,                             // normal cursor size in px
    frames: Vec<[HCURSOR; CURSORS.len()]>, // frames[i] = step i+1, loaded on first use
    min_seg: i32,                          // px a stroke must travel to count as a reversal
    last_x: i32,
    dir: i32,
    run: i32,
    reversals: VecDeque<Instant>,
    active: bool,
    level: f32, // 0 = normal size, 1 = max size
    last_tick: Instant,
    shown: usize,
}

impl Shake {
    unsafe fn new() -> Self {
        let base = GetSystemMetrics(SM_CXCURSOR).max(32);
        let mut p = POINT { x: 0, y: 0 };
        GetCursorPos(&mut p);
        Shake {
            base,
            frames: vec![[null_mut(); CURSORS.len()]; FRAMES],
            min_seg: base * 5 / 4,
            last_x: p.x,
            dir: 0,
            run: 0,
            reversals: VecDeque::new(),
            active: false,
            level: 0.0,
            last_tick: Instant::now(),
            shown: 0,
        }
    }

    /// True while the cursor is enlarged or about to be, i.e. while the timer is needed.
    fn busy(&self) -> bool {
        self.active || self.level > 0.0
    }

    /// Counts horizontal direction reversals of sufficiently long strokes.
    unsafe fn on_move(&mut self, raw_dx: i32) {
        let now = Instant::now();
        let mut p = POINT { x: 0, y: 0 };
        GetCursorPos(&mut p);
        // Prefer on-screen movement; fall back to raw counts when the cursor is pinned at a
        // screen edge so shakes there still register.
        let dx = if p.x != self.last_x { p.x - self.last_x } else { raw_dx };
        self.last_x = p.x;
        if dx == 0 {
            return;
        }
        let d = dx.signum();
        if d != self.dir {
            if self.run >= self.min_seg {
                self.reversals.push_back(now);
                if !self.busy() {
                    self.last_tick = now; // animation is about to (re)start
                }
                self.update_active(now);
            }
            self.dir = d;
            self.run = 0;
        }
        self.run += dx.abs();
    }

    /// Start on a vigorous shake, stay big while the shake continues.
    fn update_active(&mut self, now: Instant) {
        while self.reversals.front().is_some_and(|&t| now - t > WINDOW) {
            self.reversals.pop_front();
        }
        let needed = if self.active { 1 } else { REVERSALS_TO_START };
        self.active = self.reversals.len() >= needed;
    }

    /// Advances the animation. Returns ms until the next tick, or None when back to idle.
    unsafe fn tick(&mut self) -> Option<i64> {
        let now = Instant::now();
        let dt = (now - self.last_tick).as_secs_f32().min(0.05) * 1000.0;
        self.last_tick = now;
        self.update_active(now);

        // Exponential ease-out toward the target size, independent of tick jitter.
        let (target, tau) = if self.active { (1.0, GROW_MS) } else { (0.0, SHRINK_MS) };
        self.level += (target - self.level) * (1.0 - (-dt / tau).exp());
        if (target - self.level).abs() < 0.003 {
            self.level = target;
        }

        let frame = (self.level * FRAMES as f32).round() as usize;
        if frame != self.shown {
            self.shown = frame;
            if frame == 0 {
                restore_cursors();
                self.free_frames();
            } else {
                let base = self.base;
                let set = &mut self.frames[frame - 1];
                if set[0].is_null() {
                    let scale = 1.0 + (MAX_SCALE - 1.0) * frame as f32 / FRAMES as f32;
                    let size = (base as f32 * scale).round() as i32;
                    *set = CURSORS.map(|(ocr, name)| load_cursor(ocr, name, size));
                }
                for (&cursor, &(ocr, _)) in set.iter().zip(&CURSORS) {
                    // SetSystemCursor takes ownership, so hand it a copy.
                    SetSystemCursor(CopyIcon(cursor), ocr);
                }
            }
        }

        match (self.active, self.level) {
            (false, l) if l == 0.0 => None,
            (true, l) if l == 1.0 => Some(HOLD_TICK_MS),
            _ => Some(ANIM_TICK_MS),
        }
    }

    unsafe fn free_frames(&mut self) {
        for cursor in self.frames.iter_mut().flatten().filter(|c| !c.is_null()) {
            DestroyCursor(*cursor);
            *cursor = null_mut();
        }
    }
}

/// Load the user's current cursor for `name` at `size` px, falling back to the built-in one.
unsafe fn load_cursor(ocr: u32, name: PCWSTR, size: i32) -> HCURSOR {
    let mut path = [0u16; MAX_PATH as usize];
    let mut len = size_of::<[u16; MAX_PATH as usize]>() as u32;
    let found = RegGetValueW(
        HKEY_CURRENT_USER,
        w!("Control Panel\\Cursors"),
        name,
        RRF_RT_REG_SZ,
        null_mut(),
        path.as_mut_ptr().cast(),
        &mut len,
    ) == ERROR_SUCCESS
        && path[0] != 0;
    if found {
        let h = LoadImageW(null_mut(), path.as_ptr(), IMAGE_CURSOR, size, size, LR_LOADFROMFILE);
        if !h.is_null() {
            return h;
        }
    }
    // Built-in cursors (e.g. the default I-beam) ignore the requested size, so stretch a copy.
    let builtin = LoadImageW(null_mut(), ocr as usize as PCWSTR, IMAGE_CURSOR, 0, 0, LR_DEFAULTSIZE | LR_SHARED);
    CopyImage(builtin, IMAGE_CURSOR, size, size, 0)
}

/// Reload the user's cursor scheme, undoing any SetSystemCursor calls.
unsafe fn restore_cursors() {
    SystemParametersInfoW(SPI_SETCURSORS, 0, null_mut(), 0);
}

unsafe fn tray(hwnd: HWND, action: NOTIFY_ICON_MESSAGE) {
    let mut nid: NOTIFYICONDATAW = zeroed();
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    let size = GetSystemMetrics(SM_CXSMICON);
    nid.hIcon = LoadImageW(GetModuleHandleW(null()), IDI_APP, IMAGE_ICON, size, size, LR_DEFAULTCOLOR);
    for (dst, src) in nid.szTip.iter_mut().zip("BlazeCursor - shake to find your pointer".encode_utf16()) {
        *dst = src;
    }
    Shell_NotifyIconW(action, &nid);
    if !nid.hIcon.is_null() {
        DestroyIcon(nid.hIcon); // the shell keeps its own copy
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY if matches!(lp as u32, WM_RBUTTONUP | WM_LBUTTONUP) => {
            let menu = CreatePopupMenu();
            AppendMenuW(menu, MF_STRING, ID_QUIT, w!("Quit BlazeCursor"));
            let mut p = POINT { x: 0, y: 0 };
            GetCursorPos(&mut p);
            SetForegroundWindow(hwnd);
            let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, p.x, p.y, 0, hwnd, null());
            DestroyMenu(menu);
            if cmd as usize == ID_QUIT {
                PostQuitMessage(0);
            }
            0
        }
        WM_ENDSESSION => {
            restore_cursors();
            0
        }
        m if m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            tray(hwnd, NIM_ADD); // Explorer restarted
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
