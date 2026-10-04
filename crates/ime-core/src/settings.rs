//! 设置：配置模型、持久化与文件监视热生效。
//!
//! 配置文件：`%APPDATA%\Convallaria\config.toml`（可用环境变量
//! `CONVALLARIA_CONFIG_DIR` 覆盖目录，测试即依赖此机制）。

use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use notify::Watcher;
use serde::{Deserialize, Serialize};

use crate::mode::{InputMode, ShuangpinScheme};

/// 用户配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 输入模式。
    pub mode: InputMode,
    /// 双拼方案（仅双拼模式使用）。
    pub scheme: ShuangpinScheme,
    /// 候选窗每页候选数。
    pub page_size: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mode: InputMode::Quanpin,
            scheme: ShuangpinScheme::Flypy,
            page_size: 9,
        }
    }
}

/// 配置目录：环境变量覆盖 → `%APPDATA%\Convallaria`。
pub fn default_config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CONVALLARIA_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return PathBuf::from(appdata).join("Convallaria");
    }
    // 非 Windows 兜底（跨平台预留）
    let home = std::env::var_os("HOME").map(PathBuf::from);
    home.unwrap_or_else(std::env::temp_dir).join(".convallaria")
}

/// 配置存取。
pub struct ConfigStore {
    dir: PathBuf,
}

impl ConfigStore {
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn default_store() -> Self {
        Self::at(default_config_dir())
    }

    /// config.toml 完整路径。
    pub fn path(&self) -> PathBuf {
        self.dir.join("config.toml")
    }

    /// 读取配置；无文件时写入默认值并返回默认（首次启动语义）。
    pub fn load_or_init(&self) -> std::io::Result<Settings> {
        match std::fs::read_to_string(self.path()) {
            Ok(text) => Ok(toml::from_str(&text).unwrap_or_default()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let s = Settings::default();
                self.save(&s)?;
                Ok(s)
            }
            Err(e) => Err(e),
        }
    }

    /// 读取现有配置（不落盘）。
    pub fn load(&self) -> std::io::Result<Settings> {
        let text = std::fs::read_to_string(self.path())?;
        Ok(toml::from_str(&text).unwrap_or_default())
    }

    /// 保存配置。
    pub fn save(&self, s: &Settings) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let text = toml::to_string_pretty(s)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(self.path(), text)
    }

    /// 载入配置并开启文件监视；配置文件变更后原子替换运行时快照。
    pub fn watch(self: &Arc<Self>) -> std::io::Result<SettingsHandle> {
        let initial = Arc::new(self.load_or_init()?);
        let snapshot = Arc::new(RwLock::new(initial));
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let mut watcher = notify::recommended_watcher(
            move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res {
                    let touched = event.paths.iter().any(|p| {
                        p.file_name().map(|f| f == "config.toml").unwrap_or(false)
                    });
                    if touched {
                        let _ = tx.send(());
                    }
                }
            },
        )
        .map_err(std::io::Error::other)?;
        watcher
            .watch(&self.path(), notify::RecursiveMode::NonRecursive)
            .map_err(std::io::Error::other)?;

        let store = Arc::clone(self);
        let snap = Arc::clone(&snapshot);
        std::thread::Builder::new()
            .name("config-watcher".into())
            .spawn(move || {
                let _keep_alive = watcher; // 监视器必须在线程内存活
                for _ in &rx {
                    // 防抖：编辑器常多次写盘
                    std::thread::sleep(Duration::from_millis(80));
                    while rx.try_recv().is_ok() {}
                    if let Ok(s) = store.load_or_init() {
                        *snap.write().expect("配置快照锁") = Arc::new(s);
                    }
                }
            })?;
        Ok(SettingsHandle {
            store: Arc::clone(self),
            snapshot,
        })
    }
}

/// 运行时配置快照句柄：读侧 O(1) 克隆 Arc，监视线程原子替换。
pub struct SettingsHandle {
    store: Arc<ConfigStore>,
    snapshot: Arc<RwLock<Arc<Settings>>>,
}

impl SettingsHandle {
    /// 当前配置快照。
    pub fn get(&self) -> Arc<Settings> {
        self.snapshot.read().expect("配置快照锁").clone()
    }

    /// 立即从磁盘重载（监视事件丢失时的兜底路径）。
    pub fn reload(&self) -> std::io::Result<Arc<Settings>> {
        let s = Arc::new(self.store.load_or_init()?);
        *self.snapshot.write().expect("配置快照锁") = s.clone();
        Ok(s)
    }

    /// 轮询等待快照满足条件（测试与前端可选使用）。
    pub fn wait_for(&self, timeout: Duration, check: impl Fn(&Settings) -> bool) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if check(&self.get()) {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return check(&self.get());
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "convallaria-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn 首次启动_默认值并落盘() {
        let dir = temp_dir("init");
        let store = ConfigStore::at(&dir);
        let s = store.load_or_init().unwrap();
        assert_eq!(s, Settings::default());
        assert_eq!(s.mode, InputMode::Quanpin);
        assert_eq!(s.scheme, ShuangpinScheme::Flypy);
        assert_eq!(s.page_size, 9);
        assert!(store.path().exists(), "默认配置应已写入磁盘");
    }

    #[test]
    fn 重启保留修改() {
        let dir = temp_dir("persist");
        let store = ConfigStore::at(&dir);
        let mut s = store.load_or_init().unwrap();
        s.mode = InputMode::Wubi;
        s.scheme = ShuangpinScheme::Mspy;
        store.save(&s).unwrap();

        // 模拟重启：全新 store 实例
        let again = ConfigStore::at(&dir).load().unwrap();
        assert_eq!(again.mode, InputMode::Wubi);
        assert_eq!(again.scheme, ShuangpinScheme::Mspy);
    }

    #[test]
    fn 多用户目录相互隔离() {
        let dir_a = temp_dir("user-a");
        let dir_b = temp_dir("user-b");
        let mut s = ConfigStore::at(&dir_a).load_or_init().unwrap();
        s.mode = InputMode::Wubi;
        ConfigStore::at(&dir_a).save(&s).unwrap();
        // 另一目录仍是默认
        let other = ConfigStore::at(&dir_b).load_or_init().unwrap();
        assert_eq!(other.mode, InputMode::Quanpin);
        assert_ne!(ConfigStore::at(&dir_a).load().unwrap().mode, other.mode);
    }

    #[test]
    fn 配置文件热生效() {
        let dir = temp_dir("hot");
        let store = std::sync::Arc::new(ConfigStore::at(&dir));
        let handle = store.watch().unwrap();
        assert_eq!(handle.get().page_size, 9, "初始应为默认 9");

        // 另一写者修改配置文件
        let mut s = (*handle.get()).clone();
        s.page_size = 5;
        ConfigStore::at(&dir).save(&s).unwrap();

        // 无需重启：快照自动更新
        assert!(
            handle.wait_for(std::time::Duration::from_secs(5), |s| s.page_size == 5),
            "配置热生效失败：page_size 仍为 {}",
            handle.get().page_size
        );
    }

    #[test]
    fn 配置损坏时回退默认() {
        let dir = temp_dir("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), "这不是 TOML {{{").unwrap();
        let s = ConfigStore::at(&dir).load().unwrap();
        assert_eq!(s, Settings::default());
    }
}
