fn main() {
    // Media Foundation 延迟加载：Windows N/KN 版（未装媒体功能包）没有 mfplat.dll，
    // 延迟加载后程序仍能启动，并自动回退到软件编码。
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("windows-msvc") {
        println!("cargo:rustc-link-arg=/DELAYLOAD:mfplat.dll");
        println!("cargo:rustc-link-lib=delayimp");
    }
    tauri_build::build()
}
