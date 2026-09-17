use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=driver/lhm-bridge.sys");
    // 驱动文件必须与 exe 同目录（ring0 运行期按 current_exe 找它）。
    // OUT_DIR = <target>/<profile>/build/<pkg>-<hash>/out，上跳三级即 profile 目录。
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap_or_default());
    let Some(profile_dir) = out.ancestors().nth(3).map(|p| p.to_path_buf()) else {
        return;
    };
    let src = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default())
        .join("driver/lhm-bridge.sys");
    if src.exists() {
        let _ = std::fs::copy(&src, profile_dir.join("lhm-bridge.sys"));
    }
}
