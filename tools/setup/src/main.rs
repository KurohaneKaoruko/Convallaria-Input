//! Convallaria Input 安装包（单文件 setup.exe）。
//!
//! 用法：
//! - 双击运行（或命令行直接运行）：安装；非管理员时自动弹 UAC 重启自身
//! - `convallaria-setup.exe --uninstall`：卸载
//! - `--silent`：结束时不等待回车
//!
//! 自包含：DLL 与词典在编译期嵌入（见 `include_bytes!`），
//! 产物是单个 exe，可直接分发。

#![cfg(windows)]

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
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows_core::{w, GUID, PCWSTR};

/// 嵌入的输入法 DLL（由 build-installer.ps1 构建到 target\pack，避免与已安装的 DLL 抢锁）。
const IME_DLL: &[u8] = include_bytes!("../../../target/pack/release/convallaria_windows.dll");
/// 嵌入的词典。
const DICTIONARY: &[u8] = include_bytes!("../../../assets/dicts/convallaria.dict.bin");

const TIP_CLSID: &str = "{8A5C7B60-4C2A-4E1F-9D3B-5C0A11B2C001}";
const PROFILE_GUID: &str = "{8A5C7B60-4C2A-4E1F-9D3B-5C0A11B2C002}";
const CAT_KEYBOARD: &str = "{34745CFF-BF55-4F84-9AD5-5B36CE96EF02}";
const LANGID_ZH_CN: u16 = 0x0804;
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let uninstall = args.iter().any(|a| a == "--uninstall");
    let silent = args.iter().any(|a| a == "--silent");

    println!("Convallaria Input（铃兰输入法）v{VERSION}");

    // 非管理员 → 自提升（UAC）后退出当前实例
    if !is_elevated() {
        println!("需要管理员权限，正在请求提权（请在 UAC 弹窗中点「是」）…");
        if !relaunch_elevated() {
            fail("提权被取消。请右键「以管理员身份运行」后重试。");
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
    if !silent {
        println!("\n按回车键退出…");
        let _ = std::io::stdin().read_line(&mut String::new());
    }
    std::process::exit(code);
}

fn fail(msg: &str) {
    eprintln!("✘ {msg}");
    println!("\n按回车键退出…");
    let _ = std::io::stdin().read_line(&mut String::new());
    std::process::exit(1);
}

// —— 安装 ——

fn install_all() -> i32 {
    // 0) 旧版本清理（幂等）
    println!("• 清理旧版本…");
    unregister_tip();

    // 1) 安装目录：%ProgramFiles%\Convallaria Input
    let Some(program_files) = std::env::var_os("ProgramFiles").map(PathBuf::from) else {
        fail("未找到 ProgramFiles 环境变量");
        return 1;
    };
    let install_dir = program_files.join("Convallaria Input");
    if std::fs::create_dir_all(&install_dir).is_err() {
        fail(&format!("无法创建目录 {}", install_dir.display()));
        return 1;
    }

    // 2) 释放 DLL 与词典
    let dll_path = install_dir.join("convallaria_windows.dll");
    let dict_path = install_dir.join("dictionary.bin");
    if let Err(e) = std::fs::write(&dll_path, IME_DLL) {
        fail(&format!("写入 DLL 失败: {e}（若提示被占用，请先卸载旧版本并关闭输入中的应用）"));
        return 1;
    }
    if let Err(e) = std::fs::write(&dict_path, DICTIONARY) {
        fail(&format!("写入词典失败: {e}"));
        return 1;
    }
    // 词典同时放到用户配置目录（引擎查找路径）
    if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
        let dest = appdata.join("Convallaria");
        let _ = std::fs::create_dir_all(&dest);
        let _ = std::fs::write(dest.join("dictionary.bin"), DICTIONARY);
    }
    println!("✔ 文件已安装到 {}", install_dir.display());

    // 3) 注册表：CLSID / InprocServer32
    set_reg_str(
        &format!("SOFTWARE\\Classes\\CLSID\\{TIP_CLSID}"),
        "",
        "Convallaria Input",
    );
    set_reg_str(
        &format!("SOFTWARE\\Classes\\CLSID\\{TIP_CLSID}\\InprocServer32"),
        "",
        &dll_path.to_string_lossy(),
    );
    set_reg_str(
        &format!("SOFTWARE\\Classes\\CLSID\\{TIP_CLSID}\\InprocServer32"),
        "ThreadingModel",
        "Apartment",
    );

    // 4) 注册表：CTF\TIP 与语言档案、键盘类别
    set_reg_str(&format!("SOFTWARE\\Microsoft\\CTF\\TIP\\{TIP_CLSID}"), "", "Convallaria Input");
    let lang_profile = format!("SOFTWARE\\Microsoft\\CTF\\TIP\\{TIP_CLSID}\\LanguageProfile\\{LANGID_ZH_CN:#06x}\\{PROFILE_GUID}");
    set_reg_str(&lang_profile, "", "Convallaria");
    set_reg_dword(&lang_profile, "Enable", 1);
    set_reg_str(
        &format!("SOFTWARE\\Microsoft\\CTF\\TIP\\{TIP_CLSID}\\Category\\{CAT_KEYBOARD}\\{TIP_CLSID}"),
        "",
        "",
    );
    println!("✔ 文本服务已注册");

    // 5) 为当前用户启用语言配置
    enable_profile_for_current_user();
    println!("✔ 语言配置已启用");

    // 6) 卸载入口（控制面板「应用和功能」）
    let setup_in_dir = install_dir.join("convallaria-setup.exe");
    let _ = std::fs::copy(std::env::current_exe().unwrap_or_default(), &setup_in_dir);
    let uninstall_key = format!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{TIP_CLSID}");
    set_reg_str(&uninstall_key, "DisplayName", "Convallaria Input（铃兰输入法）");
    set_reg_str(&uninstall_key, "DisplayVersion", VERSION);
    set_reg_str(&uninstall_key, "Publisher", "Convallaria Project");
    set_reg_str(&uninstall_key, "UninstallString", &format!("\"{}\" --uninstall --silent", setup_in_dir.display()));
    set_reg_str(&uninstall_key, "DisplayIcon", &setup_in_dir.display().to_string());
    set_reg_dword(&uninstall_key, "NoModify", 1);
    set_reg_dword(&uninstall_key, "NoRepair", 1);
    println!("✔ 卸载入口已创建（控制面板 → 应用和功能）");
    0
}

// —— 卸载 ——

fn uninstall_all() -> i32 {
    println!("• 正在卸载 Convallaria Input…");
    unregister_tip();

    // 移除语言配置（对当前用户）
    disable_profile_for_current_user();

    // 移除卸载入口
    unsafe {
        let _ = reg_delete_tree(
            HKEY_LOCAL_MACHINE,
            &hstring_wide(&format!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{TIP_CLSID}")),
        );
    }

    // 删除文件（被占用的 DLL 需要关闭输入中的应用后重试或重启后删除）
    let mut locked = false;
    if let Some(program_files) = std::env::var_os("ProgramFiles").map(PathBuf::from) {
        let dir = program_files.join("Convallaria Input");
        if dir.exists() {
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => println!("✔ 文件已删除"),
                Err(e) => {
                    locked = true;
                    println!("⚠ 部分文件暂无法删除（{e}）：请关闭正在输入的应用后重试，或重启后手动删除 {}", dir.display());
                }
            }
        }
    }
    if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
        let _ = std::fs::remove_file(appdata.join("Convallaria").join("dictionary.bin"));
    }
    let _ = locked;
    0
}

/// 注销 TIP 相关注册表树。
fn unregister_tip() {
    unsafe {
        let _ = reg_delete_tree(HKEY_LOCAL_MACHINE, &hstring_wide(&format!("SOFTWARE\\Microsoft\\CTF\\TIP\\{TIP_CLSID}")));
        let _ = reg_delete_tree(HKEY_LOCAL_MACHINE, &hstring_wide(&format!("SOFTWARE\\Classes\\CLSID\\{TIP_CLSID}")));
        let _ = reg_delete_tree(HKEY_LOCAL_MACHINE, &hstring_wide(&format!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{TIP_CLSID}")));
    }
}

// —— TSF 语言配置 ——

fn enable_profile_for_current_user() {
    set_profile_enabled(true);
}

fn disable_profile_for_current_user() {
    set_profile_enabled(false);
}

fn set_profile_enabled(enable: bool) {
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
                &hstring_wide("Convallaria Input\0"),
                &hstring_wide(""),
                0,
            );
            let _ = profiles.EnableLanguageProfile(&clsid, LANGID_ZH_CN, &profile, enable);
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

fn hstring_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// —— 注册表辅助 ——

fn set_reg_str(subkey: &str, name: &str, value: &str) {
    unsafe {
        let mut hkey = HKEY::default();
        if RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(hstring_wide(subkey).as_ptr()),
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
            let value_w = hstring_wide(value);
            let mut data: Vec<u8> = Vec::with_capacity(value_w.len() * 2 + 2);
            for c in &value_w {
                data.extend_from_slice(&c.to_le_bytes());
            }
            data.extend_from_slice(&[0, 0]);
            let _ = RegSetValueExW(
                hkey,
                PCWSTR(hstring_wide(name).as_ptr()),
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
            PCWSTR(hstring_wide(subkey).as_ptr()),
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
                PCWSTR(hstring_wide(name).as_ptr()),
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
    let exe_w = hstring_wide(&exe.to_string_lossy());
    let params_w = hstring_wide(&params);
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
