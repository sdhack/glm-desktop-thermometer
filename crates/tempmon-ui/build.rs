fn main() {
    // 仅 Windows 目标：把 tempmon.ico 嵌入 exe 资源（资源管理器/任务栏图标）
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        if let Err(e) = winresource::WindowsResource::new()
            .set_icon("tempmon.ico")
            .compile()
        {
            eprintln!("winresource: {e}");
        }
    }
}
