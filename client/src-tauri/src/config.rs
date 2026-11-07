//! 配置。
//!
//! * 中转服务器地址和证书指纹的**默认值编译进程序**：构建时用环境变量覆盖，或者修改下面的默认值：
//!
//!   ```powershell
//!   $env:AIRHAND_SERVER="relay.example.com:4433"
//!   $env:AIRHAND_SERVER_FP="<服务器日志中打印的证书 SHA-256 指纹>"
//!   npm run tauri build
//!   ```
//!
//! * **Windows**：程序所在目录（安装目录）下有一个 `config.toml`（见 [`Settings`]）：可以手动改服务器地址和指纹，
//!   还保存着开机自动启动和服务器分配的控制码（固定不变，见 `airhand_proto::signal` 的"固定控制码"）。
//!   文件由被控端维护（安装版是系统服务，见 [`crate::winsvc`]），手动修改的内容和注释都会保留；改了要重新启动 Airhand 才生效。
//! * **macOS / Linux**：没有配置文件（不在磁盘上留下任何东西），控制码每次启动时重新分配。

use std::path::PathBuf;
use std::sync::{Arc, LazyLock};

use airhand_link::LinkConfig;
use airhand_proto::signal::Reclaim;
use anyhow::Result;
use toml_edit::DocumentMut;
use tracing::{error, warn};

/// 编译进程序的中转服务器地址 `host:port`（UDP），`config.toml` 里没有设置时使用。
pub const DEFAULT_SERVER: &str = match option_env!("AIRHAND_SERVER") {
    Some(v) => v,
    None => "127.0.0.1:4433",
};

/// 编译进程序的服务器证书 SHA-256 指纹（64 位十六进制，允许用 `:` 或空白分隔），`config.toml` 里没有设置时使用。
///
/// * 非空：严格校验，指纹不符直接拒绝连接（防中间人）。
/// * 为空：不校验证书，**仅建议在开发环境使用**。
pub const DEFAULT_SERVER_FP: &str = match option_env!("AIRHAND_SERVER_FP") {
    Some(v) => v,
    None => "",
};

// 指纹写错了在编译时就报错，而不是等到运行时连不上服务器（规则与 airhand_link::net::parse_fingerprint 相同）
const _: () = assert!(
    fingerprint_is_valid(DEFAULT_SERVER_FP),
    "AIRHAND_SERVER_FP must be the 64 hex digit fingerprint printed in the server log (`:` and spaces are allowed)"
);

const fn fingerprint_is_valid(s: &str) -> bool {
    let b = s.as_bytes();
    let (mut i, mut digits) = (0, 0);
    while i < b.len() {
        if b[i].is_ascii_hexdigit() {
            digits += 1;
        } else if !(b[i] == b':' || b[i].is_ascii_whitespace()) {
            return false;
        }
        i += 1;
    }
    digits == 0 || digits == 64
}

/// 产品版本
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 配置文件名（在程序所在目录）
#[cfg_attr(any(not(windows), test), allow(dead_code))]
pub const FILE_NAME: &str = "config.toml";

/// 进程启动时读到的配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// 中转服务器地址（`server`）
    pub server: String,
    /// 中转服务器证书指纹（`server_fp`），空字符串表示不校验
    pub server_fp: String,
    /// 开机自动启动（`autostart`，Windows 安装版）
    pub autostart: bool,
    /// 服务器分配的控制码和加密过的密钥（`[code]`），用 [`Settings::code`] 解开
    saved_code: Option<(i64, String)>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            server: DEFAULT_SERVER.to_string(),
            server_fp: DEFAULT_SERVER_FP.to_string(),
            autostart: true,
            saved_code: None,
        }
    }
}

impl Settings {
    /// 保存的控制码和取回它用的密钥。密钥只有保存它的账户（安装版是服务里的被控端，SYSTEM）解得开，
    /// 解不开（例如配置文件是从别的电脑拷来的）时返回 `None`：让服务器重新分配一个
    pub fn code(&self) -> Option<Reclaim> {
        let (code, secret) = self.saved_code.as_ref()?;
        match (u32::try_from(*code), secret::open(secret)) {
            (Ok(code), Ok(secret)) => Some(Reclaim { code, secret }),
            (_, Err(e)) => {
                warn!("can't use the saved control code ({e:#}), getting a new one");
                None
            }
            _ => {
                warn!("invalid saved control code {code}, getting a new one");
                None
            }
        }
    }
}

/// 配置文件的路径：Windows 是程序所在目录（安装目录）下的 `config.toml`；其它平台没有配置文件。
pub fn path() -> Option<PathBuf> {
    #[cfg(all(windows, not(test)))]
    {
        let exe = std::env::current_exe().ok()?;
        Some(exe.parent()?.join(FILE_NAME))
    }
    // 测试不读写配置文件，每次都是新的控制码
    #[cfg(any(not(windows), test))]
    None
}

/// 控制码会不会保存下来（有配置文件）：保存的话，注册时请服务器一直为它保留（`Register::keep`）
pub fn persistent() -> bool {
    path().is_some()
}

/// 进程启动时读到的配置（之后的修改，例如更换控制码，由各自的模块记在内存里）。
pub fn settings() -> &'static Settings {
    static SETTINGS: LazyLock<Settings> = LazyLock::new(|| {
        let Some(path) = path() else {
            return Settings::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => parse(&text).unwrap_or_else(|e| {
                error!("{}: {e:#}; using the defaults", path.display());
                Settings::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Settings::default(),
            Err(e) => {
                error!("can't read {}: {e}; using the defaults", path.display());
                Settings::default()
            }
        }
    });
    &SETTINGS
}

fn parse(text: &str) -> Result<Settings> {
    let doc: DocumentMut = text.parse()?;
    let mut s = Settings::default();
    if let Some(v) = doc.get("server").and_then(|v| v.as_str())
        && !v.trim().is_empty()
    {
        s.server = v.trim().to_string();
    }
    if let Some(v) = doc.get("server_fp").and_then(|v| v.as_str()) {
        s.server_fp = v.to_string();
    }
    if let Some(v) = doc.get("autostart").and_then(|v| v.as_bool()) {
        s.autostart = v;
    }
    if let Some(t) = doc.get("code").and_then(|v| v.as_table_like()) {
        let code = t.get("code").and_then(|v| v.as_integer());
        let secret = t.get("secret").and_then(|v| v.as_str());
        if let (Some(code), Some(secret)) = (code, secret) {
            s.saved_code = Some((code, secret.to_string()));
        }
    }
    Ok(s)
}

/// 连接中转服务器用的配置：服务器地址与指纹，加上本机名称（被控端和控制端共用）
pub fn link() -> Arc<LinkConfig> {
    static LINK: LazyLock<Arc<LinkConfig>> = LazyLock::new(|| {
        let s = settings();
        let (name, os) = crate::sysinfo::host_identity();
        Arc::new(
            LinkConfig::new(s.server.clone(), &s.server_fp, name.clone(), os.clone())
                .unwrap_or_else(|e| {
                    // 指纹写错了：不能因此就不校验证书。用一个不可能对得上的指纹，连接时报"证书指纹不匹配"
                    error!("config.toml: invalid server_fp ({e:#}), refusing to connect");
                    LinkConfig {
                        server: s.server.clone(),
                        pin: Some([0; 32]),
                        name,
                        os,
                    }
                }),
        )
    });
    LINK.clone()
}

/// 是否校验服务器证书（控制台据此提示"未固定证书"）
pub fn pinned() -> bool {
    link().pinned()
}

// ───────────────────────────── 写配置 ─────────────────────────────

/// 新建配置文件时的内容（按系统语言写注释）
fn template() -> String {
    let fp = if DEFAULT_SERVER_FP.is_empty() {
        tr!(
            "（未设置，不校验证书）",
            "(none, the certificate isn't checked)"
        )
    } else {
        DEFAULT_SERVER_FP.to_string()
    };
    let server = DEFAULT_SERVER;
    tr!(
        r#"# Airhand 配置文件。修改后要重新启动 Airhand 才生效（托盘菜单"退出"，然后再打开 Airhand）。
# 这个文件由 Airhand 维护（保存控制码、开机自动启动），手动修改的内容和注释都会保留。

# 中转服务器地址 host:port（UDP）。不设置时使用编译进程序的地址：{server}
# server = "relay.example.com:4433"

# 中转服务器证书的 SHA-256 指纹（服务器日志里打印的 64 位十六进制，可以用 : 或空格分隔）。
# 设为 "" 表示不校验证书，只适合开发环境。不设置时使用编译进程序的指纹：{fp}
# server_fp = ""

# 开机自动启动：开机后被控端服务自动运行（登录之前也能被控制），登录后显示托盘图标。托盘菜单里也可以切换
autostart = true

# 控制码：第一次连上服务器时由服务器分配，之后一直使用同一个（重启、掉线、升级都不变）。
# 托盘菜单"更换控制码"会换一个新的；删掉 code 和 secret 则下次启动时重新分配。
# secret 是取回这个控制码的凭据，用 Windows DPAPI 加密过，只有这台电脑上的 Airhand 能解开
[code]
"#,
        r#"# Airhand configuration. Restart Airhand after editing it (tray menu "Quit", then open Airhand again).
# Airhand maintains this file (it saves the control code and the autostart setting); your edits and comments are kept.

# Relay server address host:port (UDP). Without it the address built into the program is used: {server}
# server = "relay.example.com:4433"

# SHA-256 fingerprint of the relay server certificate (the 64 hex digits printed in the server log, `:` and spaces are allowed).
# "" means the certificate isn't checked: for development only. Without it the built-in fingerprint is used: {fp}
# server_fp = ""

# Start with Windows: the host service runs after boot (the computer can be controlled before anyone signs in),
# and the tray icon shows up after signing in. Can also be changed in the tray menu
autostart = true

# Control code: assigned by the server the first time, then always the same (across restarts, disconnects and upgrades).
# "Change control code" in the tray menu gets a new one; delete code and secret to get a new one at the next start.
# secret proves to the server that this computer owns the code. It's encrypted with Windows DPAPI, only Airhand on this computer can read it
[code]
"#
    )
}

/// 修改配置文件（不存在时按模板新建）。没有配置文件的平台什么也不做
fn edit(f: impl FnOnce(&mut DocumentMut) -> Result<()>) -> Result<()> {
    let Some(path) = path() else {
        return Ok(());
    };
    let mut doc = match std::fs::read_to_string(&path) {
        // 用户写坏了的文件不覆盖：控制码这次就不保存了
        Ok(text) => text.parse::<DocumentMut>().map_err(|e| {
            anyhow::anyhow!("{} isn't valid TOML, leaving it alone: {e}", path.display())
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => template().parse()?,
        Err(e) => return Err(e.into()),
    };
    f(&mut doc)?;
    write(&path, &doc.to_string())
}

/// 先写临时文件再改名：写到一半断电也不会把配置文件弄坏
fn write(path: &std::path::Path, text: &str) -> Result<()> {
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// 配置文件不存在时按模板新建（安装时调用）
#[cfg_attr(not(windows), allow(dead_code))]
pub fn ensure_file() -> Result<()> {
    match path() {
        Some(p) if !p.exists() => edit(|_| Ok(())),
        _ => Ok(()),
    }
}

/// 保存服务器分配的控制码；`None` 表示放弃了原来的控制码（下次启动时重新分配）
pub fn save_code(code: Option<Reclaim>) -> Result<()> {
    edit(|doc| {
        let table = doc
            .entry("code")
            .or_insert(toml_edit::table())
            .as_table_mut()
            .ok_or_else(|| anyhow::anyhow!("`code` in config.toml must be a table"))?;
        match code {
            Some(r) => {
                table["code"] = toml_edit::value(i64::from(r.code));
                table["secret"] = toml_edit::value(secret::seal(&r.secret)?);
            }
            None => {
                table.remove("code");
                table.remove("secret");
            }
        }
        Ok(())
    })
}

/// 保存开机自动启动的设置
#[cfg_attr(not(windows), allow(dead_code))]
pub fn save_autostart(on: bool) -> Result<()> {
    edit(|doc| {
        doc["autostart"] = toml_edit::value(on);
        Ok(())
    })
}

/// 取回控制码用的密钥：Windows 上用 DPAPI 加密后再保存（只有同一个账户——安装版是 SYSTEM——能解开），
/// 配置文件被别人读到、拷走也没用。
mod secret {
    use anyhow::{Result, bail};
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;

    #[cfg(windows)]
    const ENTROPY: &[u8] = b"airhand control code secret v1";

    pub fn seal(secret: &[u8; 16]) -> Result<String> {
        #[cfg(windows)]
        {
            Ok(STANDARD.encode(dpapi(secret, true)?))
        }
        #[cfg(not(windows))]
        {
            Ok(STANDARD.encode(secret))
        }
    }

    pub fn open(text: &str) -> Result<[u8; 16]> {
        let raw = STANDARD.decode(text.trim())?;
        #[cfg(windows)]
        let raw = dpapi(&raw, false)?;
        match <[u8; 16]>::try_from(raw.as_slice()) {
            Ok(s) => Ok(s),
            Err(_) => bail!("the secret has the wrong length"),
        }
    }

    #[cfg(windows)]
    fn dpapi(data: &[u8], protect: bool) -> Result<Vec<u8>> {
        use anyhow::Context;
        use windows::Win32::Foundation::{HLOCAL, LocalFree};
        use windows::Win32::Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        };

        let input = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let entropy = CRYPT_INTEGER_BLOB {
            cbData: ENTROPY.len() as u32,
            pbData: ENTROPY.as_ptr() as *mut u8,
        };
        let mut out = CRYPT_INTEGER_BLOB::default();
        unsafe {
            if protect {
                CryptProtectData(
                    &input,
                    windows::core::w!("Airhand"),
                    Some(&entropy),
                    None,
                    None,
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut out,
                )
                .context("CryptProtectData")?;
            } else {
                CryptUnprotectData(
                    &input,
                    None,
                    Some(&entropy),
                    None,
                    None,
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut out,
                )
                .context("CryptUnprotectData (was config.toml copied from another computer?)")?;
            }
            let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
            LocalFree(Some(HLOCAL(out.pbData.cast())));
            Ok(bytes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_check_matches_the_parser() {
        let hex = "ab".repeat(32);
        for s in [
            String::new(),
            hex.clone(),
            ["AB"; 32].join(":"),
            format!(" {hex}\n"),
            "ab".repeat(31),
            format!("sha256: {hex}"),
            "é".into(),
        ] {
            assert_eq!(
                super::fingerprint_is_valid(&s),
                airhand_link::net::parse_fingerprint(&s).is_ok(),
                "{s:?}"
            );
        }
    }

    /// 模板就是默认配置；保存控制码、开关自动启动之后能读回来，用户的修改和注释都还在
    #[test]
    fn template_and_edits_roundtrip() {
        let mut text = template();
        assert_eq!(parse(&text).unwrap(), Settings::default());

        // 用户改了服务器地址，加了自己的注释
        text = text.replacen(
            "# server = \"relay.example.com:4433\"",
            "# 我的服务器\nserver = \"relay.example.com:4433\"",
            1,
        );
        let edit = |text: &str, f: &dyn Fn(&mut DocumentMut) -> Result<()>| {
            let mut doc: DocumentMut = text.parse().unwrap();
            f(&mut doc).unwrap();
            doc.to_string()
        };
        let code = Reclaim {
            code: 123_456,
            secret: [7; 16],
        };
        text = edit(&text, &|doc| {
            let t = doc["code"].as_table_mut().unwrap();
            t["code"] = toml_edit::value(i64::from(code.code));
            t["secret"] = toml_edit::value(secret::seal(&code.secret)?);
            doc["autostart"] = toml_edit::value(false);
            Ok(())
        });
        let s = parse(&text).unwrap();
        assert_eq!(s.server, "relay.example.com:4433");
        assert_eq!(s.code(), Some(code));
        assert!(!s.autostart);
        assert!(text.contains("# 我的服务器"));

        // 放弃控制码：表还在（注释也在），只是没有内容
        text = edit(&text, &|doc| {
            let t = doc["code"].as_table_mut().unwrap();
            t.remove("code");
            t.remove("secret");
            Ok(())
        });
        assert_eq!(parse(&text).unwrap().code(), None);
        assert!(text.contains("[code]"));

        // 别的账户 / 别的电脑加密的密钥打不开：当作没有控制码
        let foreign = "[code]\ncode = 123456\nsecret = \"AAAA\"\n";
        assert_eq!(parse(foreign).unwrap().code(), None);
    }
}
