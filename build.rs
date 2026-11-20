use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/afwshswc.rc");
    println!("cargo:rerun-if-changed=assets/bulb.ico");
    println!("cargo:rerun-if-env-changed=AFWSHSWC_RC");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let compiler = env::var_os("AFWSHSWC_RC").unwrap_or_else(|| {
        if cfg!(windows) {
            "rc.exe".into()
        } else {
            "llvm-rc".into()
        }
    });
    let resource =
        PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR")).join("afwshswc.res");
    let result = Command::new(&compiler)
        .args(["/nologo", "/fo"])
        .arg(&resource)
        .arg("assets/afwshswc.rc")
        .status()
        .expect("start the Windows resource compiler");
    assert!(result.success(), "Windows resource compilation failed");
    println!("cargo:rustc-link-arg-bin=AFWSHSWC={}", resource.display());
}
