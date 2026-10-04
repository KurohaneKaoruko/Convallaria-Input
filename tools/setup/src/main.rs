//! Convallaria Input 安装包（单文件 setup.exe，GUI 弹窗交互）。
//!
//! 用法：
//! - 双击运行（或命令行直接运行）：安装；非管理员时自动弹 UAC 重启自身
//! - `convallaria-setup.exe --uninstall`：卸载
//!
//! 自包含：DLL 与词典在编译期嵌入（见 `include_bytes!`），
//! 产物是单个 exe，可直接分发。
//! 全程写日志到 `%TEMP%\convallaria-setup.log`，失败可据此定位步骤。

#![cfg(windows)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::Write as _;
use std::path::PathBuf;

use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, HANDLE, WIN32_ERROR};
use windows::Win32::Security::{
    GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::{ShellExecuteExW, SHELLEXECUTEINFOW};
use windows::Win32::UI::TextServices::{
    ITfInputProcessorProfiles, CLSID_TF_InputProcessorProfiles,
};
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

fn log(msg: &str) {
    let path = std::env::temp_dir().join("convallaria-setup.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "[{ts}] {msg}");
    }
}

fn msg_box(text: &str, style: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    unsafe {
        MessageBoxW(None, PCWSTR(wide(text).as_ptr()), TITLE, MB_TOPMOST | MB_SETFOREGROUND | style)
    }
}

/// 清理旧手写版本可能残留的注册表项（幂等）。
fn unregister_tip() {
    unsafe {
        let _ = reg_delete_tree(HKEY_LOCAL_MACHINE, &wide(&format!("SOFTWARE\\Microsoft\\CTF\\TIP\\{TIP_CLSID}")));
        let _ = reg_delete_tree(HKEY_LOCAL_MACHINE, &wide(&format!("SOFTWARE\\Classes\\CLSID\\{TIP_CLSID}")));
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
        log("非管理员，请求提权…");
        if !relaunch_elevated() {
            log("提权被取消");
            msg_box("安装需要管理员权限，但提权被取消。\n请右键选择「以管理员身份运行」后重试。", MB_OK | MB_ICONERROR);
        }
        return;
    }

    let code = if uninstall { uninstall_all() } else { install_all() };

    if code == 0 {
        println!("\n✔ 操作成功完成。");
        if !uninstall {
            println!("  按 Win+空格 或点击任务栏语言「中」图标切换到 Convallaria Input。");
        }
    }
    if !args.iter().any(|a| a == "--silent") {
        println!("\n按回车键退出…");
        let _ = std::io::stdin().read_line(&mut String::new());
    }
    std::process::exit(code);
}

// —— 安装 ——

fn install_all() -> i32 {
    log("== 安装开始 ==");
    println!("Convallaria Input（铃兰输入法）v{VERSION}");

    // 0) 旧版本清理（幂等）
    unregister_tip();
    log("旧版本清理完成");

    // 1) 安装目录（版本化）：%ProgramFiles%\Convallaria Input\<版本>\
    //    每次安装进新目录、注册表指向新目录，绕开「旧 DLL 被系统加载导致无法覆盖」
    //    的文件锁；旧版本目录在安装末尾异步清理。
    let Some(program_files) = std::env::var_os("ProgramFiles").map(PathBuf::from) else {
        log("失败：无 ProgramFiles 环境变量");
        msg_box("安装失败：未找到 ProgramFiles 环境变量。", MB_OK | MB_ICONERROR);
        return 1;
    };
    let base_dir = program_files.join("Convallaria Input");
    let install_dir = base_dir.join(VERSION);
    if let Err(e) = std::fs::create_dir_all(&install_dir) {
        log(&format!("失败：创建目录 {:?}: {e}", install_dir));
        msg_box(&format!("安装失败：无法创建目录 {}\n{e}", install_dir.display()), MB_OK | MB_ICONERROR);
        return 1;
    }

    // 2) 释放 DLL 与词典
    let dll_path = install_dir.join("convallaria_windows.dll");
    let dict_path = install_dir.join("dictionary.bin");

    // 2.5) 重启管理器：礼貌关闭正在使用旧版本 DLL 的应用（完成后自动恢复）。
    //      新版本写入新目录，不存在文件锁；此步只为让旧输入法尽快退出内存。
    let old_dll = base_dir.join("convallaria_windows.dll");
    let rm_session = {
        log("重启管理器：检测占用…");
        // 收集所有现存版本的 DLL 路径（旧扁平布局 + 各版本子目录）
        let mut targets: Vec<String> = Vec::new();
        if old_dll.exists() {
            targets.push(old_dll.to_string_lossy().into_owned());
        }
        if let Ok(entries) = std::fs::read_dir(&base_dir) {
            for entry in entries.flatten() {
                let candidate = entry.path().join("convallaria_windows.dll");
                if candidate.exists() {
                    targets.push(candidate.to_string_lossy().into_owned());
                }
            }
        }
        let (session, locked_by) = rm::shutdown_locking_apps(&targets);
        log(&format!("重启管理器：占用应用 {} 个", locked_by.len()));
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
                log("用户取消（占用确认弹窗）");
                msg_box("安装已取消，原有输入法未受影响。", MB_OK | MB_ICONINFORMATION);
                return 1;
            }
        }
        session
    };

    if let Err(e) = std::fs::write(&dll_path, IME_DLL) {
        rm::restart_and_end(rm_session);
        log(&format!("失败：写 DLL: {e}"));
        msg_box(
            &format!("安装失败：写入 DLL 失败\n{e}"),
            MB_OK | MB_ICONERROR,
        );
        return 1;
    }
    if let Err(e) = std::fs::write(&dict_path, DICTIONARY) {
        rm::restart_and_end(rm_session);
        log(&format!("失败：写词典: {e}"));
        msg_box(&format!("安装失败：写入词典失败\n{e}"), MB_OK | MB_ICONERROR);
        return 1;
    }
    log("DLL 与词典已写入");
    // 词典同时放到用户配置目录（引擎查找路径）
    if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
        let dest = appdata.join("Convallaria");
        let _ = std::fs::create_dir_all(&dest);
        let _ = std::fs::write(dest.join("dictionary.bin"), DICTIONARY);
    }

    // 3) 注册文本服务：调用 DLL 的 DllRegisterServer
    //    （InprocServer32 + TSF Profile + 全部键盘/环境类别，注册逻辑只维护这一份）
    unsafe {
        let lib = windows::Win32::System::LibraryLoader::LoadLibraryW(PCWSTR(wide(
            &dll_path.to_string_lossy(),
        )
        .as_ptr()))
        .unwrap_or_default();
        if lib.is_invalid() {
            let err = std::io::Error::last_os_error();
            rm::restart_and_end(rm_session);
            log(&format!("失败：LoadLibraryW {err}"));
            msg_box(
                &format!("安装失败：加载输入法 DLL 失败\n{err}"),
                MB_OK | MB_ICONERROR,
            );
            return 1;
        }
        log("DLL 已加载");
        let proc_addr = windows::Win32::System::LibraryLoader::GetProcAddress(
            lib,
            windows_core::s!("DllRegisterServer"),
        );
        match proc_addr {
            Some(f) => {
                let register: unsafe extern "system" fn() -> windows_core::HRESULT =
                    std::mem::transmute(f);
                let hr = register();
                if hr.is_err() {
                    let _ = windows::Win32::Foundation::FreeLibrary(lib);
                    rm::restart_and_end(rm_session);
                    log(&format!("失败：DllRegisterServer hr={hr:?}"));
                    msg_box(
                        &format!("安装失败：文本服务注册失败\n{hr:?}"),
                        MB_OK | MB_ICONERROR,
                    );
                    return 1;
                }
                log("DllRegisterServer 成功");
            }
            None => {
                let _ = windows::Win32::Foundation::FreeLibrary(lib);
                rm::restart_and_end(rm_session);
                log("失败：DLL 缺少 DllRegisterServer 导出");
                msg_box("安装失败：DLL 缺少 DllRegisterServer 导出。", MB_OK | MB_ICONERROR);
                return 1;
            }
        }
        let _ = windows::Win32::Foundation::FreeLibrary(lib);
    }
    println!("✔ 文本服务已注册");

    // 4) 为当前用户启用语言配置（出现在 Win+空格 列表中；不改变当前激活的输入法）
    enable_profile_for_current_user();
    log("语言配置已启用");

    // 5) 卸载入口（控制面板「应用和功能」）
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
    log("卸载入口已创建");

    // 6) 询问是否设为当前输入法（默认不打扰——既有输入法不受影响，Win+空格 可随时切换）
    let set_current = msg_box(
        "✔ Convallaria Input 安装完成！\n\n按 Win+空格 可随时在输入法间切换（搜狗等原输入法不受影响）。\n\n是否现在将输入切换为 Convallaria？",
        MB_YESNO | MB_ICONQUESTION,
    ) == IDYES;

    if set_current {
        activate_profile_for_current_user();
        log("已激活为当前输入法（用户选择）");
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
    // 7) 清理旧版本目录（旧 DLL 可能仍被已运行的应用加载——尽力删除，失败忽略，
    //    不影响本次安装；这些目录会在下次升级时再被清理）
    let mut removed_old = false;
    if let Ok(entries) = std::fs::read_dir(&base_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir()
                && p != install_dir
                && std::fs::remove_dir_all(&p).is_ok()
            {
                removed_old = true;
            }
            // 旧式扁平布局的残留文件（早期版本直接放在根目录）
            if p.is_file() {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
    log(&format!("旧版本清理: {}", if removed_old { "已删除" } else { "无或被占用（稍后自动清理）" }));
    log("== 安装完成 ==");
    0
}

// —— 卸载 ——

fn uninstall_all() -> i32 {
    log("== 卸载开始 ==");
    // 安装目录里的 DLL 路径（注销与文件删除都要用）
    let dll_path = std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .map(|pf| pf.join("Convallaria Input").join("convallaria_windows.dll"));

    // 移除语言配置（对当前用户）
    disable_profile_for_current_user();
    log("语言配置已禁用");

    // 移除卸载入口
    unsafe {
        let _ = reg_delete_tree(
            HKEY_LOCAL_MACHINE,
            &wide(&format!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{TIP_CLSID}")),
        );
    }

    // 删除文件：先用重启管理器关闭占用输入法的应用（含所有版本的 DLL）
    let mut targets: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(
        std::env::var_os("ProgramFiles")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("Convallaria Input"),
    ) {
        for entry in entries.flatten() {
            let candidate = entry.path().join("convallaria_windows.dll");
            if candidate.exists() {
                targets.push(candidate.to_string_lossy().into_owned());
            }
        }
    }
    let (rm_session, _) = rm::shutdown_locking_apps(&targets);

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
                    log("DllUnregisterServer 成功");
                }
                let _ = windows::Win32::Foundation::FreeLibrary(lib);
            }
        }
    }

    if let Some(program_files) = std::env::var_os("ProgramFiles").map(PathBuf::from) {
        let dir = program_files.join("Convallaria Input");
        if dir.exists() {
            if let Err(e) = std::fs::remove_dir_all(&dir) {
                log(&format!("警告：删除目录 {}: {e}（文件被占用时属正常，重启后可删）", dir.display()));
            } else {
                println!("✔ 文件已删除");
            }
        }
    }
    if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
        let _ = std::fs::remove_file(appdata.join("Convallaria").join("dictionary.bin"));
    }
    rm::restart_and_end(rm_session);
    log("== 卸载完成 ==");
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
                &wide("\0"),
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

    /// 关闭正在占用 `paths` 中任一文件的应用。返回 (会话句柄, 被关闭的应用名列表)；
    /// 会话句柄非零时调用方须在完成后执行 [`super::rm::restart_and_end`]。
    pub fn shutdown_locking_apps(paths: &[String]) -> (u32, Vec<String>) {
        unsafe {
            let mut session = 0u32;
            let mut key = [0u16; 33]; // CCH_RM_SESSION_KEY + 1
            if RmStartSession(&mut session, None, windows_core::PWSTR(key.as_mut_ptr()))
                != WIN32_ERROR(0)
            {
                return (0, Vec::new());
            }
            let files: Vec<PCWSTR> = paths.iter().map(|p| PCWSTR(wide(p).as_ptr())).collect();
            if RmRegisterResources(session, Some(&files), None, None) != WIN32_ERROR(0)
            {
                let _ = RmEndSession(session);
                return (0, Vec::new());
            }

            // 两次调用：先取所需容量，再取列表
            let mut needed = 0u32;
            let mut count = 0u32;
            let mut reboot_reasons = 0u32;
            let _ = RmGetList(session, &mut needed, &mut count, None, &mut reboot_reasons);
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
    // SHELLEXECUTEINFOW 只存裸指针，把临时 Vec 的指针存进结构体会变成悬垂指针。
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
        ShellExecuteExW(&mut sei).is_ok()
    }
}