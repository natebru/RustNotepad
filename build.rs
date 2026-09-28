#[path = "src/version.rs"]
mod version;

fn main() {
    println!("cargo:rerun-if-changed=resources/app.manifest");
    println!("cargo:rerun-if-changed=resources/app.ico");
    println!("cargo:rerun-if-changed=src/version.rs");
    println!("cargo:rerun-if-env-changed=RUSTNOTEPAD_VERSION");
    let version = std::env::var("RUSTNOTEPAD_VERSION")
        .unwrap_or_else(|_| std::env::var("CARGO_PKG_VERSION").unwrap());
    let components = version::windows_version(&version).expect("Invalid release version");
    println!("cargo:rustc-env=RUSTNOTEPAD_VERSION={version}");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let architecture =
            version::manifest_architecture(&std::env::var("CARGO_CFG_TARGET_ARCH").unwrap())
                .expect("Unsupported release architecture");
        let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
        let manifest = out.join("app.manifest");
        let numeric = components.map(|n| n.to_string());
        let template =
            std::fs::read_to_string("resources/app.manifest").expect("Read manifest template");
        std::fs::write(
            &manifest,
            template
                .replace("@VERSION@", &numeric.join("."))
                .replace("@ARCH@", architecture),
        )
        .expect("Write target-specific manifest");
        println!("cargo:rustc-link-arg-bin=notepad=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bin=notepad=/MANIFESTINPUT:{}",
            manifest.display()
        );
        let rc = out.join("version.rc");
        let res = out.join("version.res");
        let icon = std::fs::canonicalize("resources/app.ico")
            .expect("Read app icon")
            .display()
            .to_string()
            .replace('\\', "\\\\");
        let resource = format!(
            r#"
#include <winver.h>
1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEFLAGSMASK VS_FFI_FILEFLAGSMASK
FILEFLAGS 0
FILEOS VOS_NT_WINDOWS32
FILETYPE VFT_APP
FILESUBTYPE 0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "RustNotepad contributors\0"
      VALUE "FileDescription", "Rust Notepad\0"
      VALUE "FileVersion", "{version}\0"
      VALUE "InternalName", "notepad\0"
      VALUE "OriginalFilename", "notepad.exe\0"
      VALUE "ProductName", "RustNotepad\0"
      VALUE "ProductVersion", "{version}\0"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
            numeric = numeric.join(",")
        );
        std::fs::write(&rc, resource).expect("Write version resource");
        let output = std::process::Command::new("rc.exe").arg("/nologo").arg("/fo").arg(&res).arg(&rc)
            .output().expect("Run Windows SDK rc.exe (use scripts\\build.ps1 or a Visual Studio developer shell)");
        assert!(
            output.status.success(),
            "Compile version resource:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        println!("cargo:rustc-link-arg-bin=notepad={}", res.display());
    }
}
