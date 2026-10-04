//! Convallaria Input 安装包（单文件 setup.exe，GUI 弹窗交互）。
//!
//! 用法：
//! - 双击运行（或命令行直接运行）：安装；非管理员时自动弹 UAC 重启自身
//! - `convallaria-setup.exe --uninstall`：卸载
//!
//! 自包含：DLL 与词典在编译期嵌入（见 `include_bytes!`），
//! 产物是单个 exe，可直接分发。

#![cfg(windows)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, HANDLE, WIN32_ERROR};
use windows::Win32::Security::{
    GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
};
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::ShellExecuteExW;
use windows::Win32::UI::TextServices::{
    ITfInputProcessorProfiles, CLSID_TF_InputProcessorProfiles,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::SHELLEXECUTEINFOW;
use windows::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, SW_SHOWNORMAL, MB_ICONERROR, MB_ICONINFORMATION, MB_ICONQUESTION, MB_OK,
    MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO, MESSAGEBOX_RESULT, MESSAGEBOX_STYLE, IDYES,
};
use windows_core::{w, GUID, PCWSTR};

/// 嵌入的输入法 DLL（由 build-installer.ps1 构建到 target\pack，避免与已安装的 DLL 抢锁）。
const IME_DLL: &[u8] = include_bytes!("../../../target/pack/release/convallaria_windows.dll");
/// 嵌入的词典。
const DICTIONARY: &[u8] = include_bytes!("../../../assets/dicts/convallaria.dict.bin");

const TIP_CLSID: &str = "{8A5C7B60-4C2A-4E1F-9D3B-5C0A11B2C001}";
const PROFILE_GUID: &str = "{8A5C7B60-4C2A-4E1F-9D3B-5C0A11B2C002}";
const LANGID_ZH_CN: u16 = 0x0804;
const VERSION: &str = env!("CARGO_PKG_VERSION");

const TITLE: PCWSTR = w!("Convallaria Input（铃兰输入法）");

fn msg_box(text: &str, style: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    unsafe {
        MessageBoxW(None, PCWSTR(wide(text).as_ptr()), TITLE, MB_TOPMOST | MB_SETFOREGROUND | style)
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let uninstall = args.iter().any(|a| a == "--uninstall");

    // 非管理员 → 自提升（UAC）后退出当前实例
    if !is_elevated() {
        if !relaunch_elevated() {
            msg_box("安装需要管理员权限，但提权被取消。\n请右键选择「以管理员身份运行」后重试。", MB_OK | MB_ICONERROR);
        }
        return;
    }

    if uninstall {
        let code = uninstall_all();
        if code == 0 {
            msg_box("Convallaria Input 已卸载。\n\n注：若个别文件因被占用暂未删除，重启后将自动可删。",
                    MB_OK | MB_ICONINFORMATION);
        } else {
            msg_box("卸载过程中出现错误，请截图反馈。", MB_OK | MB_ICONERROR);
        }
        return;
    }

    // —— 安装 ——
    let code = install_all();
    if code != 0 {
        msg_box("安装过程中出现错误，请截图反馈。", MB_OK | MB_ICONERROR);
        return;
    }

    // 询问是否设为当前输入法（默认不打扰——搜狗等既有输入法不受影响，Win+空格 可随时切换）
    let set_current = msg_box(
        "✔ Convallaria Input 安装完成！\n\n按 Win+空格 可随时在输入法间切换（搜狗等原输入法不受影响）。\n\n是否现在将输入切换为 Convallaria？",
        MB_YESNO | MB_ICONQUESTION,
    ) == IDYES;

    if set_current {
        activate_profile_for_current_user();
        msg_box(
            "已切换到 Convallaria。打开任意输入框（如记事本），直接打拼音即可：\n\n  • nihao → 候选窗数字选词，空格上屏\n  • 回车上屏原文，Esc 取消，Shift 切中英，Ctrl+` 切模式",
            MB_OK | MB_ICONINFORMATION,
        );
    } else {
        msg_box(
            "已保留你当前的输入法。需要时按 Win+空格 切换到 Convallaria 即可。",
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

/// 注销 TIP 相关注册表树（清理旧手写版本残留）。
fn unregister_tip() {
    unsafe {
        let _ = reg_delete_tree(HKEY_LOCAL_MACHINE, &wide(&format!("SOFTWARE\\Microsoft\\CTF\\TIP\\{TIP_CLSID}")));
        let _ = reg_delete_tree(HKEY_LOCAL_MACHINE, &wide(&format!("SOFTWARE\\Classes\\CLSID\\{TIP_CLSID}")));
    }
}

// —— 安装 ——

fn install_all() -> i32 {
    // 0) 旧版本清理（幂等）
    unregister_tip();

    // 1) 安装目录：%ProgramFiles%\Convallaria Input
    let Some(program_files) = std::env::var_os("ProgramFiles").map(PathBuf::from) else {
        return 1;
    };
    let install_dir = program_files.join("Convallaria Input");
    if std::fs::create_dir_all(&install_dir).is_err() {
        return 1;
    }

    // 2) 释放 DLL 与词典
    let dll_path = install_dir.join("convallaria_windows.dll");
    let dict_path = install_dir.join("dictionary.bin");

    // 2.5) 重启管理器：自动关闭占用输入法 DLL 的应用（完成后自动恢复）
    let rm_session = {
        let (session, locked_by) = rm::shutdown_locking_apps(&dll_path.to_string_lossy());
        if !locked_by.is_empty() {
            let list = locked_by.join("、");
            let yes = msg_box(
                &format!(
                    "以下应用正在使用输入法，安装需要临时关闭它们（安装完成后将自动重新打开）：\n\n{list}\n\n继续安装？"
                ),
                MB_YESNO | MB_ICONQUESTION,
            );
            if yes != IDYES {
                rm::abort(session);
                msg_box("安装已取消，原有输入法未受影响。", MB_OK | MB_ICONINFORMATION);
                return 1;
            }
        }
        session
    };

    if std::fs::write(&dll_path, IME_DLL).is_err() {
        rm::restart_and_end(rm_session);
        msg_box(
            "写入 DLL 失败：有应用未能自动关闭。\n请关闭相关应用后重试，或重启电脑后再安装。",
            MB_OK | MB_ICONERROR,
        );
        return 1;
    }
    if std::fs::write(&dict_path, DICTIONARY).is_err() {
        rm::restart_and_end(rm_session);
        return 1;
    }
    // 词典同时放到用户配置目录（引擎查找路径）
    if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
        let dest = appdata.join("Convallaria");
        let _ = std::fs::create_dir_all(&dest);
        let _ = std::fs::write(dest.join("dictionary.bin"), DICTIONARY);
    }

    // 3) 注册文本服务：调用 DLL 的 DllRegisterServer（InprocServer32 + TSF
    //    Profile + 全部键盘/环境类别，注册逻辑只维护这一份）
    unsafe {
        let lib = windows::Win32::System::LibraryLoader::LoadLibraryW(PCWSTR(wide(&dll_path.to_string_lossy()).as_ptr()))
            .unwrap_or_default();
        if lib.is_invalid() {
            rm::restart_and_end(rm_session);
            msg_box("加载输入法 DLL 失败。", MB_OK | MB_ICONERROR);
            return 1;
        }
        let proc_addr = windows::Win32::System::LibraryLoader::GetProcAddress(
            lib,
            windows_core::s!("DllRegisterServer"),
        );
        match proc_addr {
            Some(f) => {
                let register: unsafe extern "system" fn() -> windows_core::HRESULT =
                    std::mem::transmute(f);
                if register().is_err() {
                    let _ = windows::Win32::Foundation::FreeLibrary(lib);
                    rm::restart_and_end(rm_session);
                    msg_box("文本服务注册失败（DllRegisterServer）。", MB_OK | MB_ICONERROR);
                    return 1;
                }
            }
            None => {
                let _ = windows::Win32::Foundation::FreeLibrary(lib);
                rm::restart_and_end(rm_session);
                msg_box("DLL 缺少 DllRegisterServer 导出。", MB_OK | MB_ICONERROR);
                return 1;
            }
        }
        let _ = windows::Win32::Foundation::FreeLibrary(lib);
    }
    println!("✔ 文本服务已注册");

    // 5) 为当前用户启用语言配置（出现在 Win+空格 列表中；不改变当前激活的输入法）
    enable_profile_for_current_user();

    // 6) 卸载入口（控制面板「应用和功能」）
    let setup_in_dir = install_dir.join("convallaria-setup.exe");
    let _ = std::fs::copy(std::env::current_exe().unwrap_or_default(), &setup_in_dir);
    let uninstall_key = format!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{TIP_CLSID}");
    set_reg_str(&uninstall_key, "DisplayName", "Convallaria Input（铃兰输入法）");
    set_reg_str(&uninstall_key, "DisplayVersion", VERSION);
    set_reg_str(&uninstall_key, "Publisher", "Convallaria Project");
    set_reg_str(&uninstall_key, "UninstallString", &format!("\"{}\" --uninstall", setup_in_dir.display()));
    set_reg_str(&uninstall_key, "DisplayIcon", &setup_in_dir.display().to_string());
    set_reg_dword(&uninstall_key, "NoModify", 1);
    set_reg_dword(&uninstall_key, "NoRepair", 1);

    // 恢复被临时关闭的应用
    rm::restart_and_end(rm_session);
    0
}

// —— 卸载 ——

fn uninstall_all() -> i32 {
    // 安装目录里的 DLL 路径（注销与文件删除都要用）
    let dll_path = std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .map(|pf| pf.join("Convallaria Input").join("convallaria_windows.dll"));

    // 移除语言配置（对当前用户）
    disable_profile_for_current_user();

    // 移除卸载入口
    unsafe {
        let _ = reg_delete_tree(
            HKEY_LOCAL_MACHINE,
            &wide(&format!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{TIP_CLSID}")),
        );
    }

    // 删除文件：先用重启管理器关闭占用输入法的应用
    let (rm_session, _) = dll_path
        .as_deref()
        .map(|p| p.to_string_lossy().into_owned())
        .map(|ref p| rm::shutdown_locking_apps(p))
        .unwrap_or((0, Vec::new()));

    // 文本服务注销（Profile / 类别 / CLSID 一次清掉）
    if let Some(dll) = dll_path.as_deref() {
        unsafe {
            if let Ok(lib) = windows::Win32::System::LibraryLoader::LoadLibraryW(PCWSTR(wide(&dll.to_string_lossy()).as_ptr())) {
                if let Some(f) = windows::Win32::System::LibraryLoader::GetProcAddress(
                    lib,
                    windows_core::s!("DllUnregisterServer"),
                ) {
                    let unregister: unsafe extern "system" fn() -> windows_core::HRESULT =
                        std::mem::transmute(f);
                    let _ = unregister();
                }
                let _ = windows::Win32::Foundation::FreeLibrary(lib);
            }
        }
    }

    if let Some(program_files) = std::env::var_os("ProgramFiles").map(PathBuf::from) {
        let dir = program_files.join("Convallaria Input");
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
    if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
        let _ = std::fs::remove_file(appdata.join("Convallaria").join("dictionary.bin"));
    }
    rm::restart_and_end(rm_session);
    0
}

// —— TSF 语言配置 ——

/// 仅启用（出现在切换列表），不改变当前激活的输入法。
fn enable_profile_for_current_user() {
    set_profile_enabled(true, false);
}

fn disable_profile_for_current_user() {
    set_profile_enabled(false, false);
}

/// 激活为当前输入法（用户在弹窗中选择「是」时调用）。
fn activate_profile_for_current_user() {
    set_profile_enabled(true, true);
}

fn set_profile_enabled(enable: bool, activate: bool) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if let Ok(profiles) = CoCreateInstance::<_, ITfInputProcessorProfiles>(
            &CLSID_TF_InputProcessorProfiles,
            None,
            CLSCTX_INPROC_SERVER,
        ) {
            let clsid = guid_of(TIP_CLSID);
            let profile = guid_of(PROFILE_GUID);
            let _ = profiles.AddLanguageProfile(
                &clsid,
                LANGID_ZH_CN,
                &profile,
                &wide("Convallaria Input\0"),
                &wide(""),
                0,
            );
            let _ = profiles.EnableLanguageProfile(&clsid, LANGID_ZH_CN, &profile, enable);
            if activate {
                let _ = profiles.ActivateLanguageProfile(&clsid, LANGID_ZH_CN, &profile);
            }
        }
        windows::Win32::System::Com::CoUninitialize();
    }
}

fn guid_of(s: &str) -> GUID {
    let trimmed = s.trim_matches(|c| c == '{' || c == '}');
    let mut parts = trimmed.split('-');
    let d1 = u32::from_str_radix(parts.next().unwrap_or("0"), 16).unwrap_or(0);
    let d2 = u16::from_str_radix(parts.next().unwrap_or("0"), 16).unwrap_or(0);
    let d3 = u16::from_str_radix(parts.next().unwrap_or("0"), 16).unwrap_or(0);
    let mut bytes = [0u8; 8];
    let d4 = parts.next().unwrap_or("0000");
    for (i, b) in d4.as_bytes().rchunks(2).rev().enumerate() {
        bytes[i] = u8::from_str_radix(std::str::from_utf8(b).unwrap_or("0"), 16).unwrap_or(0);
    }
    let d4b = parts.next().unwrap_or("000000000000");
    for (i, b) in d4b.as_bytes().chunks(2).enumerate() {
        bytes[2 + i] = u8::from_str_radix(std::str::from_utf8(b).unwrap_or("0"), 16).unwrap_or(0);
    }
    GUID::from_values(d1, d2, d3, bytes)
}


// —— 注册表辅助 ——

fn set_reg_str(subkey: &str, name: &str, value: &str) {
    unsafe {
        let mut hkey = HKEY::default();
        if RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(wide(subkey).as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut hkey,
            None,
        )
        .is_ok()
        {
            let value_w = wide(value);
            let mut data: Vec<u8> = Vec::with_capacity(value_w.len() * 2 + 2);
            for c in &value_w {
                data.extend_from_slice(&c.to_le_bytes());
            }
            data.extend_from_slice(&[0, 0]);
            let _ = RegSetValueExW(
                hkey,
                PCWSTR(wide(name).as_ptr()),
                None,
                REG_SZ,
                Some(&data),
            );
            let _ = RegCloseKey(hkey);
        }
    }
}

fn set_reg_dword(subkey: &str, name: &str, value: u32) {
    unsafe {
        let mut hkey = HKEY::default();
        if RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(wide(subkey).as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut hkey,
            None,
        )
        .is_ok()
        {
            let _ = RegSetValueExW(
                hkey,
                PCWSTR(wide(name).as_ptr()),
                None,
                REG_DWORD,
                Some(&value.to_le_bytes()),
            );
            let _ = RegCloseKey(hkey);
        }
    }
}

unsafe fn reg_delete_tree(root: HKEY, subkey: &[u16]) -> WIN32_ERROR {
    unsafe { windows::Win32::System::Registry::RegDeleteTreeW(root, PCWSTR(subkey.as_ptr())) }
}

// —— 重启管理器：自动关闭占用输入法文件的应用（安装后自动恢复）——

mod rm {
    use windows::Win32::Foundation::WIN32_ERROR;
    use windows::Win32::System::RestartManager::*;
    use windows_core::PCWSTR;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 关闭正在占用 `path` 的应用。返回 (会话句柄, 被关闭的应用名列表)；
    /// 会话句柄非零时调用方须在完成后执行 [`restart`] 与 [`end`]。
    pub fn shutdown_locking_apps(path: &str) -> (u32, Vec<String>) {
        unsafe {
            let mut session = 0u32;
            let mut key = [0u16; 33]; // CCH_RM_SESSION_KEY + 1
            if RmStartSession(&mut session, None, windows_core::PWSTR(key.as_mut_ptr())) != WIN32_ERROR(0) {
                return (0, Vec::new());
            }
            let file: Vec<u16> = wide(path);
            if RmRegisterResources(session, Some(&[PCWSTR(file.as_ptr())]), None, None)
                != WIN32_ERROR(0)
            {
                let _ = RmEndSession(session);
                return (0, Vec::new());
            }

            // 两次调用：先取所需容量，再取列表
            let mut needed = 0u32;
            let mut count = 0u32;
            let mut reboot_reasons = 0u32;
            let _ = RmGetList(
                session,
                &mut needed,
                &mut count,
                None,
                &mut reboot_reasons,
            );
            if needed == 0 {
                let _ = RmEndSession(session);
                return (0, Vec::new());
            }
            let mut apps = vec![RM_PROCESS_INFO::default(); needed as usize];
            count = needed;
            if RmGetList(
                session,
                &mut needed,
                &mut count,
                Some(apps.as_mut_ptr()),
                &mut reboot_reasons,
            ) != WIN32_ERROR(0)
            {
                let _ = RmEndSession(session);
                return (0, Vec::new());
            }
            apps.truncate(count as usize);
            let names: Vec<String> = apps
                .iter()
                .map(|a| {
                    let end = a
                        .strAppName
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(a.strAppName.len());
                    String::from_utf16_lossy(&a.strAppName[..end])
                })
                .collect();
            if names.is_empty() {
                let _ = RmEndSession(session);
                return (0, Vec::new());
            }

            // 优雅关闭（应用会收到保存提示）；失败由调用方兜底
            if RmShutdown(session, 0, None) != WIN32_ERROR(0) {
                let _ = RmEndSession(session);
                return (0, Vec::new());
            }
            (session, names)
        }
    }

    /// 恢复被关闭的应用并结束会话。
    pub fn restart_and_end(session: u32) {
        if session != 0 {
            unsafe {
                let _ = RmRestart(session, None, None);
                let _ = RmEndSession(session);
            }
        }
    }

    /// 结束会话（不恢复应用，用于用户取消的路径）。
    pub fn abort(session: u32) {
        if session != 0 {
            unsafe {
                let _ = RmEndSession(session);
            }
        }
    }
}

// —— 提权 ——

fn is_elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut ret_len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret_len,
        )
        .is_ok();
        let _ = ERROR_INSUFFICIENT_BUFFER;
        ok && elevation.TokenIsElevated != 0
    }
}

/// 以管理员身份重启自身（UAC），成功返回 true（父进程应随即退出）。
fn relaunch_elevated() -> bool {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return false,
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let params = format!("\"{}\" {}", exe.display(), args.join(" "));
    // 宽字符串缓冲区必须存活到 ShellExecuteExW 调用之后：
    // SHELLEXECUTEINFOW 只存裸指针，把临时 Vec 的指针存进结构体会变成悬垂指针
    // （症状：系统弹「找不到文件『随机乱码』」）。
    let exe_w = wide(&exe.to_string_lossy());
    let params_w = wide(&params);
    let verb = w!("runas");
    unsafe {
        let mut sei = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: windows::Win32::UI::Shell::SEE_MASK_DEFAULT,
            lpVerb: PCWSTR(verb.as_ptr()),
            lpFile: PCWSTR(exe_w.as_ptr()),
            lpParameters: PCWSTR(params_w.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        // ShellExecuteExW 的 hInstApp 等字段由系统填充
        ShellExecuteExW(&mut sei).is_ok()
    }
}
