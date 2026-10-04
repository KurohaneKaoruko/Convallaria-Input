//! 自注册：写 InprocServer32，并经 `ITfInputProcessorProfiles` / `ITfCategoryMgr`
//! 把输入法登记为键盘类文本服务。
//!
//! 类别声明必须齐全：除「键盘 TIP」外还要声明沉浸式 / 系统托盘等能力，
//! 否则 Win10/11 的输入切换器会把它过滤掉（表现为装上之后找不到、无法使用）。
//! 注册到 HKEY_CLASSES_ROOT，因此需要管理员权限。

use windows::Win32::System::Registry::{HKEY, HKEY_CLASSES_ROOT};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::System::LibraryLoader::{
    GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
    GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
};
use windows::Win32::System::Registry::*;
use windows::Win32::UI::TextServices::{
    CLSID_TF_CategoryMgr, CLSID_TF_InputProcessorProfiles, GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER,
    GUID_TFCAT_TIPCAP_COMLESS, GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT,
    GUID_TFCAT_TIPCAP_INPUTMODECOMPARTMENT, GUID_TFCAT_TIPCAP_SECUREMODE,
    GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT, GUID_TFCAT_TIPCAP_UIELEMENTENABLED,
    GUID_TFCAT_TIP_KEYBOARD, ITfCategoryMgr, ITfInputProcessorProfiles,
};
use windows_core::{GUID, PCWSTR};

use crate::{LANGID_ZH_CN, PROFILE_GUID, TIP_CLSID, TIP_CLSID_STR};

/// 除「键盘类 TIP」外还需声明的运行环境能力；缺了会被现代输入切换器过滤。
const CATEGORIES: &[GUID] = &[
    GUID_TFCAT_TIP_KEYBOARD,
    GUID_TFCAT_TIPCAP_UIELEMENTENABLED,
    GUID_TFCAT_TIPCAP_SECUREMODE,
    GUID_TFCAT_TIPCAP_COMLESS,
    GUID_TFCAT_TIPCAP_INPUTMODECOMPARTMENT,
    GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT,
    GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT,
    GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER,
];

/// 本 DLL 在磁盘上的完整路径（InprocServer32 用）。
fn module_path() -> Option<String> {
    // 以本模块内的函数地址反查模块句柄，避免拿到宿主 exe 的路径
    let marker = register as *const ();
    let mut module = Default::default();
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            PCWSTR(marker as *const u16),
            &mut module,
        )
        .ok()?;
    }
    let mut buf = [0u16; 512];
    let len = unsafe { GetModuleFileNameW(Some(module), &mut buf) } as usize;
    if len == 0 || len >= buf.len() {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..len]))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn wide_bytes(s: &str) -> Vec<u8> {
    let mut v: Vec<u8> = s.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
    v.extend_from_slice(&[0, 0]);
    v
}

/// 写 InprocServer32（HKEY_CLASSES_ROOT）。
fn write_inproc_server(dll_path: &str) -> bool {
    unsafe {
        let mut hkey = HKEY::default();
        if RegCreateKeyExW(
            HKEY_CLASSES_ROOT,
            PCWSTR(wide(&format!("CLSID\\{TIP_CLSID_STR}")).as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut hkey,
            None,
        )
        .is_err()
        {
            return false;
        }
        let _ = RegSetValueExW(
            hkey,
            PCWSTR::null(),
            None,
            REG_SZ,
            Some(&wide_bytes(dll_path)),
        );
        let _ = RegSetValueExW(
            hkey,
            PCWSTR(wide("ThreadingModel").as_ptr()),
            None,
            REG_SZ,
            Some(&wide_bytes("Apartment")),
        );
        let _ = RegCloseKey(hkey);
        true
    }
}

/// 完整注册（由 DllRegisterServer 在提权环境里调用）。返回是否成功。
pub fn register() -> bool {
    let Some(dll_path) = module_path() else {
        return false;
    };
    if !write_inproc_server(&dll_path) {
        return false;
    }

    // COM 作用域：regsvr32 宿主通常已初始化 COM；这里按需初始化并配对释放
    let com_initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
    let ok = register_profile();
    if com_initialized {
        unsafe { windows::Win32::System::Com::CoUninitialize() };
    }
    ok
}

fn register_profile() -> bool {
    unsafe {
        let Ok(profiles) = CoCreateInstance::<_, ITfInputProcessorProfiles>(
            &CLSID_TF_InputProcessorProfiles,
            None,
            CLSCTX_INPROC_SERVER,
        ) else {
            return false;
        };
        if profiles.Register(&TIP_CLSID).is_err() {
            return false;
        }
        // 描述串按 null 扫描，必须以 0 结尾
        if profiles
            .AddLanguageProfile(
                &TIP_CLSID,
                LANGID_ZH_CN,
                &PROFILE_GUID,
                &wide("Convallaria Input"),
                &wide(""),
                0,
            )
            .is_err()
        {
            return false;
        }
        let Ok(category) = CoCreateInstance::<_, ITfCategoryMgr>(
            &CLSID_TF_CategoryMgr,
            None,
            CLSCTX_INPROC_SERVER,
        ) else {
            return false;
        };
        for catid in CATEGORIES {
            if category.RegisterCategory(&TIP_CLSID, catid, &TIP_CLSID).is_err() {
                return false;
            }
        }
        true
    }
}

/// 撤销注册（尽力而为，由 DllUnregisterServer 调用）。
pub fn unregister() {
    let com_initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
    unsafe {
        if let Ok(category) = CoCreateInstance::<_, ITfCategoryMgr>(
            &CLSID_TF_CategoryMgr,
            None,
            CLSCTX_INPROC_SERVER,
        ) {
            for catid in CATEGORIES {
                let _ = category.UnregisterCategory(&TIP_CLSID, catid, &TIP_CLSID);
            }
        }
        if let Ok(profiles) = CoCreateInstance::<_, ITfInputProcessorProfiles>(
            &CLSID_TF_InputProcessorProfiles,
            None,
            CLSCTX_INPROC_SERVER,
        ) {
            let _ = profiles.RemoveLanguageProfile(&TIP_CLSID, LANGID_ZH_CN, &PROFILE_GUID);
            let _ = profiles.Unregister(&TIP_CLSID);
        }
        let _ = windows::Win32::System::Registry::RegDeleteTreeW(
            HKEY_CLASSES_ROOT,
            PCWSTR(wide(&format!("CLSID\\{TIP_CLSID_STR}")).as_ptr()),
        );
    }
    if com_initialized {
        unsafe { windows::Win32::System::Com::CoUninitialize() };
    }
}
