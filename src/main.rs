#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod icons_data;

use std::collections::HashMap;
use std::fs;
use std::mem::size_of;
use std::path::PathBuf;
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use windows::core::w;
use windows::Win32::Foundation::{
    BOOL, COLORREF, ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DWMWINDOWATTRIBUTE,
};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BeginPaint, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BitBlt,
    BLENDFUNCTION, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateBitmap, CreateCompatibleBitmap,
    CreateCompatibleDC, CreateDIBSection, CreateFontW, CreatePen, CreateRoundRectRgn,
    CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH, DeleteDC, DeleteObject, DIB_RGB_COLORS,
    DrawTextW, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE,
    DT_VCENTER, DT_WORDBREAK, EndPaint, FillRect, FillRgn, FW_BOLD, FW_NORMAL, FW_SEMIBOLD,
    GdiAlphaBlend, GetMonitorInfoW, HDC, HFONT, InvalidateRect, IntersectClipRect,
    RestoreDC, SaveDC, MONITORINFO,
    MONITOR_DEFAULTTONEAREST, MonitorFromPoint, OUT_DEFAULT_PRECIS, PAINTSTRUCT, PS_SOLID,
    RoundRect, SelectObject, SetBkMode, SetTextColor, SRCCOPY, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN,
    RegisterHotKey, ReleaseCapture, SetCapture, UnregisterHotKey, VK_CONTROL, VK_LWIN, VK_MENU,
    VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, CreateWindowExW, CS_HREDRAW, CS_VREDRAW, DefWindowProcW, DestroyIcon,
    DI_NORMAL, DispatchMessageW, DrawIconEx, GetClientRect, GetCursorPos, GetMessageW,
    GetSystemMetrics, HICON, HWND_TOPMOST, ICONINFO, IDC_ARROW, IDC_HAND, KillTimer, LoadCursorW,
    LWA_ALPHA, MB_ICONINFORMATION, MB_ICONQUESTION, MB_YESNO, MB_SETFOREGROUND, MB_TOPMOST,
    IDYES, MessageBoxW, MSG, PostQuitMessage, RegisterClassW,
    RegisterWindowMessageW, SetForegroundWindow, SetLayeredWindowAttributes, SetProcessDPIAware,
    SetTimer, SetWindowPos, ShowWindow, SM_CXSCREEN, SM_CYSCREEN, SW_HIDE, SW_SHOW,
    SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_SHOWWINDOW, TranslateMessage, WM_ACTIVATE, WM_CLOSE,
    WM_DESTROY, WM_ERASEBKGND, WM_HOTKEY, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONUP,
    WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_RBUTTONUP, WM_CHAR, WM_SYSCHAR, WM_SYSKEYDOWN, WM_SYSKEYUP,
    WM_TIMER, WM_USER, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};

const APP_NAME: &str = "MixSwitch";
const APP_VERSION: &str = "v1.0.0";

const WM_TRAYICON: u32 = WM_USER + 100;
const TIMER_LIVE_SYNC: usize = 1;
// Keep activity meters live even while the user captures the mouse on a slider.
const MIXER_REFRESH_MS: u32 = 120;
const COMPACT_FLYOUT_WIDTH: i32 = 420;
const COMPACT_BODY_ROW: i32 = 58;
const SETTINGS_REFRESH_MS: u32 = 600;
const TIMER_OSD_HIDE: usize = 2;
// Shared layout constants ensure sizing, painting and mouse hit-testing agree.
const SETTINGS_BASE_HEIGHT: i32 = 376;
const SETTINGS_DEVICE_ROW_HEIGHT: i32 = 56;
const OSD_WIDTH: i32 = 244;
const OSD_HEIGHT: i32 = 72;

const HK_ID_MIXER: i32 = 1;
const HK_ID_SWITCH: i32 = 2;

static WM_TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
static TRAY_HICON: AtomicIsize = AtomicIsize::new(0);

// Color Helper: RGB to Win32 COLORREF (0x00BBGGRR)
const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16))
}

// MixSwitch Fluid Identity (2026): the new green -> cyan -> blue brand.
// Accent stays calm and solid on controls; gradients remain exclusive to logo art.
const COL_HEADER_BG: COLORREF = rgb(0x1E, 0x26, 0x32);
const COL_DEV_BTN_BG: COLORREF = rgb(0x15, 0x1D, 0x29);
const COL_DEV_BTN_HOVER: COLORREF = rgb(0x2D, 0x3A, 0x4A);
const COL_DROPDOWN_BG: COLORREF = rgb(0x17, 0x20, 0x2D);
const COL_DROPDOWN_ACTIVE: COLORREF = rgb(0x2B, 0x3B, 0x4C);
const COL_SETTINGS_BG: COLORREF = rgb(0x1A, 0x23, 0x31);
const COL_CARD_BG: COLORREF = rgb(0x29, 0x36, 0x47);
const COL_CARD_HOVER: COLORREF = rgb(0x35, 0x46, 0x58);
const COL_BODY_BG: COLORREF = rgb(0x13, 0x1B, 0x27);
const COL_MENU_BG: COLORREF = rgb(0x17, 0x20, 0x2D);
const COL_MENU_HDR: COLORREF = rgb(0x22, 0x30, 0x40);
const COL_MENU_HOVER: COLORREF = rgb(0x2C, 0x40, 0x51);
const COL_MENU_DANGER_HOV: COLORREF = rgb(0x43, 0x2A, 0x38);
const COL_BORDER: COLORREF = rgb(0x3A, 0x50, 0x62);
const COL_TEXT_MAIN: COLORREF = rgb(0xF3, 0xF8, 0xFC);
const COL_TEXT_DIM: COLORREF = rgb(0xB0, 0xBF, 0xD0);
const COL_PEAK_GREEN: COLORREF = rgb(0x71, 0xEB, 0xAD);
const COL_GREEN_ON: COLORREF = rgb(0x8F, 0xF4, 0xB1);
const COL_TRACK: COLORREF = rgb(0x3D, 0x50, 0x62);
// Consistent vivid red for mute state (not plum / burgundy).
const COL_MUTE: COLORREF = rgb(0xF0, 0x48, 0x48);
const COL_OSD_BG: COLORREF = rgb(0x1A, 0x25, 0x34);
const COL_BRAND_CYAN: COLORREF = rgb(0x23, 0xD1, 0xC9);
const COL_BRAND_BLUE: COLORREF = rgb(0x27, 0xAA, 0xED);

fn blend_color(base: COLORREF, top: COLORREF, top_pct: u32) -> COLORREF {
    let t = top_pct.min(100);
    let blend = |shift: u32| {
        let a = (base.0 >> shift) & 255;
        let b = (top.0 >> shift) & 255;
        ((a * (100 - t) + b * t) / 100) as u8
    };
    rgb(blend(0), blend(8), blend(16))
}

// Intentionally a MixSwitch brand accent: avoids reading OS DWM color as the
// primary identity. All interactive primitives use the same accessible cyan.
fn brand_accent() -> COLORREF { COL_BRAND_CYAN }
fn brand_accent_bright() -> COLORREF { blend_color(COL_BRAND_CYAN, COL_TEXT_MAIN, 22) }
fn brand_accent_soft() -> COLORREF { blend_color(COL_MENU_HDR, COL_BRAND_CYAN, 24) }

/// Use the OS-provided material on supported Windows 11 builds, falling back
/// naturally to the layered, near-opaque graphite painting on older systems.
unsafe fn apply_fluent_backdrop(hwnd: HWND) {
    apply_win11_corners(hwnd);
    let dark_mode: i32 = 1;
    let backdrop_type: i32 = 2; // DWMSBT_MAINWINDOW (Mica), Windows 11 22H2+
    let _ = DwmSetWindowAttribute(
        hwnd, DWMWINDOWATTRIBUTE(20), &dark_mode as *const _ as *const _,
        size_of::<i32>() as u32,
    );
    let _ = DwmSetWindowAttribute(
        hwnd, DWMWINDOWATTRIBUTE(38), &backdrop_type as *const _ as *const _,
        size_of::<i32>() as u32,
    );
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct AppConfig {
    pub hotkey_mixer: String,
    pub hotkey_switch: String,
    pub show_system_sounds: bool,
    pub device_icons: HashMap<String, String>,
    pub device_aliases: HashMap<String, String>,
    pub excluded_devices: Vec<String>,
    pub launch_at_startup: bool,
    pub startup_prompt_shown: bool,
    // The v4.2 startup setup notice is shown only once after upgrading.  This
    // migrates users whose earlier versions silently skipped the dialog.
    pub startup_v42_notice_shown: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            hotkey_mixer: "alt+q".to_string(),
            hotkey_switch: "alt+w".to_string(),
            show_system_sounds: true,
            device_icons: HashMap::new(),
            device_aliases: HashMap::new(),
            excluded_devices: Vec::new(),
            launch_at_startup: false,
            startup_prompt_shown: false,
            startup_v42_notice_shown: false,
        }
    }
}

#[derive(Clone, Debug)]
enum HitTarget {
    DeviceDropdownToggle,
    QuickSwitchBtn,
    SettingsToggleBtn,
    SelectDevice(String),
    MasterMuteToggle,
    MasterSlider(RECT),
    RecordHotkeyMixer,
    RecordHotkeySwitch,
    ToggleSystemSounds,
    ToggleDeviceInclude(String),
    CycleDeviceIcon(String, String),
    RenameDevice(String),
    ToggleStartup,
    OpenAboutDialog,
    AppMuteToggle(String),
    AppSlider(String, RECT),
}

struct UiHitBox {
    rect: RECT,
    target: HitTarget,
}

struct GlobalState {
    config: AppConfig,
    audio: audio::FullAudioState,
    hwnd_flyout: HWND,
    hwnd_osd: HWND,
    hwnd_menu: HWND,
    hwnd_about: HWND,
    flyout_visible: bool,
    menu_visible: bool,
    about_visible: bool,
    dropdown_expanded: bool,
    settings_expanded: bool,
    recording_target: Option<String>,
    rename_device_id: Option<String>,
    rename_buffer: String,
    dragging_slider: Option<(String, RECT)>,
    scroll_offset: i32,
    max_scroll: i32,
    last_nonzero_master: u32,
    last_nonzero_apps: HashMap<String, u32>,
    hitboxes: Vec<UiHitBox>,
    hover_pos: POINT,
    menu_hover_index: i32,
    osd_title: String,
    osd_subtitle: String,
    flyout_width: i32,
    flyout_height: i32,
    last_hide_at: Option<Instant>,
    drag_pending: Option<(String, u32)>,
    last_drag_apply: Instant,
    last_peak_refresh: Instant,
}

static STATE: Lazy<Mutex<GlobalState>> = Lazy::new(|| {
    let cfg = load_config_from_disk();
    let initial_audio = audio::get_full_audio_state(
        cfg.show_system_sounds,
        &cfg.device_icons,
        &cfg.excluded_devices,
    )
    .unwrap_or(audio::FullAudioState {
        current_device_id: String::new(),
        current_device_name: "Default Audio Output".to_string(),
        current_device_icon: "headphones".to_string(),
        master_volume: 100,
        master_muted: false,
        master_peak: 0,
        devices: Vec::new(),
        sessions: Vec::new(),
    });

    Mutex::new(GlobalState {
        config: cfg,
        audio: initial_audio,
        hwnd_flyout: HWND::default(),
        hwnd_osd: HWND::default(),
        hwnd_menu: HWND::default(),
        hwnd_about: HWND::default(),
        flyout_visible: false,
        menu_visible: false,
        about_visible: false,
        dropdown_expanded: false,
        settings_expanded: false,
        recording_target: None,
        rename_device_id: None,
        rename_buffer: String::new(),
        dragging_slider: None,
        scroll_offset: 0,
        max_scroll: 0,
        last_nonzero_master: 50,
        last_nonzero_apps: HashMap::new(),
        hitboxes: Vec::new(),
        hover_pos: POINT { x: -1, y: -1 },
        menu_hover_index: -1,
        osd_title: "Playback Device".to_string(),
        osd_subtitle: "Speakers".to_string(),
        flyout_width: COMPACT_FLYOUT_WIDTH,
        flyout_height: 280,
        last_hide_at: None,
        drag_pending: None,
        last_drag_apply: Instant::now(),
        last_peak_refresh: Instant::now(),
    })
});

fn get_config_path() -> PathBuf {
    let base = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    let new_dir = base.join("MixSwitch");
    let _ = fs::create_dir_all(&new_dir);
    let new_cfg = new_dir.join("mixswitch_config.json");

    // Migrate old AudioHub config seamlessly if present
    if !new_cfg.exists() {
        let old_cfg = base.join("AudioHub").join("audiohub_config.json");
        if old_cfg.exists() {
            let _ = fs::copy(&old_cfg, &new_cfg);
        }
    }
    new_cfg
}

fn load_config_from_disk() -> AppConfig {
    let path = get_config_path();
    if path.exists() {
        if let Ok(raw) = fs::read_to_string(&path) {
            match serde_json::from_str::<AppConfig>(&raw) {
                Ok(parsed) => return parsed,
                Err(_) => {
                    let _ = fs::copy(&path, path.with_extension("json.bak"));
                }
            }
        }
    }
    AppConfig::default()
}

fn save_config_to_disk(cfg: &AppConfig) {
    let path = get_config_path();
    if let Ok(json) = serde_json::to_string_pretty(cfg) {
        let _ = fs::write(path, json);
    }
}

// Display aliases affect only MixSwitch labels, not Windows device names or Core Audio IDs.
fn default_short_name(full: &str) -> &str {
    let candidate = full.split_once(" (").map(|(name, _)| name).unwrap_or(full).trim();
    if candidate.is_empty() { full } else { candidate }
}

fn display_device_name(cfg: &AppConfig, device_id: &str, full: &str) -> String {
    cfg.device_aliases.get(device_id)
        .filter(|alias| !alias.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| default_short_name(full).to_string())
}

fn displayed_switched_device(device_id: &str, full: &str) -> String {
    let st = STATE.lock();
    display_device_name(&st.config, device_id, full)
}

// Opt-in per-user startup, using the Windows Run key. No admin permissions,
// service, installer, or additional dependency required. reg.exe is invoked
// only when the user changes the checkbox, never by the paint/timer loop.
fn set_launch_at_startup(enabled: bool) -> bool {
    const KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    let mut command = Command::new("reg.exe");
    if enabled {
        let Ok(exe) = std::env::current_exe() else { return false };
        // Registry Run command must quote paths containing spaces.
        let quoted_path = format!("\"{}\"", exe.display());
        command.args(["add", KEY, "/v", "MixSwitch", "/t", "REG_SZ", "/d", &quoted_path, "/f"]);
    } else {
        command.args(["delete", KEY, "/v", "MixSwitch", "/f"]);
    }
    // Avoid flashing a console window because MixSwitch is a GUI application.
    command.creation_flags(0x08000000);
    match command.status() {
        Ok(code) => code.success(),
        Err(_) => false,
    }
}

fn commit_rename_device() {
    let mut st = STATE.lock();
    if let Some(id) = st.rename_device_id.take() {
        let name = st.rename_buffer.trim().to_string();
        if name.is_empty() {
            st.config.device_aliases.remove(&id); // Empty name means Reset
        } else {
            st.config.device_aliases.insert(id, name);
        }
        save_config_to_disk(&st.config);
    }
    st.rename_buffer.clear();
}

fn pt_in_rect(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x <= r.right && y >= r.top && y <= r.bottom
}

fn parse_hotkey_string(hk_str: &str) -> Option<(HOT_KEY_MODIFIERS, u32)> {
    let cleaned = hk_str.trim().to_lowercase();
    if cleaned.is_empty() {
        return None;
    }

    let mut mods = MOD_NOREPEAT;
    let mut vk: u32 = 0;

    for part in cleaned.split('+').map(|s| s.trim()).filter(|s| !s.is_empty()) {
        match part {
            "alt" | "menu" => mods |= MOD_ALT,
            "ctrl" | "control" => mods |= MOD_CONTROL,
            "shift" => mods |= MOD_SHIFT,
            "win" | "windows" | "super" | "meta" => mods |= MOD_WIN,
            "space" => vk = 0x20,
            "tab" => vk = 0x09,
            "enter" | "return" => vk = 0x0D,
            "escape" | "esc" => vk = 0x1B,
            "backspace" => vk = 0x08,
            "up" => vk = 0x26,
            "down" => vk = 0x28,
            "left" => vk = 0x25,
            "right" => vk = 0x27,
            "insert" => vk = 0x2D,
            "delete" => vk = 0x2E,
            "home" => vk = 0x24,
            "end" => vk = 0x23,
            "page_up" | "pageup" => vk = 0x21,
            "page_down" | "pagedown" => vk = 0x22,
            "f1" => vk = 0x70,
            "f2" => vk = 0x71,
            "f3" => vk = 0x72,
            "f4" => vk = 0x73,
            "f5" => vk = 0x74,
            "f6" => vk = 0x75,
            "f7" => vk = 0x76,
            "f8" => vk = 0x77,
            "f9" => vk = 0x78,
            "f10" => vk = 0x79,
            "f11" => vk = 0x7A,
            "f12" => vk = 0x7B,
            s if s.len() == 1 && s.chars().all(|c| c.is_ascii_alphanumeric()) => {
                vk = s.chars().next().unwrap().to_ascii_uppercase() as u32;
            }
            _ => return None,
        }
    }

    if vk != 0 {
        Some((mods, vk))
    } else {
        None
    }
}

unsafe fn register_global_hotkeys(hwnd: HWND, cfg: &AppConfig) {
    let _ = UnregisterHotKey(hwnd, HK_ID_MIXER);
    let _ = UnregisterHotKey(hwnd, HK_ID_SWITCH);

    if let Some((mods, vk)) = parse_hotkey_string(&cfg.hotkey_mixer) {
        let _ = RegisterHotKey(hwnd, HK_ID_MIXER, mods, vk);
    }
    if let Some((mods, vk)) = parse_hotkey_string(&cfg.hotkey_switch) {
        let _ = RegisterHotKey(hwnd, HK_ID_SWITCH, mods, vk);
    }
}

unsafe fn unregister_global_hotkeys(hwnd: HWND) {
    let _ = UnregisterHotKey(hwnd, HK_ID_MIXER);
    let _ = UnregisterHotKey(hwnd, HK_ID_SWITCH);
}

unsafe fn apply_win11_corners(hwnd: HWND) {
    let pref = DWMWCP_ROUND;
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE,
        &pref as *const _ as *const _,
        size_of::<i32>() as u32,
    );
}

unsafe fn get_active_monitor_work_rect() -> RECT {
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let mut mi = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if GetMonitorInfoW(hmon, &mut mi).as_bool() {
        mi.rcWork
    } else {
        RECT {
            left: 0,
            top: 0,
            right: GetSystemMetrics(SM_CXSCREEN),
            bottom: GetSystemMetrics(SM_CYSCREEN) - 48,
        }
    }
}

// =====================================================================
// Crisp Pre-multiplied Alpha BGRA Renderer & Custom Tray Icon Builder
// =====================================================================
unsafe fn draw_bgra_sprite(hdc: HDC, x: i32, y: i32, w: i32, h: i32, pixels: &[u8]) {
    let mem_dc = CreateCompatibleDC(hdc);
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits_ptr: *mut core::ffi::c_void = std::ptr::null_mut();
    if let Ok(dib) = CreateDIBSection(hdc, &bmi, DIB_RGB_COLORS, &mut bits_ptr, None, 0) {
        if !bits_ptr.is_null() && pixels.len() >= (w * h * 4) as usize {
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits_ptr as *mut u8, (w * h * 4) as usize);
            let old = SelectObject(mem_dc, dib);
            let bf = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let _ = GdiAlphaBlend(hdc, x, y, w, h, mem_dc, 0, 0, w, h, bf);
            SelectObject(mem_dc, old);
        }
        let _ = DeleteObject(dib);
    }
    let _ = DeleteDC(mem_dc);
}

/// Creates a crisp 32x32 32-bit ARGB HICON from MixSwitch vector identity
unsafe fn create_mixswitch_tray_hicon() -> HICON {
    let w = icons_data::TRAY_32_W;
    let h = icons_data::TRAY_32_H;
    let screen_dc = windows::Win32::Graphics::Gdi::GetDC(None);
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits_ptr: *mut core::ffi::c_void = std::ptr::null_mut();
    let hbm_color =
        CreateDIBSection(screen_dc, &bmi, DIB_RGB_COLORS, &mut bits_ptr, None, 0).unwrap_or_default();
    if !bits_ptr.is_null() {
        std::ptr::copy_nonoverlapping(
            icons_data::TRAY_32.as_ptr(),
            bits_ptr as *mut u8,
            icons_data::TRAY_32.len(),
        );
    }
    let mask_bits = [0u8; 128];
    let hbm_mask = CreateBitmap(w, h, 1, 1, Some(mask_bits.as_ptr() as *const _));
    let _ = windows::Win32::Graphics::Gdi::ReleaseDC(None, screen_dc);

    let ii = ICONINFO {
        fIcon: BOOL(1),
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: hbm_mask,
        hbmColor: hbm_color,
    };
    let hicon = CreateIconIndirect(&ii).unwrap_or_default();
    let _ = DeleteObject(hbm_color);
    let _ = DeleteObject(hbm_mask);
    hicon
}

unsafe fn add_tray_icon(hwnd: HWND) {
    let mut nid = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: WM_TRAYICON,
        hIcon: HICON(TRAY_HICON.load(Ordering::Relaxed)),
        ..Default::default()
    };
    let tip_str = format!("{} ({})", APP_NAME, APP_VERSION);
    for (i, ch) in tip_str.encode_utf16().enumerate() {
        if i < nid.szTip.len() - 1 {
            nid.szTip[i] = ch;
        }
    }
    let _ = Shell_NotifyIconW(NIM_ADD, &nid);
}

unsafe fn remove_tray_icon(hwnd: HWND) {
    let nid = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    };
    let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
}

// =====================================================================
// GDI Drawing Helpers (With DT_NOPREFIX enforced everywhere!)
// =====================================================================
unsafe fn fill_round_rect(
    hdc: HDC,
    r: &RECT,
    radius: i32,
    fill_col: COLORREF,
    border_col: Option<COLORREF>,
) {
    let rgn = CreateRoundRectRgn(r.left, r.top, r.right, r.bottom, radius, radius);
    let brush = CreateSolidBrush(fill_col);
    let _ = FillRgn(hdc, rgn, brush);
    let _ = DeleteObject(brush);
    let _ = DeleteObject(rgn);

    if let Some(b_col) = border_col {
        let pen = CreatePen(PS_SOLID, 1, b_col);
        let null_brush = windows::Win32::Graphics::Gdi::GetStockObject(
            windows::Win32::Graphics::Gdi::NULL_BRUSH,
        );
        let old_p = SelectObject(hdc, pen);
        let old_b = SelectObject(hdc, null_brush);
        let _ = RoundRect(hdc, r.left, r.top, r.right, r.bottom, radius, radius);
        SelectObject(hdc, old_p);
        SelectObject(hdc, old_b);
        let _ = DeleteObject(pen);
    }
}

// A translucent graphite veil fades a muted application's icon while
// preserving its identity. The actual icon handle is never modified.
unsafe fn fade_muted_icon(hdc: HDC, rect: RECT) {
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    if width <= 0 || height <= 0 { return; }
    let src = CreateCompatibleDC(hdc);
    let bmp = CreateCompatibleBitmap(hdc, width, height);
    let prev = SelectObject(src, bmp);
    let brush = CreateSolidBrush(COL_BODY_BG);
    let local = RECT { left:0, top:0, right:width, bottom:height };
    let _ = FillRect(src, &local, brush);
    let _ = GdiAlphaBlend(hdc, rect.left, rect.top, width, height,
        src, 0, 0, width, height,
        BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0,
            SourceConstantAlpha: 115, AlphaFormat: 0 });
    let _ = DeleteObject(brush);
    SelectObject(src, prev);
    let _ = DeleteObject(bmp);
    let _ = DeleteDC(src);
}

unsafe fn make_font(size: i32, weight: i32) -> HFONT {
    CreateFontW(
        -size,
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        DEFAULT_CHARSET.0 as u32,
        OUT_DEFAULT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32,
        DEFAULT_PITCH.0 as u32,
        w!("Segoe UI"),
    )
}

unsafe fn draw_text_str(
    hdc: HDC,
    text: &str,
    mut rc: RECT,
    color: COLORREF,
    font: HFONT,
    flags: u32,
) {
    let old_f = SelectObject(hdc, font);
    SetBkMode(hdc, TRANSPARENT);
    SetTextColor(hdc, color);
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    // Always OR with DT_NOPREFIX so '&' never turns into an underscore!
    let _ = DrawTextW(
        hdc,
        &mut wide,
        &mut rc,
        windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT(flags | DT_NOPREFIX.0),
    );
    SelectObject(hdc, old_f);
}

// Ask only once for an opt-in. Existing users who already enabled Startup
// keep their choice without a popup; this never toggles it silently.
unsafe fn offer_startup_on_first_run() {
    let (already_asked_this_release, already_enabled) = {
        let st = STATE.lock();
        (st.config.startup_v42_notice_shown, st.config.launch_at_startup)
    };
    if already_asked_this_release { return; }

    // Previous versions could mark the original prompt as seen before users
    // noticed it. Present a clear one-time opt-in/confirmation on this update;
    // we never enable autostart without an explicit Yes.
    let response = if already_enabled {
        MessageBoxW(
            None,
            w!("MixSwitch is currently configured to start with Windows.\n\nKeep launching MixSwitch at sign-in?\n\nYou can change this any time in Settings."),
            w!("MixSwitch - Windows Startup"),
            MB_YESNO | MB_ICONQUESTION | MB_TOPMOST | MB_SETFOREGROUND,
        )
    } else {
        MessageBoxW(
            None,
            w!("Would you like MixSwitch to start automatically when you sign in to Windows?\n\nIt will stay in the system tray. You can change this any time in Settings."),
            w!("MixSwitch - Windows Startup"),
            MB_YESNO | MB_ICONQUESTION | MB_TOPMOST | MB_SETFOREGROUND,
        )
    };
    let wanted = response == IDYES;
    let registration_ok = if wanted == already_enabled {
        // Re-register an enabled value in case the exe moved to another folder.
        if wanted { set_launch_at_startup(true) } else { true }
    } else {
        set_launch_at_startup(wanted)
    };
    let mut st = STATE.lock();
    st.config.startup_prompt_shown = true;
    st.config.startup_v42_notice_shown = true;
    if registration_ok { st.config.launch_at_startup = wanted; }
    save_config_to_disk(&st.config);
    drop(st);
    if !registration_ok {
        let _ = MessageBoxW(
            None,
            w!("Could not update Windows Startup. You can retry from MixSwitch Settings."),
            w!("MixSwitch - Startup"),
            MB_ICONINFORMATION,
        );
    }
}

// =====================================================================
// Show / Hide Windows & OSD Trigger
// =====================================================================
unsafe fn show_top_left_osd(title: &str, subtitle: &str) {
    let hwnd_osd = {
        let mut st = STATE.lock();
        st.osd_title = title.to_string();
        st.osd_subtitle = subtitle.to_string();
        st.hwnd_osd
    };
    if hwnd_osd.0 == 0 {
        return;
    }

    let work = get_active_monitor_work_rect();
    let w = OSD_WIDTH;
    let h = OSD_HEIGHT;
    let _ = SetWindowPos(
        hwnd_osd,
        HWND_TOPMOST,
        work.left + 22,
        work.top + 22,
        w,
        h,
        SWP_NOACTIVATE | SWP_SHOWWINDOW,
    );
    let _ = ShowWindow(hwnd_osd, SW_SHOWNOACTIVATE);
    let _ = InvalidateRect(hwnd_osd, None, BOOL(0));
    let _ = SetTimer(hwnd_osd, TIMER_OSD_HIDE, 1850, None);
}

unsafe fn show_about_window() {
    close_custom_tray_menu();
    hide_flyout();
    let hwnd_about = {
        let mut st = STATE.lock();
        st.about_visible = true;
        st.hwnd_about
    };
    if hwnd_about.0 == 0 {
        return;
    }
    let work = get_active_monitor_work_rect();
    let w = 420;
    let h = 340;
    let x = work.left + (work.right - work.left - w) / 2;
    let y = work.top + (work.bottom - work.top - h) / 2;
    let _ = SetWindowPos(hwnd_about, HWND_TOPMOST, x, y, w, h, SWP_SHOWWINDOW);
    let _ = ShowWindow(hwnd_about, SW_SHOW);
    let _ = SetForegroundWindow(hwnd_about);
    let _ = InvalidateRect(hwnd_about, None, BOOL(0));
}

unsafe fn close_about_window() {
    let hwnd_about = {
        let mut st = STATE.lock();
        if !st.about_visible {
            return;
        }
        st.about_visible = false;
        st.hwnd_about
    };
    let _ = ShowWindow(hwnd_about, SW_HIDE);
}

unsafe fn compute_desired_flyout_height(st: &GlobalState) -> (i32, i32) {
    let work = get_active_monitor_work_rect();
    let usable_h = (work.bottom - work.top - 36).max(180);
    let max_total_h = ((usable_h as f32) * 0.88) as i32;

    // The settings view REPLACES the mixer rows and does not scroll them.
    // The header itself remains available to return via the gear icon.
    if st.settings_expanded {
        let full_h = 84 + SETTINGS_BASE_HEIGHT
            + st.audio.devices.len() as i32 * SETTINGS_DEVICE_ROW_HEIGHT + 20;
        let shown_h = full_h.min(max_total_h).max(180);
        // The Settings content can scroll on laptops and low-height screens.
        return (shown_h, (full_h - shown_h).max(0));
    }

    let mut header_h = 130;
    if st.dropdown_expanded {
        header_h += 58 + (st.audio.devices.len() as i32) * 44;
    }
    let sessions_count = st.audio.sessions.len().max(1) as i32;
    let content_body_h = 18 + sessions_count * COMPACT_BODY_ROW;
    let actual_body_h = content_body_h.min((max_total_h - header_h).max(80));
    let max_scroll = (content_body_h - actual_body_h).max(0);
    (header_h + actual_body_h, max_scroll)
}

// Read live peak meters while a slider is being dragged.  Do NOT replace
// volume / mute values: they are optimistically updated by apply_slider_drag.
// This fixes the v3 frozen green bar without introducing a second timer.
unsafe fn refresh_drag_peaks() {
    let (cfg, hwnd) = {
        let mut st = STATE.lock();
        st.last_peak_refresh = Instant::now(); // throttle even if Core Audio read fails
        (st.config.clone(), st.hwnd_flyout)
    };
    if let Ok(latest) = audio::get_full_audio_state(
        cfg.show_system_sounds, &cfg.device_icons, &cfg.excluded_devices,
    ) {
        let mut st = STATE.lock();
        st.audio.master_peak = latest.master_peak;
        let peaks: HashMap<String, u32> = latest.sessions.into_iter()
            .map(|session| (session.key, session.peak))
            .collect();
        for session in &mut st.audio.sessions {
            session.peak = peaks.get(&session.key).copied().unwrap_or(0);
        }
        st.last_peak_refresh = Instant::now();
        drop(st);
        if hwnd.0 != 0 {
            let _ = InvalidateRect(hwnd, None, BOOL(0));
        }
    }
}

unsafe fn refresh_state_and_resize() {
    let (cfg, hwnd_flyout, is_vis) = {
        let st = STATE.lock();
        (st.config.clone(), st.hwnd_flyout, st.flyout_visible)
    };

    if let Ok(new_audio) = audio::get_full_audio_state(
        cfg.show_system_sounds,
        &cfg.device_icons,
        &cfg.excluded_devices,
    ) {
        let mut st = STATE.lock();
        if new_audio.master_volume > 0 {
            st.last_nonzero_master = new_audio.master_volume;
        }
        for s in &new_audio.sessions {
            if s.volume > 0 {
                st.last_nonzero_apps.insert(s.key.clone(), s.volume);
            }
        }
        st.audio = new_audio;
        let (total_h, max_sc) = compute_desired_flyout_height(&st);
        st.flyout_height = total_h;
        st.max_scroll = max_sc;
        if st.scroll_offset > st.max_scroll {
            st.scroll_offset = st.max_scroll;
        }
        let w = st.flyout_width;
        drop(st);

        if is_vis && hwnd_flyout.0 != 0 {
            let work = get_active_monitor_work_rect();
            let x = work.right - w - 14;
            let y = work.bottom - total_h - 14;
            let _ = SetWindowPos(hwnd_flyout, HWND_TOPMOST, x, y, w, total_h, SWP_NOACTIVATE);
            let _ = InvalidateRect(hwnd_flyout, None, BOOL(0));
        }
    }
}

// Layered window alpha affects the whole flyout, including text.  Retain
// sufficient contrast while allowing a noticeably more glass-like mixer.
// The settings view stays denser for readability.
unsafe fn set_flyout_opacity(hwnd: HWND, settings_expanded: bool) {
    let alpha = if settings_expanded { 249 } else { 217 };
    let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA);
}

unsafe fn open_flyout(expand_settings: bool) {
    close_custom_tray_menu();
    close_about_window();
    {
        let mut st = STATE.lock();
        st.flyout_visible = true;
        st.dropdown_expanded = false;
        st.settings_expanded = expand_settings;
        st.scroll_offset = 0;
        st.recording_target = None;
        st.rename_device_id = None;
        st.hover_pos = POINT { x: -1, y: -1 };
    }
    refresh_state_and_resize();
    let (hwnd, w, h) = {
        let st = STATE.lock();
        (st.hwnd_flyout, st.flyout_width, st.flyout_height)
    };
    set_flyout_opacity(hwnd, expand_settings);
    let work = get_active_monitor_work_rect();
    let x = work.right - w - 14;
    let y = work.bottom - h - 14;
    let _ = SetWindowPos(hwnd, HWND_TOPMOST, x, y, w, h, SWP_SHOWWINDOW);
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = SetForegroundWindow(hwnd);
    let refresh_ms = if expand_settings {SETTINGS_REFRESH_MS} else {MIXER_REFRESH_MS};
    let _ = SetTimer(hwnd, TIMER_LIVE_SYNC, refresh_ms, None);
    let _ = InvalidateRect(hwnd, None, BOOL(0));
}

unsafe fn hide_flyout() {
    let (hwnd, was_recording, cfg) = {
        let mut st = STATE.lock();
        if !st.flyout_visible {
            return;
        }
        st.flyout_visible = false;
        st.last_hide_at = Some(Instant::now());
        st.dragging_slider = None;
        st.rename_device_id = None;
        st.rename_buffer.clear();
        let rec = st.recording_target.take().is_some();
        (st.hwnd_flyout, rec, st.config.clone())
    };
    flush_drag_pending();
    if was_recording {
        register_global_hotkeys(hwnd, &cfg);
    }
    let _ = KillTimer(hwnd, TIMER_LIVE_SYNC);
    let _ = ShowWindow(hwnd, SW_HIDE);
}

unsafe fn toggle_flyout() {
    let vis = STATE.lock().flyout_visible;
    if vis {
        hide_flyout();
    } else {
        open_flyout(false);
    }
}

unsafe fn show_custom_tray_menu() {
    hide_flyout();
    close_about_window();
    refresh_state_and_resize();
    let hwnd_menu = {
        let mut st = STATE.lock();
        st.menu_visible = true;
        st.menu_hover_index = -1;
        st.hwnd_menu
    };

    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let work = get_active_monitor_work_rect();
    let mw = 390;
    let mh = 368;
    let x = (pt.x - mw + 20).clamp(work.left + 8, work.right - mw - 8);
    let y = (pt.y - mh - 8).clamp(work.top + 8, work.bottom - mh - 8);

    let _ = SetWindowPos(hwnd_menu, HWND_TOPMOST, x, y, mw, mh, SWP_SHOWWINDOW);
    let _ = ShowWindow(hwnd_menu, SW_SHOW);
    let _ = SetForegroundWindow(hwnd_menu);
    let _ = InvalidateRect(hwnd_menu, None, BOOL(0));
}

unsafe fn close_custom_tray_menu() {
    let hwnd_menu = {
        let mut st = STATE.lock();
        if !st.menu_visible {
            return;
        }
        st.menu_visible = false;
        st.hwnd_menu
    };
    let _ = ShowWindow(hwnd_menu, SW_HIDE);
}

// =====================================================================
// Paint Custom Dark Right-Click Tray Menu (Crisp Vector Icons + About v1.0.0)
// =====================================================================
unsafe fn paint_tray_menu(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let w = rc.right - rc.left;
    let h = rc.bottom - rc.top;

    let mem_dc = CreateCompatibleDC(hdc);
    let mem_bm = CreateCompatibleBitmap(hdc, w, h);
    let old_bm = SelectObject(mem_dc, mem_bm);

    fill_round_rect(mem_dc, &rc, 12, COL_MENU_BG, Some(COL_BORDER));

    let f_bold_11 = make_font(18, FW_BOLD.0 as i32);
    let f_bold_8 = make_font(12, FW_BOLD.0 as i32);
    let f_reg_10 = make_font(15, FW_SEMIBOLD.0 as i32);
    let f_sub_9 = make_font(13, FW_NORMAL.0 as i32);

    let (dev_name, dev_icon, hk_mix, hk_sw, hov_idx) = {
        let st = STATE.lock();
        (
            display_device_name(&st.config, &st.audio.current_device_id,
                &st.audio.current_device_name),
            st.audio.current_device_icon.clone(),
            st.config.hotkey_mixer.to_uppercase(),
            st.config.hotkey_switch.to_uppercase(),
            st.menu_hover_index,
        )
    };

    // 1. Brand Header Card with MixSwitch Logo
    let hdr_rc = RECT {
        left: 10,
        top: 10,
        right: w - 10,
        bottom: 83,
    };
    fill_round_rect(mem_dc, &hdr_rc, 11, COL_MENU_HDR, None);

    draw_bgra_sprite(mem_dc, 20, 18, 32, 32, icons_data::LOGO_32);

    draw_text_str(
        mem_dc,
        APP_NAME,
        RECT {
            left: 62,
            top: 17,
            right: w - 85,
            bottom: 36,
        },
        COL_TEXT_MAIN,
        f_bold_11,
        DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
    );

    let badge_rc = RECT {
        left: w - 78,
        top: 18,
        right: w - 18,
        bottom: 39,
    };
    fill_round_rect(mem_dc, &badge_rc, 6, COL_CARD_BG, Some(brand_accent_soft()));
    draw_text_str(
        mem_dc,
        APP_VERSION,
        badge_rc,
        brand_accent_bright(),
        f_bold_8,
        DT_CENTER.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
    );

    // Active device small vector icon + name
    let dev_sp = match dev_icon.as_str() {
        "speaker" => icons_data::IC_SPEAKER_20_WHT,
        "monitor" => icons_data::IC_MONITOR_20_WHT,
        _ => icons_data::IC_HEADPHONES_20_WHT,
    };
    draw_bgra_sprite(mem_dc, 62, 49, 20, 20, dev_sp);

    draw_text_str(
        mem_dc,
        &dev_name,
        RECT {
            left: 86,
            top: 47,
            right: w - 18,
            bottom: 72,
        },
        COL_TEXT_DIM,
        f_sub_9,
        DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_END_ELLIPSIS.0,
    );

    // 5 Menu Rows:
    // 0: Open Volume Mixer
    // 1: Switch Audio Device
    // 2: Settings & Shortcuts
    // 3: About MixSwitch (v1.0.0)
    // 4: Exit MixSwitch
    let items: [(&[u8], &str, &str, bool); 5] = [
        (icons_data::IC_SLIDERS_18_ORG, "Open Volume Mixer", &hk_mix, false),
        (icons_data::IC_SWITCH_18_ORG, "Switch Audio Device", &hk_sw, false),
        (icons_data::IC_GEAR_20_ORG, "Settings & Shortcuts", "", false),
        (icons_data::IC_INFO_18_ORG, "About MixSwitch (v1.0.0)", "", false),
        (icons_data::IC_CLOSE_18_RED, "Exit MixSwitch", "", true),
    ];

    let mut y = 92;
    for (idx, (sprite, label, badge, danger)) in items.iter().enumerate() {
        if idx == 4 {
            let div_rc = RECT {
                left: 14,
                top: y + 4,
                right: w - 14,
                bottom: y + 5,
            };
            let br = CreateSolidBrush(COL_BORDER);
            FillRect(mem_dc, &div_rc, br);
            let _ = DeleteObject(br);
            y += 14;
        }

        let row_rc = RECT {
            left: 10,
            top: y,
            right: w - 10,
            bottom: y + 44,
        };

        if hov_idx == idx as i32 {
            let h_col = if *danger {
                COL_MENU_DANGER_HOV
            } else {
                COL_MENU_HOVER
            };
            fill_round_rect(mem_dc, &row_rc, 8, h_col, None);
        }

        let sp_sz = if idx == 2 { 20 } else { 18 };
        draw_bgra_sprite(mem_dc, 20, y + (44 - sp_sz) / 2, sp_sz, sp_sz, sprite);

        draw_text_str(
            mem_dc,
            label,
            RECT {
                left: 48,
                top: y,
                right: if badge.is_empty() { w - 18 } else { w - 108 },
                bottom: y + 44,
            },
            if *danger { COL_MUTE } else { COL_TEXT_MAIN },
            f_reg_10,
            DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
        );

        if !badge.is_empty() {
            // Generous badge width so PAGE_DOWN or CTRL+SHIFT+F12 fits cleanly without clipping!
            let char_w = (badge.len() as i32 * 7).max(46) + 16;
            let b_rc = RECT {
                left: w - 16 - char_w,
                top: y + 10,
                right: w - 16,
                bottom: y + 35,
            };
            fill_round_rect(mem_dc, &b_rc, 6, COL_DEV_BTN_BG, Some(COL_BORDER));
            draw_text_str(
                mem_dc,
                badge,
                b_rc,
                brand_accent_bright(),
                f_bold_8,
                DT_CENTER.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
            );
        }

        y += 48;
    }

    let _ = BitBlt(hdc, 0, 0, w, h, mem_dc, 0, 0, SRCCOPY);
    SelectObject(mem_dc, old_bm);
    let _ = DeleteObject(mem_bm);
    let _ = DeleteDC(mem_dc);
    let _ = DeleteObject(f_bold_11);
    let _ = DeleteObject(f_bold_8);
    let _ = DeleteObject(f_reg_10);
    let _ = DeleteObject(f_sub_9);
    let _ = EndPaint(hwnd, &ps);
}

// =====================================================================
// Paint Independent Top-Left OSD Window (Clean Modern Design + Vector Icon)
// =====================================================================
unsafe fn paint_osd_window(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let w = rc.right - rc.left;
    let h = rc.bottom - rc.top;

    let mem_dc = CreateCompatibleDC(hdc);
    let mem_bm = CreateCompatibleBitmap(hdc, w, h);
    let old_bm = SelectObject(mem_dc, mem_bm);

    fill_round_rect(mem_dc, &rc, 14, COL_OSD_BG, Some(COL_BORDER));

    // Left cyan brand indicator
    let accent_pill = RECT {
        left: 10,
        top: 14,
        right: 14,
        bottom: h - 14,
    };
    fill_round_rect(mem_dc, &accent_pill, 4, brand_accent(), None);

    // Rounded icon container box
    let ic_box = RECT {
        left: 23,
        top: (h - 46) / 2,
        right: 69,
        bottom: (h + 46) / 2,
    };
    fill_round_rect(mem_dc, &ic_box, 10, COL_CARD_BG, None);

    let (title, subtitle, icon_type) = {
        let st = STATE.lock();
        (
            st.osd_title.clone(),
            st.osd_subtitle.clone(),
            st.audio.current_device_icon.clone(),
        )
    };

    let sp = match icon_type.as_str() {
        "speaker" => icons_data::IC_SPEAKER_32_ORG,
        "monitor" => icons_data::IC_MONITOR_32_ORG,
        _ => icons_data::IC_HEADPHONES_32_ORG,
    };
    draw_bgra_sprite(mem_dc, 30, (h - 32) / 2, 32, 32, sp);

    let f_title = make_font(14, FW_BOLD.0 as i32);
    let f_sub = make_font(18, FW_BOLD.0 as i32);

    let title_col = if title.contains("Conflict") {
        COL_MUTE
    } else {
        brand_accent_bright()
    };
    draw_text_str(
        mem_dc,
        &title.to_uppercase(),
        RECT {
            left: 79,
            top: 12,
            right: w - 14,
            bottom: 34,
        },
        title_col,
        f_title,
        DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
    );

    draw_text_str(
        mem_dc,
        &subtitle,
        RECT {
            left: 79,
            top: 34,
            right: w - 14,
            bottom: h - 11,
        },
        COL_TEXT_MAIN,
        f_sub,
        DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_END_ELLIPSIS.0,
    );

    let _ = BitBlt(hdc, 0, 0, w, h, mem_dc, 0, 0, SRCCOPY);
    SelectObject(mem_dc, old_bm);
    let _ = DeleteObject(mem_bm);
    let _ = DeleteDC(mem_dc);
    let _ = DeleteObject(f_title);
    let _ = DeleteObject(f_sub);
    let _ = EndPaint(hwnd, &ps);
}

// =====================================================================
// Paint Dedicated About MixSwitch Dialog Window
// =====================================================================
unsafe fn paint_about_window(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let w = rc.right - rc.left;
    let h = rc.bottom - rc.top;

    let mem_dc = CreateCompatibleDC(hdc);
    let mem_bm = CreateCompatibleBitmap(hdc, w, h);
    let old_bm = SelectObject(mem_dc, mem_bm);

    fill_round_rect(mem_dc, &rc, 14, COL_BODY_BG, Some(brand_accent_soft()));

    // Draw 64x64 MixSwitch Logo centered at top
    draw_bgra_sprite(mem_dc, (w - 64) / 2, 22, 64, 64, icons_data::LOGO_64);

    let f_title = make_font(25, FW_BOLD.0 as i32);
    let f_badge = make_font(12, FW_BOLD.0 as i32);
    let f_desc = make_font(17, FW_NORMAL.0 as i32);
    let f_btn = make_font(16, FW_BOLD.0 as i32);

    draw_text_str(
        mem_dc,
        APP_NAME,
        RECT {
            left: 20,
            top: 94,
            right: w - 20,
            bottom: 122,
        },
        COL_TEXT_MAIN,
        f_title,
        DT_CENTER.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
    );

    let pill_rc = RECT {
        left: (w - 130) / 2,
        top: 126,
        right: (w + 130) / 2,
        bottom: 148,
    };
    fill_round_rect(mem_dc, &pill_rc, 8, COL_CARD_BG, Some(brand_accent_soft()));
    draw_text_str(
        mem_dc,
        &format!("{}  •  Pure Rust", APP_VERSION),
        pill_rc,
        brand_accent_bright(),
        f_badge,
        DT_CENTER.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
    );

    let desc_txt = "Native per-app volume control\n& instant playback device switching.\nWindows Core Audio • Rust / Win32\nDesigned to run quietly in the system tray";
    draw_text_str(
        mem_dc,
        desc_txt,
        RECT {
            left: 24,
            top: 162,
            right: w - 24,
            bottom: 267,
        },
        COL_TEXT_DIM,
        f_desc,
        DT_CENTER.0 | DT_WORDBREAK.0,
    );

    let close_rc = RECT {
        left: (w - 110) / 2,
        top: h - 46,
        right: (w + 110) / 2,
        bottom: h - 16,
    };
    fill_round_rect(mem_dc, &close_rc, 7, brand_accent(), None);
    draw_text_str(
        mem_dc,
        "Close",
        close_rc,
        COL_TEXT_MAIN,
        f_btn,
        DT_CENTER.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
    );

    let _ = BitBlt(hdc, 0, 0, w, h, mem_dc, 0, 0, SRCCOPY);
    SelectObject(mem_dc, old_bm);
    let _ = DeleteObject(mem_bm);
    let _ = DeleteDC(mem_dc);
    let _ = DeleteObject(f_title);
    let _ = DeleteObject(f_badge);
    let _ = DeleteObject(f_desc);
    let _ = DeleteObject(f_btn);
    let _ = EndPaint(hwnd, &ps);
}

// =====================================================================
// Paint Main Mixer Flyout Window (Double-Buffered GDI + Vector Icons)
// =====================================================================
unsafe fn draw_modern_slider(
    hdc: HDC,
    track_rc: RECT,
    value: u32,
    peak: u32,
    muted: bool,
    accent: COLORREF,
) {
    let cy = (track_rc.top + track_rc.bottom) / 2;
    let usable_w = (track_rc.right - track_rc.left).max(1);
    let thumb_x = track_rc.left + ((value.min(100) as i32 * usable_w) / 100);
    let is_silent = muted || value == 0;
    let active_col = if is_silent {
        // Matched muted red for both the slider and its status icon.
        COL_MUTE
    } else { accent };

    // Background track
    let bg_bar = RECT {
        left: track_rc.left,
        top: cy - 4,
        right: track_rc.right,
        bottom: cy + 4,
    };
    fill_round_rect(hdc, &bg_bar, 4, COL_TRACK, None);

    // Active filled bar
    if thumb_x > track_rc.left {
        let fg_bar = RECT {
            left: track_rc.left,
            top: cy - 4,
            right: thumb_x,
            bottom: cy + 4,
        };
        fill_round_rect(hdc, &fg_bar, 4, active_col, None);
    }

    // Live Green Peak Meter Bar underneath
    if !is_silent && peak > 0 {
        let peak_x = track_rc.left + ((peak.min(100) as i32 * usable_w) / 100);
        let pk_bar = RECT {
            left: track_rc.left,
            top: cy + 7,
            right: peak_x,
            bottom: cy + 10,
        };
        fill_round_rect(hdc, &pk_bar, 2, COL_PEAK_GREEN, None);
    }

    // Pill Thumb
    let thumb_rc = RECT {
        left: thumb_x - 7,
        top: cy - 14,
        right: thumb_x + 7,
        bottom: cy + 14,
    };
    fill_round_rect(hdc, &thumb_rc, 7, active_col, None);
}

unsafe fn paint_flyout_window(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let w = rc.right - rc.left;
    let h = rc.bottom - rc.top;

    let mem_dc = CreateCompatibleDC(hdc);
    let mem_bm = CreateCompatibleBitmap(hdc, w, h);
    let old_bm = SelectObject(mem_dc, mem_bm);

    // Background Body
    let body_brush = CreateSolidBrush(COL_BODY_BG);
    FillRect(mem_dc, &rc, body_brush);
    let _ = DeleteObject(body_brush);

    let f_bold_11 = make_font(21, FW_BOLD.0 as i32);
    let f_bold_9 = make_font(15, FW_BOLD.0 as i32);
    let f_bold_8 = make_font(13, FW_BOLD.0 as i32);
    let f_reg_10 = make_font(16, FW_NORMAL.0 as i32);
    let f_val_13 = make_font(21, FW_SEMIBOLD.0 as i32);

    let mut hitboxes: Vec<UiHitBox> = Vec::new();

    let mut st = STATE.lock();
    let hov = st.hover_pos;

    let mut y = 12;

    // Estimate Header background height
    let mut est_hdr_bottom = y + 64 + 54;
    if st.dropdown_expanded {
        est_hdr_bottom += 58 + (st.audio.devices.len() as i32) * 44;
    }
    if st.settings_expanded {
        est_hdr_bottom += SETTINGS_BASE_HEIGHT +
            (st.audio.devices.len() as i32) * SETTINGS_DEVICE_ROW_HEIGHT;
    }
    let hdr_bg_rc = RECT {
        left: 1,
        top: 1,
        right: w - 1,
        bottom: est_hdr_bottom,
    };
    let hdr_brush = CreateSolidBrush(COL_HEADER_BG);
    FillRect(mem_dc, &hdr_bg_rc, hdr_brush);
    let _ = DeleteObject(hdr_brush);

    // 1. Device Selector Bar
    let dev_bar_rc = RECT {
        left: 12,
        top: y,
        right: w - 12,
        bottom: y + 56,
    };
    fill_round_rect(mem_dc, &dev_bar_rc, 8, COL_DEV_BTN_BG, None);

    // Larger device glyph is decorative in the mixer, not an edit control.
    let dev_sp = match st.audio.current_device_icon.as_str() {
        "speaker" => icons_data::IC_SPEAKER_32_ORG,
        "monitor" => icons_data::IC_MONITOR_32_ORG,
        _ => icons_data::IC_HEADPHONES_32_ORG,
    };
    draw_bgra_sprite(mem_dc, 23, y + 12, 32, 32, dev_sp);

    // Settings Gear Button (rightmost)
    let gear_rc = RECT {
        left: w - 54,
        top: y + 8,
        right: w - 14,
        bottom: y + 48,
    };
    let gear_bg = if st.settings_expanded {
        COL_DROPDOWN_ACTIVE
    } else if pt_in_rect(&gear_rc, hov.x, hov.y) {
        COL_DEV_BTN_HOVER
    } else {
        COL_DEV_BTN_BG
    };
    fill_round_rect(mem_dc, &gear_rc, 7, gear_bg, None);
    draw_bgra_sprite(mem_dc, gear_rc.left + 6, gear_rc.top + 6,
        28, 28, icons_data::GEAR_CYAN_28);
    hitboxes.push(UiHitBox {
        rect: gear_rc,
        target: HitTarget::SettingsToggleBtn,
    });

    // Quick Switch Button
    let sw_rc = RECT {
        left: w - 143,
        top: y + 9,
        right: w - 62,
        bottom: y + 47,
    };
    let sw_hov = pt_in_rect(&sw_rc, hov.x, hov.y);
    fill_round_rect(
        mem_dc,
        &sw_rc,
        7,
        if sw_hov {
            brand_accent()
        } else {
            COL_DROPDOWN_ACTIVE
        },
        None,
    );
    draw_bgra_sprite(mem_dc, sw_rc.left + 8, sw_rc.top + 7, 16, 16, icons_data::IC_SWITCH_16_WHT);
    draw_text_str(
        mem_dc,
        "Switch",
        RECT {
            left: sw_rc.left + 26,
            top: sw_rc.top,
            right: sw_rc.right - 6,
            bottom: sw_rc.bottom,
        },
        COL_TEXT_MAIN,
        f_bold_9,
        DT_CENTER.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
    );
    hitboxes.push(UiHitBox {
        rect: sw_rc,
        target: HitTarget::QuickSwitchBtn,
    });

    // Device Name + Chevron Dropdown Toggle
    let dd_rc = RECT {
        left: 66,
        top: y + 8,
        right: w - 153,
        bottom: y + 48,
    };
    draw_text_str(
        mem_dc,
        &display_device_name(&st.config, &st.audio.current_device_id,
            &st.audio.current_device_name),
        RECT {
            left: dd_rc.left,
            top: dd_rc.top,
            right: dd_rc.right - 20,
            bottom: dd_rc.bottom,
        },
        COL_TEXT_MAIN,
        f_bold_11,
        DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_END_ELLIPSIS.0,
    );
    draw_text_str(
        mem_dc,
        if st.dropdown_expanded { "▴" } else { "▾" },
        RECT {
            left: dd_rc.right - 18,
            top: dd_rc.top,
            right: dd_rc.right,
            bottom: dd_rc.bottom,
        },
        COL_TEXT_DIM,
        f_bold_11,
        DT_CENTER.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
    );
    hitboxes.push(UiHitBox {
        rect: dd_rc,
        target: HitTarget::DeviceDropdownToggle,
    });

    y += 64;

    // 2. Expanded Playback Device Dropdown
    if st.dropdown_expanded && !st.audio.devices.is_empty() {
        let panel_h = 46 + (st.audio.devices.len() as i32) * 44;
        let panel_rc = RECT {
            left: 16,
            top: y,
            right: w - 16,
            bottom: y + panel_h,
        };
        fill_round_rect(mem_dc, &panel_rc, 8, COL_DROPDOWN_BG, None);

        draw_text_str(
            mem_dc,
            "SELECT PLAYBACK DEVICE",
            RECT {
                left: 26,
                top: y + 6,
                right: w - 26,
                bottom: y + 24,
            },
            COL_TEXT_DIM,
            f_bold_8,
            DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
        );

        let mut dy = y + 36;
        let devs = st.audio.devices.clone();
        for dev in devs {
            let item_rc = RECT {
                left: 22,
                top: dy,
                right: w - 22,
                bottom: dy + 40,
            };
            let is_hov = pt_in_rect(&item_rc, hov.x, hov.y);
            if dev.is_default || is_hov {
                fill_round_rect(
                    mem_dc,
                    &item_rc,
                    6,
                    if dev.is_default {
                        COL_DROPDOWN_ACTIVE
                    } else {
                        COL_DEV_BTN_BG
                    },
                    None,
                );
            }
            let sp = match dev.icon_type.as_str() {
                "speaker" => icons_data::IC_SPEAKER_20_WHT,
                "monitor" => icons_data::IC_MONITOR_20_WHT,
                _ => icons_data::IC_HEADPHONES_20_WHT,
            };
            draw_bgra_sprite(mem_dc, 30, dy + 10, 20, 20, sp);

            draw_text_str(
                mem_dc,
                &display_device_name(&st.config, &dev.id, &dev.name),
                RECT {
                    left: 58,
                    top: dy,
                    right: w - 30,
                    bottom: dy + 40,
                },
                if dev.is_default {
                    COL_TEXT_MAIN
                } else {
                    COL_TEXT_DIM
                },
                if dev.is_default { f_bold_9 } else { f_reg_10 },
                DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_END_ELLIPSIS.0,
            );
            hitboxes.push(UiHitBox {
                rect: item_rc,
                target: HitTarget::SelectDevice(dev.id),
            });
            dy += 44;
        }
        y += panel_h + 12;
    }

    // 3. Expanded Inline Settings Panel
    if st.settings_expanded {
        // Remember the fixed header hitboxes, which remain clickable when scrolling.
        let fixed_header_hits = hitboxes.len();
        let panel_h = SETTINGS_BASE_HEIGHT +
            (st.audio.devices.len() as i32) * SETTINGS_DEVICE_ROW_HEIGHT;
        let panel_rc = RECT {
            left: 16,
            top: y,
            right: w - 16,
            bottom: y + panel_h,
        };
        fill_round_rect(mem_dc, &panel_rc, 8, COL_SETTINGS_BG, None);
        let saved_clip = SaveDC(mem_dc);
        let _ = IntersectClipRect(mem_dc, 16, y, w - 16, h - 6);

        let mut sy = y + 16 - st.scroll_offset;
        draw_text_str(
            mem_dc,
            "Settings",
            RECT { left: 26, top: sy, right: w - 26, bottom: sy + 28 },
            COL_TEXT_MAIN,
            f_bold_11,
            DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
        );
        sy += 40;
        draw_text_str(
            mem_dc,
            "KEYBOARD SHORTCUTS",
            RECT {
                left: 26,
                top: sy,
                right: w - 26,
                bottom: sy + 18,
            },
            COL_TEXT_DIM,
            f_bold_8,
            DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
        );
        sy += 23;
        draw_text_str(mem_dc, "Click a shortcut to record a new key",
            RECT { left: 26, top: sy, right: w-26, bottom: sy+20 },
            brand_accent_bright(), f_bold_8,
            DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0);
        sy += 27;

        // Row 1: Open Mixer Hotkey
        draw_text_str(
            mem_dc,
            "Open Mixer Flyout:",
            RECT {
                left: 26,
                top: sy,
                right: w - 150,
                bottom: sy + 44,
            },
            COL_TEXT_MAIN,
            f_reg_10,
            DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
        );
        let hk1_rc = RECT {
            left: w - 144,
            top: sy + 3,
            right: w - 26,
            bottom: sy + 31,
        };
        let is_rec1 = st.recording_target.as_deref() == Some("hotkey_mixer");
        fill_round_rect(
            mem_dc,
            &hk1_rc,
            6,
            if is_rec1 { brand_accent_soft() } else { COL_CARD_HOVER },
            Some(if is_rec1 { brand_accent() } else { brand_accent_soft() }),
        );
        let hk1_txt = if is_rec1 {
            "Press key...".to_string()
        } else {
            st.config.hotkey_mixer.to_uppercase()
        };
        draw_text_str(
            mem_dc,
            &hk1_txt,
            hk1_rc,
            if is_rec1 {
                COL_TEXT_MAIN
            } else {
                brand_accent_bright()
            },
            f_bold_9,
            DT_CENTER.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
        );
        hitboxes.push(UiHitBox {
            rect: hk1_rc,
            target: HitTarget::RecordHotkeyMixer,
        });
        sy += 36;

        // Row 2: Switch Audio Device Hotkey
        draw_text_str(
            mem_dc,
            "Switch Audio Device:",
            RECT {
                left: 26,
                top: sy,
                right: w - 150,
                bottom: sy + 44,
            },
            COL_TEXT_MAIN,
            f_reg_10,
            DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
        );
        let hk2_rc = RECT {
            left: w - 144,
            top: sy + 3,
            right: w - 26,
            bottom: sy + 31,
        };
        let is_rec2 = st.recording_target.as_deref() == Some("hotkey_switch");
        fill_round_rect(
            mem_dc,
            &hk2_rc,
            6,
            if is_rec2 { brand_accent_soft() } else { COL_CARD_HOVER },
            Some(if is_rec2 { brand_accent() } else { brand_accent_soft() }),
        );
        let hk2_txt = if is_rec2 {
            "Press key...".to_string()
        } else {
            st.config.hotkey_switch.to_uppercase()
        };
        draw_text_str(
            mem_dc,
            &hk2_txt,
            hk2_rc,
            if is_rec2 {
                COL_TEXT_MAIN
            } else {
                brand_accent_bright()
            },
            f_bold_9,
            DT_CENTER.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
        );
        hitboxes.push(UiHitBox {
            rect: hk2_rc,
            target: HitTarget::RecordHotkeySwitch,
        });
        sy += 36;

        // Row 3: Show System Sounds
        draw_text_str(
            mem_dc,
            "Show System Sounds Row:",
            RECT {
                left: 26,
                top: sy,
                right: w - 110,
                bottom: sy + 44,
            },
            COL_TEXT_MAIN,
            f_reg_10,
            DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
        );
        let sys_rc = RECT {
            left: w - 96,
            top: sy + 3,
            right: w - 26,
            bottom: sy + 31,
        };
        fill_round_rect(mem_dc, &sys_rc, 6, COL_CARD_BG, None);
        draw_text_str(
            mem_dc,
            if st.config.show_system_sounds {
                "ON"
            } else {
                "OFF"
            },
            sys_rc,
            if st.config.show_system_sounds {
                COL_GREEN_ON
            } else {
                COL_TEXT_DIM
            },
            f_bold_9,
            DT_CENTER.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
        );
        hitboxes.push(UiHitBox {
            rect: sys_rc,
            target: HitTarget::ToggleSystemSounds,
        });
        sy += 42;

        draw_text_str(
            mem_dc,
            "PLAYBACK DEVICES",
            RECT {
                left: 26,
                top: sy,
                right: w - 26,
                bottom: sy + 18,
            },
            COL_TEXT_DIM,
            f_bold_8,
            DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
        );
        sy += 28;

        draw_text_str(mem_dc, "Pencil: rename  •  Device icon: click to change",
            RECT {left: 26, top: sy, right: w-26, bottom: sy+16},
            brand_accent_bright(), f_bold_8,
            DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0);
        sy += 16;

        // Each device has a dedicated selection hitbox, a rename button and an icon button.
        let devs = st.audio.devices.clone();
        let f_device = make_font(16, FW_SEMIBOLD.0 as i32);
        for dev in devs {
            let row = RECT {left: 24, top: sy, right: w-24, bottom: sy+52};
            fill_round_rect(mem_dc, &row, 8,
                if pt_in_rect(&row, hov.x, hov.y) {COL_CARD_HOVER} else {COL_CARD_BG},
                Some(COL_BORDER));

            let chk_rc = RECT {left: 29, top: sy+12, right: 53, bottom: sy+40};
            let included = !dev.excluded;
            let chk_sp = if included {icons_data::IC_CHECK_ON_20}
                         else {icons_data::IC_CHECK_OFF_20};
            draw_bgra_sprite(mem_dc, 31, sy+15, 20,20,chk_sp);
            hitboxes.push(UiHitBox { rect: chk_rc,
                target: HitTarget::ToggleDeviceInclude(dev.id.clone()) });

            let alias = display_device_name(&st.config, &dev.id, &dev.name);
            let editing = st.rename_device_id.as_deref() == Some(dev.id.as_str());
            let line = if editing {
                format!("{}|", st.rename_buffer)
            } else { alias };
            draw_text_str(mem_dc, &line,
                RECT {left: 60, top: sy+2, right: w-190, bottom: sy+27},
                if editing {brand_accent_bright()} else {COL_TEXT_MAIN},
                f_device, DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_END_ELLIPSIS.0);
            draw_text_str(mem_dc,
                if editing { "Enter: save · Empty: reset" } else { &dev.name },
                RECT {left: 60, top: sy+27, right: w-190, bottom: sy+47},
                COL_TEXT_DIM, f_bold_8,
                DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_END_ELLIPSIS.0);

            let rename_rc = RECT {left: w-178, top:sy+10, right:w-91,bottom:sy+42};
            let rename_hover = pt_in_rect(&rename_rc, hov.x,hov.y);
            fill_round_rect(mem_dc,&rename_rc,7,
                if rename_hover || editing {COL_DROPDOWN_ACTIVE} else {COL_DEV_BTN_BG},
                Some(if editing {brand_accent()} else {COL_BORDER}));
            draw_text_str(mem_dc, if editing {"Editing…"} else {"Rename"},
                rename_rc, brand_accent_bright(), f_bold_8,
                DT_CENTER.0 | DT_VCENTER.0 | DT_SINGLELINE.0);
            hitboxes.push(UiHitBox {rect: rename_rc,
                target: HitTarget::RenameDevice(dev.id.clone())});

            let cyc_rc = RECT {left:w-82, top:sy+9, right:w-30, bottom:sy+43};
            let is_cyc_hov = pt_in_rect(&cyc_rc, hov.x, hov.y);
            fill_round_rect(mem_dc,&cyc_rc,7,
                if is_cyc_hov {COL_DROPDOWN_ACTIVE} else {COL_DEV_BTN_BG},
                Some(if is_cyc_hov {brand_accent()} else {COL_BORDER}));
            let dev_sp = match dev.icon_type.as_str() {
                "speaker" => icons_data::IC_SPEAKER_24_ORG,
                "monitor" => icons_data::IC_MONITOR_24_ORG,
                _ => icons_data::IC_HEADPHONES_24_ORG,
            };
            draw_bgra_sprite(mem_dc,cyc_rc.left+14,cyc_rc.top+5,24,24,dev_sp);
            hitboxes.push(UiHitBox {rect:cyc_rc,
                target:HitTarget::CycleDeviceIcon(dev.id.clone(),dev.icon_type.clone())});
            sy += SETTINGS_DEVICE_ROW_HEIGHT;
        }
        let _ = DeleteObject(f_device);

        // Startup is strictly opt-in: no registration until the user enables it.
        sy += 8;
        draw_text_str(mem_dc, "GENERAL", RECT {left:26,top:sy,right:w-26,bottom:sy+17},
            COL_TEXT_DIM, f_bold_8,DT_LEFT.0|DT_VCENTER.0|DT_SINGLELINE.0);
        sy += 22;
        draw_text_str(mem_dc, "Launch at Windows Startup",
            RECT {left:26,top:sy,right:w-136,bottom:sy+36},
            COL_TEXT_MAIN,f_reg_10,DT_LEFT.0|DT_VCENTER.0|DT_SINGLELINE.0);
        let start_rc=RECT {left:w-116,top:sy+2,right:w-26,bottom:sy+32};
        fill_round_rect(mem_dc,&start_rc,6,COL_CARD_BG,Some(COL_BORDER));
        draw_text_str(mem_dc,if st.config.launch_at_startup {"ON"} else {"OFF"},
            start_rc,if st.config.launch_at_startup {COL_GREEN_ON} else {COL_TEXT_DIM},
            f_bold_9,DT_CENTER.0|DT_VCENTER.0|DT_SINGLELINE.0);
        hitboxes.push(UiHitBox {rect:start_rc,target:HitTarget::ToggleStartup});
        sy += 38;

        // Settings Footer: MixSwitch v1.0.0 + About Button
        sy += 8;
        draw_bgra_sprite(mem_dc, 26, sy + 1, 32, 32, icons_data::LOGO_32);
        draw_text_str(
            mem_dc,
            &format!("{} {}", APP_NAME, APP_VERSION),
            RECT {
                left: 66,
                top: sy,
                right: w - 110,
                bottom: sy + 34,
            },
            COL_TEXT_DIM,
            f_bold_9,
            DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
        );

        let abt_rc = RECT {
            left: w - 102,
            top: sy + 3,
            right: w - 26,
            bottom: sy + 34,
        };
        fill_round_rect(mem_dc, &abt_rc, 6, COL_CARD_BG, Some(COL_BORDER));
        draw_text_str(
            mem_dc,
            "About",
            abt_rc,
            brand_accent_bright(),
            f_bold_9,
            DT_CENTER.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
        );
        hitboxes.push(UiHitBox {
            rect: abt_rc,
            target: HitTarget::OpenAboutDialog,
        });

        let _ = RestoreDC(mem_dc, saved_clip);
        // Scrolled-out settings rows must not intercept clicks on the fixed header.
        let mut index = 0;
        hitboxes.retain(|hb| {
            let keep = index < fixed_header_hits || (hb.rect.top >= y && hb.rect.bottom <= h);
            index += 1;
            keep
        });
        y += panel_h + 10;
    }

    // Settings mode contains no master or application mixer rows.
    if !st.settings_expanded {
    // 4. Master Volume Row (Crisp Vector Speaker Icon)
    let m_silent = st.audio.master_muted || st.audio.master_volume == 0;
    let m_icon_rc = RECT {
        left: 12,
        top: y + 2,
        right: 58,
        bottom: y + 50,
    };
    if m_silent {
        draw_bgra_sprite(mem_dc, 20, y + 10, 30, 30, icons_data::MUTE_30);
    } else {
        draw_bgra_sprite(mem_dc, 23, y + 14, 24, 24, icons_data::IC_VOL_ON_24);
    }
    hitboxes.push(UiHitBox {
        rect: m_icon_rc,
        target: HitTarget::MasterMuteToggle,
    });

    let m_track_rc = RECT {
        left: 68,
        top: y + 2,
        right: w - 84,
        bottom: y + 50,
    };
    draw_modern_slider(
        mem_dc,
        m_track_rc,
        st.audio.master_volume,
        st.audio.master_peak,
        m_silent,
        brand_accent(),
    );
    hitboxes.push(UiHitBox {
        rect: m_track_rc,
        target: HitTarget::MasterSlider(m_track_rc),
    });

    let m_val_rc = RECT {
        left: w - 78,
        top: y + 2,
        right: w - 13,
        bottom: y + 50,
    };
    if m_silent {
        draw_bgra_sprite(mem_dc, w - 55, y + 10, 30, 30, icons_data::MUTE_30);
    } else {
        draw_text_str(mem_dc, &st.audio.master_volume.to_string(), m_val_rc,
            COL_TEXT_MAIN, f_val_13, DT_RIGHT.0 | DT_VCENTER.0 | DT_SINGLELINE.0);
    }
    hitboxes.push(UiHitBox {
        rect: m_val_rc,
        target: HitTarget::MasterMuteToggle,
    });

    y += 54;

    // Header Bottom Divider
    let div_rc = RECT {
        left: 1,
        top: y,
        right: w - 1,
        bottom: y + 1,
    };
    let b_br = CreateSolidBrush(COL_BORDER);
    FillRect(mem_dc, &div_rc, b_br);
    let _ = DeleteObject(b_br);

    let body_top = y + 1;
    let mut app_y = body_top + 6 - st.scroll_offset;

    // 5. Application Audio Sessions
    let sessions = st.audio.sessions.clone();
    if sessions.is_empty() {
        draw_text_str(mem_dc, "No active application audio",
            RECT { left: 30, top: body_top + 20, right: w - 30, bottom: body_top + 55 },
            COL_TEXT_DIM, f_reg_10,
            DT_CENTER.0 | DT_VCENTER.0 | DT_SINGLELINE.0);
    }
    let mut hovered_app: Option<(String, i32)> = None;
    for s in sessions {
        if app_y + COMPACT_BODY_ROW >= body_top && app_y <= h {
            let s_silent = s.muted || s.volume == 0;
            let ic_rc = RECT {
                left: 14,
                top: app_y + 7,
                right: 55,
                bottom: app_y + 49,
            };

            if s.key == "whatsapp_group" {
                draw_bgra_sprite(mem_dc, ic_rc.left, ic_rc.top,
                    34, 34, icons_data::WHATSAPP_34);
            } else if s.hicon != 0 {
                let _ = DrawIconEx(
                    mem_dc,
                    ic_rc.left,
                    ic_rc.top,
                    HICON(s.hicon),
                    34,
                    34,
                    0,
                    None,
                    DI_NORMAL,
                );
            } else {
                draw_bgra_sprite(mem_dc, ic_rc.left + 1, ic_rc.top + 1, 32, 32, icons_data::IC_SYS_SOUNDS_32);
            }
            if s_silent {
                fade_muted_icon(mem_dc, RECT {left:ic_rc.left,top:ic_rc.top,
                    right:ic_rc.left+34,bottom:ic_rc.top+34});
            }
            hitboxes.push(UiHitBox {
                rect: ic_rc,
                target: HitTarget::AppMuteToggle(s.key.clone()),
            });
            if pt_in_rect(&ic_rc, hov.x, hov.y) {
                hovered_app = Some((s.name.clone(), app_y));
            }


            let s_track_rc = RECT {
                left: 68,
                top: app_y + 4,
                right: w - 84,
                bottom: app_y + 52,
            };
            draw_modern_slider(mem_dc, s_track_rc, s.volume, s.peak, s_silent, brand_accent());
            hitboxes.push(UiHitBox {
                rect: s_track_rc,
                target: HitTarget::AppSlider(s.key.clone(), s_track_rc),
            });

            let s_val_rc = RECT {
                left: w - 78,
                top: app_y + 4,
                right: w - 13,
                bottom: app_y + 52,
            };
            if s_silent {
                draw_bgra_sprite(mem_dc, w - 55, app_y + 13, 30, 30, icons_data::MUTE_30);
            } else {
                draw_text_str(mem_dc, &s.volume.to_string(), s_val_rc,
                    COL_TEXT_MAIN, f_val_13, DT_RIGHT.0 | DT_VCENTER.0 | DT_SINGLELINE.0);
            }
            hitboxes.push(UiHitBox {
                rect: s_val_rc,
                target: HitTarget::AppMuteToggle(s.key.clone()),
            });
        }
        app_y += COMPACT_BODY_ROW;
    }

    // Accessible app identification without permanent labels in the flyout.
    if let Some((name, row_y)) = hovered_app {
        let tip_w = ((name.chars().count() as i32) * 8 + 28).clamp(92, w - 95);
        let tip_y = (row_y - 24).clamp(body_top + 2, h - 34);
        let tip_rc = RECT { left: 70, top: tip_y, right: 70 + tip_w, bottom: tip_y + 29 };
        fill_round_rect(mem_dc, &tip_rc, 8, COL_DROPDOWN_ACTIVE, Some(COL_BORDER));
        draw_text_str(mem_dc, &name,
            RECT {left: tip_rc.left + 9, top: tip_rc.top, right: tip_rc.right - 9, bottom: tip_rc.bottom},
            COL_TEXT_MAIN, f_bold_9,
            DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_END_ELLIPSIS.0);
    }

    } // End dedicated mixer mode.

    // Outer 1px frame border
    let pen = CreatePen(PS_SOLID, 1, COL_BORDER);
    let null_b =
        windows::Win32::Graphics::Gdi::GetStockObject(windows::Win32::Graphics::Gdi::NULL_BRUSH);
    let old_p = SelectObject(mem_dc, pen);
    let old_b = SelectObject(mem_dc, null_b);
    let _ = RoundRect(mem_dc, 0, 0, w, h, 12, 12);
    SelectObject(mem_dc, old_p);
    SelectObject(mem_dc, old_b);
    let _ = DeleteObject(pen);

    st.hitboxes = hitboxes;
    drop(st);

    let _ = BitBlt(hdc, 0, 0, w, h, mem_dc, 0, 0, SRCCOPY);
    SelectObject(mem_dc, old_bm);
    let _ = DeleteObject(mem_bm);
    let _ = DeleteDC(mem_dc);
    let _ = DeleteObject(f_bold_11);
    let _ = DeleteObject(f_bold_9);
    let _ = DeleteObject(f_bold_8);
    let _ = DeleteObject(f_reg_10);
    let _ = DeleteObject(f_val_13);
    let _ = EndPaint(hwnd, &ps);
}

// =====================================================================
// Slider Drag Calculation & Key Recording
// =====================================================================
unsafe fn apply_slider_drag(target_key: &str, track_rc: RECT, mouse_x: i32, force: bool) {
    let usable = (track_rc.right - track_rc.left).max(1);
    let rel = (mouse_x - track_rc.left).clamp(0, usable);
    let new_val = ((rel * 100) as f32 / usable as f32).round().clamp(0.0, 100.0) as u32;

    let do_apply = {
        let mut st = STATE.lock();
        if target_key == "master" {
            st.audio.master_volume = new_val;
            st.audio.master_muted = new_val == 0;
            if new_val > 0 {
                st.last_nonzero_master = new_val;
            }
        } else {
            if new_val > 0 {
                st.last_nonzero_apps.insert(target_key.to_string(), new_val);
            }
            for s in &mut st.audio.sessions {
                if s.key == target_key {
                    s.volume = new_val;
                    s.muted = new_val == 0;
                }
            }
        }
        st.drag_pending = Some((target_key.to_string(), new_val));
        force || st.last_drag_apply.elapsed() >= Duration::from_millis(40)
    };

    if do_apply {
        flush_drag_pending();
    }
}

unsafe fn flush_drag_pending() {
    let pending = {
        let mut st = STATE.lock();
        let p = st.drag_pending.take();
        if p.is_some() {
            st.last_drag_apply = Instant::now();
        }
        p
    };
    if let Some((key, val)) = pending {
        if key == "master" {
            let _ = audio::set_master_volume(val);
        } else {
            let _ = audio::set_session_volume(&key, val);
        }
    }
}

unsafe fn handle_hotkey_recording(hwnd: HWND, vk: u32) {
    if vk == 0x1B {
        let cfg = {
            let mut st = STATE.lock();
            st.recording_target = None;
            st.config.clone()
        };
        register_global_hotkeys(hwnd, &cfg);
        let _ = InvalidateRect(hwnd, None, BOOL(0));
        return;
    }

    if [0x10, 0x11, 0x12, 0x5B, 0x5C, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5].contains(&vk) {
        return;
    }

    let mut mods: Vec<&str> = Vec::new();
    if (GetAsyncKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000) != 0 {
        mods.push("ctrl");
    }
    if (GetAsyncKeyState(VK_MENU.0 as i32) as u16 & 0x8000) != 0 {
        mods.push("alt");
    }
    if (GetAsyncKeyState(VK_SHIFT.0 as i32) as u16 & 0x8000) != 0 {
        mods.push("shift");
    }
    if (GetAsyncKeyState(VK_LWIN.0 as i32) as u16 & 0x8000) != 0
        || (GetAsyncKeyState(VK_RWIN.0 as i32) as u16 & 0x8000) != 0
    {
        mods.push("win");
    }

    let key_name = match vk {
        0x20 => Some("space".to_string()),
        0x09 => Some("tab".to_string()),
        0x0D => Some("enter".to_string()),
        0x08 => Some("backspace".to_string()),
        0x26 => Some("up".to_string()),
        0x28 => Some("down".to_string()),
        0x25 => Some("left".to_string()),
        0x27 => Some("right".to_string()),
        0x2D => Some("insert".to_string()),
        0x2E => Some("delete".to_string()),
        0x24 => Some("home".to_string()),
        0x23 => Some("end".to_string()),
        0x21 => Some("page_up".to_string()),
        0x22 => Some("page_down".to_string()),
        0x70..=0x7B => Some(format!("f{}", vk - 0x6F)),
        0x30..=0x39 | 0x41..=0x5A => Some((vk as u8 as char).to_ascii_lowercase().to_string()),
        _ => None,
    };

    let Some(k_str) = key_name else {
        return;
    };

    mods.push(&k_str);
    let combo = mods.join("+");

    let mut st = STATE.lock();
    let Some(target) = st.recording_target.take() else {
        return;
    };

    let other = if target == "hotkey_mixer" {
        st.config.hotkey_switch.clone()
    } else {
        st.config.hotkey_mixer.clone()
    };

    if combo.eq_ignore_ascii_case(&other) {
        let cfg = st.config.clone();
        drop(st);
        register_global_hotkeys(hwnd, &cfg);
        show_top_left_osd("Shortcut Conflict", "Already assigned to the other action!");
        let _ = InvalidateRect(hwnd, None, BOOL(0));
        return;
    }

    if target == "hotkey_mixer" {
        st.config.hotkey_mixer = combo.clone();
    } else {
        st.config.hotkey_switch = combo.clone();
    }
    save_config_to_disk(&st.config);
    let cfg = st.config.clone();
    drop(st);

    register_global_hotkeys(hwnd, &cfg);
    show_top_left_osd("Shortcut Saved", &combo.to_uppercase());
    let _ = InvalidateRect(hwnd, None, BOOL(0));
}

// =====================================================================
// Win32 Window Procedures
// =====================================================================
unsafe extern "system" fn flyout_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            paint_flyout_window(hwnd);
            LRESULT(0)
        }
        WM_HOTKEY => {
            let hk_id = wparam.0 as i32;
            if hk_id == HK_ID_MIXER {
                toggle_flyout();
            } else if hk_id == HK_ID_SWITCH {
                let excluded = STATE.lock().config.excluded_devices.clone();
                if let Ok((device_id, dev_name)) = audio::cycle_next_audio_device(&excluded) {
                    refresh_state_and_resize();
                    let label = displayed_switched_device(&device_id, &dev_name);
                    show_top_left_osd("Playback Device", &label);
                }
            }
            LRESULT(0)
        }
        WM_TRAYICON => {
            let ev = (lparam.0 & 0xFFFF) as u32;
            match ev {
                WM_LBUTTONUP => {
                    let just_closed = STATE
                        .lock()
                        .last_hide_at
                        .map(|t| t.elapsed() < Duration::from_millis(350))
                        .unwrap_or(false);
                    if !just_closed {
                        toggle_flyout();
                    }
                }
                WM_RBUTTONUP => {
                    show_custom_tray_menu();
                }
                WM_MBUTTONUP => {
                    let (cur_m, cur_v, last_nz) = {
                        let st = STATE.lock();
                        (
                            st.audio.master_muted,
                            st.audio.master_volume,
                            st.last_nonzero_master,
                        )
                    };
                    if cur_m || cur_v == 0 {
                        let restore = if cur_v == 0 { last_nz } else { cur_v };
                        let _ = audio::set_master_volume(restore);
                        let _ = audio::set_master_mute(false);
                        show_top_left_osd("Master Audio", &format!("Unmuted ({}%)", restore));
                    } else {
                        let _ = audio::set_master_mute(true);
                        show_top_left_osd("Master Audio", "Muted");
                    }
                    refresh_state_and_resize();
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_ACTIVATE => {
            let active = (wparam.0 & 0xFFFF) as u32;
            if active == 0 {
                let dragging = STATE.lock().dragging_slider.is_some();
                if !dragging {
                    hide_flyout();
                }
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == TIMER_LIVE_SYNC {
                let is_vis = STATE.lock().flyout_visible;
                let dragging = STATE.lock().dragging_slider.is_some();
                if is_vis {
                    if dragging {
                        refresh_drag_peaks();
                    } else {
                        refresh_state_and_resize();
                    }
                }
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let drag = {
                let mut st = STATE.lock();
                st.hover_pos = POINT { x, y };
                st.dragging_slider.clone()
            };
            if let Some((key, track_rc)) = drag {
                apply_slider_drag(&key, track_rc, x, false);
                // WM_TIMER may be deprioritized by a flood of WM_MOUSEMOVE.
                // A throttled peak refresh keeps the meter responsive then too.
                let is_due = STATE.lock().last_peak_refresh.elapsed()
                    >= Duration::from_millis(MIXER_REFRESH_MS as u64);
                if is_due { refresh_drag_peaks(); }
            }
            let _ = InvalidateRect(hwnd, None, BOOL(0));
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;

            let hit = {
                let st = STATE.lock();
                st.hitboxes
                    .iter()
                    .find(|hb| pt_in_rect(&hb.rect, x, y))
                    .map(|hb| hb.target.clone())
            };

            if let Some(target) = hit {
                match target {
                    HitTarget::MasterSlider(rc) => {
                        STATE.lock().dragging_slider = Some(("master".to_string(), rc));
                        SetCapture(hwnd);
                        apply_slider_drag("master", rc, x, true);
                        let _ = InvalidateRect(hwnd, None, BOOL(0));
                    }
                    HitTarget::AppSlider(k, rc) => {
                        STATE.lock().dragging_slider = Some((k.clone(), rc));
                        SetCapture(hwnd);
                        apply_slider_drag(&k, rc, x, true);
                        let _ = InvalidateRect(hwnd, None, BOOL(0));
                    }
                    HitTarget::DeviceDropdownToggle => {
                        {
                            let mut st = STATE.lock();
                            st.dropdown_expanded = !st.dropdown_expanded;
                            if st.dropdown_expanded {
                                st.settings_expanded = false;
                                st.scroll_offset = 0;
                            }
                        }
                        let _ = SetTimer(hwnd, TIMER_LIVE_SYNC, MIXER_REFRESH_MS, None);
                        refresh_state_and_resize();
                    }
                    HitTarget::QuickSwitchBtn => {
                        let excluded = STATE.lock().config.excluded_devices.clone();
                        if let Ok(_) = audio::cycle_next_audio_device(&excluded) {
                            refresh_state_and_resize();
                        }
                    }
                    HitTarget::SettingsToggleBtn => {
                        let (now_exp, cfg) = {
                            let mut st = STATE.lock();
                            st.settings_expanded = !st.settings_expanded;
                            st.scroll_offset = 0;
                            st.rename_device_id = None;
                            st.rename_buffer.clear();
                            if st.settings_expanded {
                                st.dropdown_expanded = false;
                            } else {
                                st.recording_target = None;
                            }
                            (st.settings_expanded, st.config.clone())
                        };
                        if !now_exp {
                            register_global_hotkeys(hwnd, &cfg);
                        }
                        set_flyout_opacity(hwnd, now_exp);
                        let refresh_ms = if now_exp {SETTINGS_REFRESH_MS} else {MIXER_REFRESH_MS};
                        let _ = SetTimer(hwnd, TIMER_LIVE_SYNC, refresh_ms, None);
                        refresh_state_and_resize();
                    }
                    HitTarget::SelectDevice(dev_id) => {
                        STATE.lock().dropdown_expanded = false;
                        let _ = audio::set_default_audio_device(&dev_id);
                        refresh_state_and_resize();
                    }
                    HitTarget::MasterMuteToggle => {
                        let (cur_m, cur_v, last_nz) = {
                            let st = STATE.lock();
                            (
                                st.audio.master_muted,
                                st.audio.master_volume,
                                st.last_nonzero_master,
                            )
                        };
                        if cur_m || cur_v == 0 {
                            let restore = if cur_v == 0 { last_nz } else { cur_v };
                            let _ = audio::set_master_volume(restore);
                            let _ = audio::set_master_mute(false);
                        } else {
                            let _ = audio::set_master_mute(true);
                        }
                        refresh_state_and_resize();
                    }
                    HitTarget::AppMuteToggle(k) => {
                        let (cur_m, cur_v, restore) = {
                            let st = STATE.lock();
                            let sess = st.audio.sessions.iter().find(|s| s.key == k);
                            let m = sess.map(|s| s.muted).unwrap_or(false);
                            let v = sess.map(|s| s.volume).unwrap_or(50);
                            let r = st.last_nonzero_apps.get(&k).copied().unwrap_or(50);
                            (m, v, r)
                        };
                        if cur_m || cur_v == 0 {
                            let target_v = if cur_v == 0 { restore } else { cur_v };
                            let _ = audio::set_session_volume(&k, target_v);
                            let _ = audio::set_session_mute(&k, false);
                        } else {
                            let _ = audio::set_session_mute(&k, true);
                        }
                        refresh_state_and_resize();
                    }
                    HitTarget::RecordHotkeyMixer => {
                        unregister_global_hotkeys(hwnd);
                        STATE.lock().recording_target = Some("hotkey_mixer".to_string());
                        let _ = InvalidateRect(hwnd, None, BOOL(0));
                    }
                    HitTarget::RecordHotkeySwitch => {
                        unregister_global_hotkeys(hwnd);
                        STATE.lock().recording_target = Some("hotkey_switch".to_string());
                        let _ = InvalidateRect(hwnd, None, BOOL(0));
                    }
                    HitTarget::ToggleSystemSounds => {
                        {
                            let mut st = STATE.lock();
                            st.config.show_system_sounds = !st.config.show_system_sounds;
                            save_config_to_disk(&st.config);
                        }
                        refresh_state_and_resize();
                    }
                    HitTarget::ToggleDeviceInclude(dev_id) => {
                        {
                            let mut st = STATE.lock();
                            if let Some(pos) =
                                st.config.excluded_devices.iter().position(|x| x == &dev_id)
                            {
                                st.config.excluded_devices.remove(pos);
                            } else {
                                st.config.excluded_devices.push(dev_id);
                            }
                            save_config_to_disk(&st.config);
                        }
                        refresh_state_and_resize();
                    }
                    HitTarget::CycleDeviceIcon(dev_id, cur_t) => {
                        let types = ["speaker", "headphones", "monitor"];
                        let idx = types.iter().position(|t| *t == cur_t).unwrap_or(0);
                        let next_t = types[(idx + 1) % types.len()].to_string();
                        {
                            let mut st = STATE.lock();
                            st.config.device_icons.insert(dev_id, next_t);
                            save_config_to_disk(&st.config);
                        }
                        refresh_state_and_resize();
                    }
                    HitTarget::RenameDevice(dev_id) => {
                        let mut st = STATE.lock();
                        st.rename_device_id = Some(dev_id.clone());
                        st.rename_buffer = st.config.device_aliases.get(&dev_id).cloned()
                            .unwrap_or_else(|| st.audio.devices.iter()
                                .find(|dev| dev.id == dev_id)
                                .map(|dev| default_short_name(&dev.name).to_string())
                                .unwrap_or_default());
                        drop(st);
                        let _ = InvalidateRect(hwnd, None, BOOL(0));
                    }
                    HitTarget::ToggleStartup => {
                        let enable = !STATE.lock().config.launch_at_startup;
                        if set_launch_at_startup(enable) {
                            let mut st = STATE.lock();
                            st.config.launch_at_startup = enable;
                            save_config_to_disk(&st.config);
                        } else {
                            show_top_left_osd("Startup Settings", "Could not change startup registration");
                        }
                        let _ = InvalidateRect(hwnd, None, BOOL(0));
                    }
                    HitTarget::OpenAboutDialog => {
                        show_about_window();
                    }
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let was_dragging = STATE.lock().dragging_slider.take().is_some();
            if was_dragging {
                flush_drag_pending();
                let _ = ReleaseCapture();
                let _ = InvalidateRect(hwnd, None, BOOL(0));
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) & 0xFFFF) as i16;
            let step: i32 = if delta > 0 { 4 } else if delta < 0 { -4 } else { 0 };

            let mut pt = POINT {
                x: (lparam.0 & 0xFFFF) as i16 as i32,
                y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
            };
            let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut pt);

            let slider_hit = {
                let st = STATE.lock();
                st.hitboxes
                    .iter()
                    .find(|hb| pt_in_rect(&hb.rect, pt.x, pt.y))
                    .map(|hb| hb.target.clone())
            };

            match slider_hit {
                Some(HitTarget::MasterSlider(_)) | Some(HitTarget::MasterMuteToggle) => {
                    let cur = STATE.lock().audio.master_volume as i32;
                    let nv = (cur + step).clamp(0, 100) as u32;
                    let _ = audio::set_master_volume(nv);
                    refresh_state_and_resize();
                }
                Some(HitTarget::AppSlider(k, _)) | Some(HitTarget::AppMuteToggle(k)) => {
                    let cur = STATE
                        .lock()
                        .audio
                        .sessions
                        .iter()
                        .find(|s| s.key == k)
                        .map(|s| s.volume as i32)
                        .unwrap_or(50);
                    let nv = (cur + step).clamp(0, 100) as u32;
                    let _ = audio::set_session_volume(&k, nv);
                    refresh_state_and_resize();
                }
                _ => {
                    let mut st = STATE.lock();
                    st.scroll_offset = (st.scroll_offset - step * 14).clamp(0, st.max_scroll);
                    drop(st);
                    let _ = InvalidateRect(hwnd, None, BOOL(0));
                }
            }
            LRESULT(0)
        }
        WM_CHAR => {
            let key = wparam.0 as u32;
            let mut st = STATE.lock();
            if st.rename_device_id.is_some() {
                if key == 8 {
                    st.rename_buffer.pop();
                } else if key >= 32 && key != 127 && st.rename_buffer.chars().count() < 42 {
                    if let Some(ch) = char::from_u32(key) {
                        if !ch.is_control() { st.rename_buffer.push(ch); }
                    }
                }
                drop(st);
                let _ = InvalidateRect(hwnd,None,BOOL(0));
                LRESULT(0)
            } else { DefWindowProcW(hwnd,msg,wparam,lparam) }
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let vk = wparam.0 as u32;
            let renaming = STATE.lock().rename_device_id.is_some();
            if renaming {
                if vk == 0x0D { // Enter: save local alias
                    commit_rename_device();
                    refresh_state_and_resize();
                } else if vk == 0x1B { // Esc: cancel without saving
                    let mut st = STATE.lock();
                    st.rename_device_id = None;
                    st.rename_buffer.clear();
                    drop(st);
                    let _ = InvalidateRect(hwnd,None,BOOL(0));
                } else if vk == 0x41 &&
                    (GetAsyncKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000) != 0 {
                    // Ctrl+A clears the buffer for quick replacement / reset.
                    STATE.lock().rename_buffer.clear();
                    let _ = InvalidateRect(hwnd,None,BOOL(0));
                }
                LRESULT(0)
            } else if STATE.lock().recording_target.is_some() {
                handle_hotkey_recording(hwnd, vk);
                LRESULT(0)
            } else if msg == WM_KEYDOWN {
                if vk == 0x1B {
                    hide_flyout();
                }
                LRESULT(0)
            } else {
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
        WM_SYSCHAR | WM_SYSKEYUP => {
            let is_rec = STATE.lock().recording_target.is_some();
            if is_rec {
                LRESULT(0)
            } else {
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
        WM_CLOSE => {
            hide_flyout();
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ if msg != 0 && msg == WM_TASKBAR_CREATED.load(Ordering::Relaxed) => {
            add_tray_icon(hwnd);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe extern "system" fn osd_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            paint_osd_window(hwnd);
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == TIMER_OSD_HIDE {
                let _ = KillTimer(hwnd, TIMER_OSD_HIDE);
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// Menu layout and hit testing share the same y coordinates, so a quick click
// works even if WM_MOUSEMOVE has not yet arrived.
fn tray_menu_hit_index(y: i32) -> i32 {
    if (92..136).contains(&y) { 0 }
    else if (140..184).contains(&y) { 1 }
    else if (188..232).contains(&y) { 2 }
    else if (236..280).contains(&y) { 3 }
    else if (298..342).contains(&y) { 4 }
    else { -1 }
}

unsafe extern "system" fn menu_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            paint_tray_menu(hwnd);
            LRESULT(0)
        }
        WM_ACTIVATE => {
            let active = (wparam.0 & 0xFFFF) as u32;
            if active == 0 {
                close_custom_tray_menu();
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let new_idx = tray_menu_hit_index(y);
            let mut st = STATE.lock();
            if st.menu_hover_index != new_idx {
                st.menu_hover_index = new_idx;
                drop(st);
                let _ = InvalidateRect(hwnd, None, BOOL(0));
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let idx = tray_menu_hit_index(y);
            close_custom_tray_menu();
            match idx {
                0 => open_flyout(false),
                1 => {
                    let excluded = STATE.lock().config.excluded_devices.clone();
                    if let Ok((device_id, dev_name)) = audio::cycle_next_audio_device(&excluded) {
                        refresh_state_and_resize();
                        let label = displayed_switched_device(&device_id, &dev_name);
                        show_top_left_osd("Playback Device", &label);
                    }
                }
                2 => open_flyout(true),
                3 => show_about_window(),
                4 => {
                    PostQuitMessage(0);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            if wparam.0 as u32 == 0x1B {
                close_custom_tray_menu();
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe extern "system" fn about_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            paint_about_window(hwnd);
            LRESULT(0)
        }
        WM_ACTIVATE => {
            let active = (wparam.0 & 0xFFFF) as u32;
            if active == 0 {
                close_about_window();
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            close_about_window();
            LRESULT(0)
        }
        WM_KEYDOWN => {
            if wparam.0 as u32 == 0x1B {
                close_about_window();
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// =====================================================================
// Application Entry Point
// =====================================================================
fn main() {
    unsafe {
        // 1. Enable Per-Monitor V2 High-DPI Awareness so fonts & shapes render razor-sharp on Windows 11
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_err() {
            let _ = SetProcessDPIAware();
        }

        // 2. Single-instance mutex check
        let _mutex = CreateMutexW(None, false, w!("Local\\MixSwitch_SingleInstance_Mutex_v1"));
        if GetLastError() == Err(ERROR_ALREADY_EXISTS.into()) {
            MessageBoxW(
                None,
                w!("MixSwitch is already running (check the system tray)."),
                w!("MixSwitch"),
                MB_ICONINFORMATION,
            );
            return;
        }

        audio::ensure_com_initialized();
        WM_TASKBAR_CREATED.store(
            RegisterWindowMessageW(w!("TaskbarCreated")),
            Ordering::Relaxed,
        );
        let hinstance = GetModuleHandleW(None).unwrap_or_default();

        // Register Window Classes
        let flyout_class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(flyout_wnd_proc),
            hInstance: hinstance.into(),
            hCursor: LoadCursorW(None, IDC_HAND).unwrap_or_default(),
            lpszClassName: w!("MixSwitchFlyoutClass"),
            ..Default::default()
        };
        RegisterClassW(&flyout_class);

        let osd_class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(osd_wnd_proc),
            hInstance: hinstance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: w!("MixSwitchOsdClass"),
            ..Default::default()
        };
        RegisterClassW(&osd_class);

        let menu_class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(menu_wnd_proc),
            hInstance: hinstance.into(),
            hCursor: LoadCursorW(None, IDC_HAND).unwrap_or_default(),
            lpszClassName: w!("MixSwitchMenuClass"),
            ..Default::default()
        };
        RegisterClassW(&menu_class);

        let about_class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(about_wnd_proc),
            hInstance: hinstance.into(),
            hCursor: LoadCursorW(None, IDC_HAND).unwrap_or_default(),
            lpszClassName: w!("MixSwitchAboutClass"),
            ..Default::default()
        };
        RegisterClassW(&about_class);

        // Create Main Flyout Window
        let hwnd_flyout = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
            w!("MixSwitchFlyoutClass"),
            w!("MixSwitch"),
            WS_POPUP,
            0,
            0,
            COMPACT_FLYOUT_WIDTH,
            280,
            None,
            None,
            hinstance,
            None,
        );
        set_flyout_opacity(hwnd_flyout, false);
        apply_fluent_backdrop(hwnd_flyout);

        // Create Independent Top-Left OSD Window
        let hwnd_osd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED,
            w!("MixSwitchOsdClass"),
            w!("MixSwitchOSD"),
            WS_POPUP,
            22,
            22,
            OSD_WIDTH,
            OSD_HEIGHT,
            None,
            None,
            hinstance,
            None,
        );
        let _ = SetLayeredWindowAttributes(hwnd_osd, COLORREF(0), 193, LWA_ALPHA);
        apply_fluent_backdrop(hwnd_osd);

        // Create Custom Dark Right-Click Tray Menu Window
        let hwnd_menu = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
            w!("MixSwitchMenuClass"),
            w!("MixSwitchMenu"),
            WS_POPUP,
            0,
            0,
            390,
            368,
            None,
            None,
            hinstance,
            None,
        );
        let _ = SetLayeredWindowAttributes(hwnd_menu, COLORREF(0), 249, LWA_ALPHA);
        apply_fluent_backdrop(hwnd_menu);

        // Create Dedicated About Dialog Window
        let hwnd_about = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
            w!("MixSwitchAboutClass"),
            w!("About MixSwitch"),
            WS_POPUP,
            0,
            0,
            420,
            340,
            None,
            None,
            hinstance,
            None,
        );
        let _ = SetLayeredWindowAttributes(hwnd_about, COLORREF(0), 249, LWA_ALPHA);
        apply_fluent_backdrop(hwnd_about);

        let cfg = {
            let mut st = STATE.lock();
            st.hwnd_flyout = hwnd_flyout;
            st.hwnd_osd = hwnd_osd;
            st.hwnd_menu = hwnd_menu;
            st.hwnd_about = hwnd_about;
            st.config.clone()
        };

        register_global_hotkeys(hwnd_flyout, &cfg);

        // Create System Tray Icon using MixSwitch green-cyan-blue brand icon
        let tray_hicon = create_mixswitch_tray_hicon();
        TRAY_HICON.store(tray_hicon.0, Ordering::Relaxed);
        add_tray_icon(hwnd_flyout);
        offer_startup_on_first_run();

        // Native Zero-CPU Win32 Message Loop
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        unregister_global_hotkeys(hwnd_flyout);
        remove_tray_icon(hwnd_flyout);
        if !tray_hicon.is_invalid() {
            let _ = DestroyIcon(tray_hicon);
        }
    }
}
