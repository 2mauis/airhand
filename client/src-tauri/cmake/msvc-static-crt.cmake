# 由 .cargo/config.toml 通过 CMAKE_TOOLCHAIN_FILE_<target> 引用，作用于用 CMake 构建的 C 依赖（libopus）。
#
# Rust 端开启了 crt-static（静态 VC 运行时），用 cc 构建的 C 依赖（如 OpenH264）会自动跟随，
# 但 CMake 默认按 /MD（动态运行时）编译。这里让 CMake 构建的库也使用 /MT，
# 与其余代码保持一致，不依赖链接器对 CRT 混用的容错。
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded" CACHE STRING "" FORCE)
# libopus 的 CMakeLists 会根据这个选项覆盖 CMAKE_MSVC_RUNTIME_LIBRARY
set(OPUS_STATIC_RUNTIME ON CACHE BOOL "" FORCE)
