//! 编译期常量配置。
//!
//! 客户端**没有任何配置文件**，服务器地址直接编译进二进制。
//! 修改下面的默认值，或在构建时通过环境变量覆盖：
//!
//! ```powershell
//! $env:AIRHAND_SERVER="relay.example.com:4433"
//! $env:AIRHAND_SERVER_FP="<服务器日志中打印的证书 SHA-256 指纹>"
//! npm run tauri build
//! ```

/// 中转服务器地址 `host:port`（UDP）。
pub const SERVER_ADDR: &str = match option_env!("AIRHAND_SERVER") {
    Some(v) => v,
    None => "127.0.0.1:4433",
};

/// 服务器证书 SHA-256 指纹（64 位十六进制，允许包含 `:`）。
///
/// * 非空：严格校验，指纹不符直接拒绝连接（防中间人）。
/// * 为空：不校验证书，**仅建议在开发环境使用**。
pub const SERVER_CERT_SHA256: &str = match option_env!("AIRHAND_SERVER_FP") {
    Some(v) => v,
    None => "",
};

/// 产品版本
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
