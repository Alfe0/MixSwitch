#![allow(non_snake_case)]

use std::collections::HashMap;
use std::time::{Duration, Instant};
use std::path::Path;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use windows::core::{interface, ComInterface, GUID, HRESULT, IUnknown, IUnknown_Vtbl, PCWSTR, PWSTR};
use windows::Win32::Foundation::{BOOL, CloseHandle};
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioMeterInformation};
use windows::Win32::Media::Audio::{
    eCommunications, eConsole, eMultimedia, eRender, IAudioSessionControl2, IAudioSessionEnumerator,
    IAudioSessionManager2, IMMDevice, IMMDeviceCollection, IMMDeviceEnumerator, ISimpleAudioVolume,
    MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_APARTMENTTHREADED, STGM_READ,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
    TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Shell::ExtractIconExW;
use windows::Win32::UI::Shell::PropertiesSystem::PROPERTYKEY;
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, HICON};

// Cache extracted HICON handles per executable path so icons are extracted only once
static HICON_CACHE: Lazy<Mutex<HashMap<String, isize>>> = Lazy::new(|| Mutex::new(HashMap::new()));
// A bounded, short-lived identity cache: parent-process checks are only performed
// for WebView2 sessions and are never part of the audio device switching path.
static WEBVIEW_OWNER_CACHE: Lazy<Mutex<HashMap<u32, (Instant, Option<String>)>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));


const PKEY_DEVICE_FRIENDLY_NAME: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 14,
};

const CLSID_POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

#[interface("f8679f50-850a-41cf-9c72-430f290290c8")]
unsafe trait IPolicyConfig: IUnknown {
    fn GetMixFormat(&self, dev_name: PCWSTR, format: *mut *mut core::ffi::c_void) -> HRESULT;
    fn GetDeviceFormat(&self, dev_name: PCWSTR, default: i32, format: *mut *mut core::ffi::c_void) -> HRESULT;
    fn ResetDeviceFormat(&self, dev_name: PCWSTR) -> HRESULT;
    fn SetDeviceFormat(&self, dev_name: PCWSTR, endpoint_format: *const core::ffi::c_void, mix_format: *const core::ffi::c_void) -> HRESULT;
    fn GetProcessingPeriod(&self, dev_name: PCWSTR, default: i32, default_period: *mut i64, min_period: *mut i64) -> HRESULT;
    fn SetProcessingPeriod(&self, dev_name: PCWSTR, period: *const i64) -> HRESULT;
    fn GetShareMode(&self, dev_name: PCWSTR, mode: *mut core::ffi::c_void) -> HRESULT;
    fn SetShareMode(&self, dev_name: PCWSTR, mode: *const core::ffi::c_void) -> HRESULT;
    fn GetPropertyValue(&self, dev_name: PCWSTR, fx_store: i32, key: *const core::ffi::c_void, pv: *mut core::ffi::c_void) -> HRESULT;
    fn SetPropertyValue(&self, dev_name: PCWSTR, fx_store: i32, key: *const core::ffi::c_void, pv: *const core::ffi::c_void) -> HRESULT;
    fn SetDefaultEndpoint(&self, dev_name: PCWSTR, role: u32) -> HRESULT;
    fn SetEndpointVisibility(&self, dev_name: PCWSTR, visible: i32) -> HRESULT;
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub icon_type: String,
    pub excluded: bool,
}

#[derive(Clone, Debug)]
pub struct AppSessionInfo {
    pub key: String,
    pub name: String,
    pub exe_path: String,
    pub volume: u32,
    pub muted: bool,
    pub peak: u32,
    pub hicon: isize,
    pub is_system: bool,
}

#[derive(Clone, Debug)]
pub struct FullAudioState {
    pub current_device_id: String,
    pub current_device_name: String,
    pub current_device_icon: String,
    pub master_volume: u32,
    pub master_muted: bool,
    pub master_peak: u32,
    pub devices: Vec<AudioDeviceInfo>,
    pub sessions: Vec<AppSessionInfo>,
}

pub fn ensure_com_initialized() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
}

pub fn infer_device_icon(dev_id: &str, dev_name: &str, custom_map: &HashMap<String, String>) -> String {
    if let Some(saved) = custom_map.get(dev_id) {
        if ["speaker", "headphones", "monitor"].contains(&saved.as_str()) {
            return saved.clone();
        }
    }
    let lower = dev_name.to_lowercase();
    for kw in ["m60", "edifier", "speaker", "speakers", "realtek", "soundbar"] {
        if lower.contains(kw) {
            return "speaker".to_string();
        }
    }
    for kw in [
        "nvidia", "amd", "lg", "samsung", "dell", "asus", "benq", "acer", "msi", "monitor", "hdmi",
        "displayport", "tv",
    ] {
        if lower.contains(kw) {
            return "monitor".to_string();
        }
    }
    "headphones".to_string()
}

unsafe fn get_device_id_string(device: &IMMDevice) -> Option<String> {
    let raw_id: PWSTR = device.GetId().ok()?;
    let id_str = raw_id.to_string().ok();
    CoTaskMemFree(Some(raw_id.0 as *const _));
    id_str
}

unsafe fn get_device_friendly_name(device: &IMMDevice) -> String {
    if let Ok(store) = device.OpenPropertyStore(STGM_READ) {
        if let Ok(prop) = store.GetValue(&PKEY_DEVICE_FRIENDLY_NAME) {
            if let Ok(pwstr) = PropVariantToStringAlloc(&prop) {
                let s = pwstr.to_string().unwrap_or_else(|_| "Audio Output".to_string());
                CoTaskMemFree(Some(pwstr.0 as *const _));
                if !s.trim().is_empty() {
                    return s;
                }
            }
        }
    }
    "Audio Output".to_string()
}

pub fn get_cached_hicon(exe_path: &str) -> isize {
    if exe_path.is_empty() || !Path::new(exe_path).exists() {
        return 0;
    }
    {
        let cache = HICON_CACHE.lock();
        if let Some(&h) = cache.get(exe_path) {
            return h;
        }
    }
    unsafe {
        let wide_path: Vec<u16> = exe_path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut large_icon: HICON = HICON::default();
        let mut small_icon: HICON = HICON::default();

        let count = ExtractIconExW(
            PCWSTR(wide_path.as_ptr()),
            0,
            Some(&mut large_icon),
            Some(&mut small_icon),
            1,
        );

        let chosen = if count > 0 && !large_icon.is_invalid() {
            if !small_icon.is_invalid() {
                let _ = DestroyIcon(small_icon);
            }
            large_icon.0
        } else if count > 0 && !small_icon.is_invalid() {
            small_icon.0
        } else {
            0
        };

        HICON_CACHE.lock().insert(exe_path.to_string(), chosen);
        chosen
    }
}

/// Fast image lookup used by both native sessions and verified WebView2 owners.
fn process_image_path(pid: u32) -> Option<String> {
    if pid == 0 { return None; }
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, BOOL(0), pid).ok()?;
        let mut buf = [0u16; 2048];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len,
        ).is_ok();
        let _ = CloseHandle(handle);
        if ok && len > 0 {
            Some(String::from_utf16_lossy(&buf[..len as usize]))
        } else { None }
    }
}

/// A WebView2 renderer has a generic binary name. Only label it WhatsApp if
/// its actual ancestor process belongs to WhatsApp; other WebView2 apps retain
/// their own identity. No global window-title guessing or process relabeling.
fn whatsapp_owner_for_webview(pid: u32) -> Option<String> {
    {
        let cache = WEBVIEW_OWNER_CACHE.lock();
        if let Some((checked_at, path)) = cache.get(&pid) {
            if checked_at.elapsed() < Duration::from_secs(30) {
                return path.clone();
            }
        }
    }

    let found = unsafe {
        let snapshot = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            Ok(s) => s,
            Err(_) => return None,
        };
        let mut parents = HashMap::new();
        let mut entry = PROCESSENTRY32W::default();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                parents.insert(entry.th32ProcessID, entry.th32ParentProcessID);
                if Process32NextW(snapshot, &mut entry).is_err() { break; }
            }
        }
        let _ = CloseHandle(snapshot);

        let mut current = pid;
        let mut owner = None;
        // WebView2 renderers may have intermediate WebView2 parent processes.
        for _ in 0..8 {
            let Some(&parent) = parents.get(&current) else { break };
            if parent == 0 || parent == current { break; }
            if let Some(path) = process_image_path(parent) {
                let name = Path::new(&path).file_stem()
                    .map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
                if name.contains("whatsapp") {
                    owner = Some(path);
                    break;
                }
            }
            current = parent;
        }
        owner
    };

    let mut cache = WEBVIEW_OWNER_CACHE.lock();
    if cache.len() > 256 {
        cache.retain(|_, (t, _)| t.elapsed() < Duration::from_secs(30));
        if cache.len() > 256 { cache.clear(); }
    }
    cache.insert(pid, (Instant::now(), found.clone()));
    found
}

/// Resolve audio session owner.  Grouping keys are also used by mute/volume
/// functions, so the same resolver must be used for both display and control.
fn resolve_process_fast(pid: u32) -> Option<(String, String, String)> {
    if pid == 0 {
        return Some((
            "system_sounds".to_string(),
            "System Sounds".to_string(),
            String::new(),
        ));
    }

    let exe_path = process_image_path(pid)?;
    let stem = Path::new(&exe_path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "App".to_string());
    let lower_name = stem.to_lowercase();
    let lower_exe = exe_path.to_lowercase();

    if lower_name == "msedgewebview2" {
        if let Some(owner_path) = whatsapp_owner_for_webview(pid) {
            return Some(("whatsapp_group".to_string(), "WhatsApp".to_string(), owner_path));
        }
        // Never claim an unrelated WebView2 process belongs to WhatsApp.
        return Some((format!("webview2_pid_{}", pid), "WebView2".to_string(), exe_path));
    }

    let group_key = if lower_name.contains("steam") {
        "steam_group".to_string()
    } else if lower_name.contains("whatsapp") || lower_exe.contains("whatsapp") {
        "whatsapp_group".to_string()
    } else {
        lower_exe
    };
    let final_title = if group_key == "steam_group" {
        "Steam".to_string()
    } else if group_key == "whatsapp_group" {
        "WhatsApp".to_string()
    } else {
        let mut chars = stem.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            None => "Application".to_string(),
        }
    };
    Some((group_key, final_title, exe_path))
}

pub fn get_full_audio_state(
    show_system_sounds: bool,
    device_icons: &HashMap<String, String>,
    excluded_devices: &[String],
) -> Result<FullAudioState, String> {
    ensure_com_initialized();

    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| e.to_string())?;

        let default_dev: IMMDevice = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(|e| e.to_string())?;

        let current_device_id = get_device_id_string(&default_dev).unwrap_or_default();
        let current_device_name = get_device_friendly_name(&default_dev);
        let current_device_icon =
            infer_device_icon(&current_device_id, &current_device_name, device_icons);

        let endpoint_vol: IAudioEndpointVolume = default_dev
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| e.to_string())?;

        let master_scalar = endpoint_vol.GetMasterVolumeLevelScalar().unwrap_or(1.0);
        let master_volume = (master_scalar * 100.0).round().clamp(0.0, 100.0) as u32;
        let master_muted = endpoint_vol.GetMute().unwrap_or(BOOL(0)).as_bool();

        let master_peak =
            if let Ok(meter) = default_dev.Activate::<IAudioMeterInformation>(CLSCTX_ALL, None) {
                let p = meter.GetPeakValue().unwrap_or(0.0);
                (p * 100.0).round().clamp(0.0, 100.0) as u32
            } else {
                0
            };

        let mut devices_list = Vec::new();
        if let Ok(collection) = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) {
            let count = collection.GetCount().unwrap_or(0);
            for i in 0..count {
                if let Ok(dev) = collection.Item(i) {
                    if let Some(d_id) = get_device_id_string(&dev) {
                        let d_name = get_device_friendly_name(&dev);
                        let ic_type = infer_device_icon(&d_id, &d_name, device_icons);
                        let is_def = d_id == current_device_id;
                        let is_excl = excluded_devices.contains(&d_id);
                        devices_list.push(AudioDeviceInfo {
                            id: d_id,
                            name: d_name,
                            is_default: is_def,
                            icon_type: ic_type,
                            excluded: is_excl,
                        });
                    }
                }
            }
        }

        let mut grouped_sessions: Vec<AppSessionInfo> = Vec::new();
        let mut key_to_index: HashMap<String, usize> = HashMap::new();

        if let Ok(manager2) = default_dev.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) {
            if let Ok(session_enum) = manager2.GetSessionEnumerator() {
                let s_count = session_enum.GetCount().unwrap_or(0);
                for i in 0..s_count {
                    if let Ok(ctrl) = session_enum.GetSession(i) {
                        let ctrl2: Result<IAudioSessionControl2, _> = ctrl.cast();
                        let simple_vol: Result<ISimpleAudioVolume, _> = ctrl.cast();

                        if let (Ok(c2), Ok(sv)) = (ctrl2, simple_vol) {
                            let is_sys_session = c2.IsSystemSoundsSession() == HRESULT(0);
                            let pid = if is_sys_session {
                                0
                            } else {
                                c2.GetProcessId().unwrap_or(0)
                            };

                            if pid == 0 && !show_system_sounds {
                                continue;
                            }

                            let Some((group_key, app_name, exe_path)) = resolve_process_fast(pid)
                            else {
                                continue;
                            };

                            let vol_f = sv.GetMasterVolume().unwrap_or(1.0);
                            let vol_u = (vol_f * 100.0).round().clamp(0.0, 100.0) as u32;
                            let is_muted = sv.GetMute().unwrap_or(BOOL(0)).as_bool();

                            let peak_u = if let Ok(meter) = ctrl.cast::<IAudioMeterInformation>() {
                                (meter.GetPeakValue().unwrap_or(0.0) * 100.0)
                                    .round()
                                    .clamp(0.0, 100.0) as u32
                            } else {
                                0
                            };

                            if let Some(&idx) = key_to_index.get(&group_key) {
                                if peak_u > grouped_sessions[idx].peak {
                                    grouped_sessions[idx].peak = peak_u;
                                }
                                if grouped_sessions[idx].hicon == 0 && !exe_path.is_empty() {
                                    grouped_sessions[idx].hicon = get_cached_hicon(&exe_path);
                                }
                            } else {
                                let hicon = if pid == 0 {
                                    0
                                } else {
                                    get_cached_hicon(&exe_path)
                                };

                                let info = AppSessionInfo {
                                    key: group_key.clone(),
                                    name: app_name,
                                    exe_path,
                                    volume: vol_u,
                                    muted: is_muted,
                                    peak: peak_u,
                                    hicon,
                                    is_system: pid == 0,
                                };
                                key_to_index.insert(group_key, grouped_sessions.len());
                                grouped_sessions.push(info);
                            }
                        }
                    }
                }
            }
        }

        grouped_sessions.sort_by(|a, b| b.is_system.cmp(&a.is_system).then(a.name.cmp(&b.name)));

        Ok(FullAudioState {
            current_device_id,
            current_device_name,
            current_device_icon,
            master_volume,
            master_muted,
            master_peak,
            devices: devices_list,
            sessions: grouped_sessions,
        })
    }
}

pub fn set_master_volume(volume: u32) -> Result<(), String> {
    ensure_com_initialized();
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| e.to_string())?;
        let default_dev: IMMDevice = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(|e| e.to_string())?;
        let endpoint_vol: IAudioEndpointVolume = default_dev
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| e.to_string())?;

        let clamped = volume.min(100) as f32 / 100.0;
        endpoint_vol
            .SetMasterVolumeLevelScalar(clamped, std::ptr::null())
            .map_err(|e| e.to_string())?;

        if volume == 0 {
            let _ = endpoint_vol.SetMute(BOOL(1), std::ptr::null());
        } else if endpoint_vol.GetMute().unwrap_or(BOOL(0)).as_bool() {
            let _ = endpoint_vol.SetMute(BOOL(0), std::ptr::null());
        }
        Ok(())
    }
}

pub fn set_master_mute(mute: bool) -> Result<(), String> {
    ensure_com_initialized();
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| e.to_string())?;
        let default_dev: IMMDevice = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(|e| e.to_string())?;
        let endpoint_vol: IAudioEndpointVolume = default_dev
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| e.to_string())?;

        endpoint_vol
            .SetMute(BOOL(if mute { 1 } else { 0 }), std::ptr::null())
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

pub fn set_session_volume(target_key: &str, volume: u32) -> Result<(), String> {
    ensure_com_initialized();
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| e.to_string())?;
        let default_dev: IMMDevice = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(|e| e.to_string())?;
        let manager2: IAudioSessionManager2 = default_dev
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| e.to_string())?;
        let session_enum: IAudioSessionEnumerator =
            manager2.GetSessionEnumerator().map_err(|e| e.to_string())?;

        let count = session_enum.GetCount().unwrap_or(0);
        let scalar = volume.min(100) as f32 / 100.0;

        for i in 0..count {
            if let Ok(ctrl) = session_enum.GetSession(i) {
                if let (Ok(c2), Ok(sv)) = (
                    ctrl.cast::<IAudioSessionControl2>(),
                    ctrl.cast::<ISimpleAudioVolume>(),
                ) {
                    let is_sys = c2.IsSystemSoundsSession() == HRESULT(0);
                    let pid = if is_sys { 0 } else { c2.GetProcessId().unwrap_or(0) };
                    if let Some((group_key, _, _)) = resolve_process_fast(pid) {
                        if group_key == target_key {
                            let _ = sv.SetMasterVolume(scalar, std::ptr::null());
                            if volume == 0 {
                                let _ = sv.SetMute(BOOL(1), std::ptr::null());
                            } else if sv.GetMute().unwrap_or(BOOL(0)).as_bool() {
                                let _ = sv.SetMute(BOOL(0), std::ptr::null());
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

pub fn set_session_mute(target_key: &str, mute: bool) -> Result<(), String> {
    ensure_com_initialized();
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| e.to_string())?;
        let default_dev: IMMDevice = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(|e| e.to_string())?;
        let manager2: IAudioSessionManager2 = default_dev
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| e.to_string())?;
        let session_enum: IAudioSessionEnumerator =
            manager2.GetSessionEnumerator().map_err(|e| e.to_string())?;

        let count = session_enum.GetCount().unwrap_or(0);
        for i in 0..count {
            if let Ok(ctrl) = session_enum.GetSession(i) {
                if let (Ok(c2), Ok(sv)) = (
                    ctrl.cast::<IAudioSessionControl2>(),
                    ctrl.cast::<ISimpleAudioVolume>(),
                ) {
                    let is_sys = c2.IsSystemSoundsSession() == HRESULT(0);
                    let pid = if is_sys { 0 } else { c2.GetProcessId().unwrap_or(0) };
                    if let Some((group_key, _, _)) = resolve_process_fast(pid) {
                        if group_key == target_key {
                            let _ = sv.SetMute(BOOL(if mute { 1 } else { 0 }), std::ptr::null());
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

pub fn set_default_audio_device(device_id: &str) -> Result<String, String> {
    ensure_com_initialized();
    unsafe {
        let policy_config: IPolicyConfig =
            CoCreateInstance(&CLSID_POLICY_CONFIG_CLIENT, None, CLSCTX_ALL)
                .map_err(|e| format!("IPolicyConfig failed: {}", e))?;

        let wide_id: Vec<u16> = device_id.encode_utf16().chain(std::iter::once(0)).collect();
        for role in [eConsole.0 as u32, eMultimedia.0 as u32, eCommunications.0 as u32] {
            let hr = policy_config.SetDefaultEndpoint(PCWSTR(wide_id.as_ptr()), role);
            if hr.is_err() {
                return Err(format!("SetDefaultEndpoint failed with HRESULT {:?}", hr));
            }
        }

        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| e.to_string())?;
        if let Ok(default_dev) = enumerator.GetDefaultAudioEndpoint(eRender, eConsole) {
            return Ok(get_device_friendly_name(&default_dev));
        }

        Ok("Default Audio Output".to_string())
    }
}

pub fn cycle_next_audio_device(excluded_devices: &[String]) -> Result<(String, String), String> {
    ensure_com_initialized();
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| e.to_string())?;

        let current_id = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .ok()
            .and_then(|d| get_device_id_string(&d))
            .unwrap_or_default();

        let collection: IMMDeviceCollection = enumerator
            .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
            .map_err(|e| e.to_string())?;

        let count = collection.GetCount().unwrap_or(0);
        let mut all_devices: Vec<(String, String)> = Vec::new();

        for i in 0..count {
            if let Ok(dev) = collection.Item(i) {
                if let Some(id) = get_device_id_string(&dev) {
                    let name = get_device_friendly_name(&dev);
                    all_devices.push((id, name));
                }
            }
        }

        if all_devices.len() < 2 {
            return Err("Requires at least 2 active audio devices to switch.".to_string());
        }

        let mut active_pool: Vec<(String, String)> = all_devices
            .iter()
            .filter(|(id, _)| !excluded_devices.contains(id))
            .cloned()
            .collect();

        if active_pool.len() < 2 {
            active_pool = all_devices;
        }

        let next_idx = match active_pool.iter().position(|(id, _)| id == &current_id) {
            Some(pos) => (pos + 1) % active_pool.len(),
            None => 0,
        };
        let (next_id, next_name) = active_pool[next_idx].clone();

        set_default_audio_device(&next_id)?;
        Ok((next_id, next_name))
    }
}
