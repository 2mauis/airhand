# Rust 核心按名字找 JNI 方法（Java_app_airhand_android_core_Native_*）
-keep class app.airhand.android.core.Native { *; }
-keepclasseswithmembernames class * {
    native <methods>;
}
