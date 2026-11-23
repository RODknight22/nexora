fn main() {
    let attributes = tauri_build::Attributes::new();

    #[cfg(windows)]
    let attributes = {
        embed_windows_manifest();
        attributes.windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest())
    };

    tauri_build::try_build(attributes).expect("failed to run tauri-build");
}

#[cfg(windows)]
fn embed_windows_manifest() {
    use std::{env, fs, path::PathBuf, process::Command};

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let manifest_path = manifest_dir.join("windows-app-manifest.xml");
    println!("cargo:rerun-if-changed={}", manifest_path.display());

    match env::var("CARGO_CFG_TARGET_ENV").as_deref() {
        Ok("msvc") => {
            println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
            println!(
                "cargo:rustc-link-arg=/MANIFESTINPUT:{}",
                manifest_path.display()
            );
        }
        Ok("gnu") => {
            let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
            let rc_path = out_dir.join("manifest.rc");
            let obj_path = out_dir.join("manifest.o");
            let escaped_manifest = manifest_path.display().to_string().replace('\\', "\\\\");
            fs::write(&rc_path, format!("1 24 \"{escaped_manifest}\"\n"))
                .expect("failed to write manifest.rc");

            let windres = if Command::new("x86_64-w64-mingw32-windres")
                .arg("--version")
                .output()
                .is_ok()
            {
                "x86_64-w64-mingw32-windres"
            } else {
                "windres"
            };

            let status = Command::new(windres)
                .args(["-i"])
                .arg(&rc_path)
                .args(["-o"])
                .arg(&obj_path)
                .args(["-O", "coff"])
                .status()
                .expect("failed to invoke windres");
            assert!(status.success(), "windres failed compiling manifest.rc");

            println!("cargo:rustc-link-arg={}", obj_path.display());
        }
        _ => {}
    }
}
