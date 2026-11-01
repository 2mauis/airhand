//! 编译期配置。
//!
//! 客户端**没有任何配置文件**，服务器地址直接编译进二进制。
//! 修改下面的默认值，或在构建时通过环境变量覆盖：
//!
//! ```powershell
//! $env:AIRHAND_SERVER="relay.example.com:4433"
//! $env:AIRHAND_SERVER_FP="<服务器日志中打印的证书 SHA-256 指纹>"
//! npm run tauri build
//! ```

use std::sync::{Arc, LazyLock};

use airhand_link::LinkConfig;

/// 中转服务器地址 `host:port`（UDP）。
pub const SERVER_ADDR: &str = match option_env!("AIRHAND_SERVER") {
    Some(v) => v,
    None => "127.0.0.1:4433",
};

/// 服务器证书 SHA-256 指纹（64 位十六进制，允许用 `:` 或空白分隔）。
///
/// * 非空：严格校验，指纹不符直接拒绝连接（防中间人）。
/// * 为空：不校验证书，**仅建议在开发环境使用**。
pub const SERVER_CERT_SHA256: &str = match option_env!("AIRHAND_SERVER_FP") {
    Some(v) => v,
    None => "",
};

// 指纹写错了在编译时就报错，而不是等到运行时连不上服务器（规则与 airhand_link::net::parse_fingerprint 相同）
const _: () = assert!(
    fingerprint_is_valid(SERVER_CERT_SHA256),
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

/// 连接中转服务器用的配置：上面编译进程序的地址与指纹，加上本机名称（被控端和控制端共用）
pub fn link() -> Arc<LinkConfig> {
    static LINK: LazyLock<Arc<LinkConfig>> = LazyLock::new(|| {
        let (name, os) = crate::sysinfo::host_identity();
        Arc::new(
            LinkConfig::new(SERVER_ADDR, SERVER_CERT_SHA256, name, os)
                .expect("the fingerprint is checked at compile time"),
        )
    });
    LINK.clone()
}

#[cfg(test)]
mod tests {
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
}
