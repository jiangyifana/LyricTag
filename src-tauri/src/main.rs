// 发布构建隐藏控制台窗口。日志因此落到 %APPDATA%/LyricTag/logs/lyrictag.log。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    lyrictag_lib::run()
}
