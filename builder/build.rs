//! Haiku Setup's icon (made from assets/icon/haiku.png, as Haiku.exe's is),
//! version information and manifest: no administrator rights (asInvoker),
//! sharp on high-DPI screens, long paths.

use std::env::var;

#[path = "../source/icon.rs"]
mod icon;

fn main() {
    let dir = var("CARGO_MANIFEST_DIR").unwrap();
    let out = var("OUT_DIR").unwrap();
    println!("cargo:rerun-if-changed=../source/icon.rs");
    println!("cargo:rerun-if-changed=../assets/icon/haiku.png");
    println!("cargo:rerun-if-changed=build.rs");
    if var("CARGO_CFG_TARGET_FAMILY").unwrap_or_default() != "windows" || var("CARGO_CFG_TARGET_ENV").unwrap_or_default() != "gnu" {
        return;
    }
    let png = std::path::PathBuf::from(format!("{dir}/../assets/icon/haiku.png"));
    icon::write_ico(&png, std::path::Path::new(&format!("{out}/haiku.ico"))).expect("Haiku's icon");
    let manifest = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="Haiku.Setup" version="0.1.0.0"/>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security><requestedPrivileges><requestedExecutionLevel level="asInvoker" uiAccess="false"/></requestedPrivileges></security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application><supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/></application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </windowsSettings>
  </application>
</assembly>
"#;
    std::fs::write(format!("{out}/setup.manifest"), manifest).unwrap();
    let rc = r#"#include <winver.h>
1 ICON "haiku.ico"
1 24 "setup.manifest"
VS_VERSION_INFO VERSIONINFO
FILEVERSION     0,1,0,0
PRODUCTVERSION  0,1,0,0
FILEOS          VOS_NT_WINDOWS32
FILETYPE        VFT_APP
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904B0"
        BEGIN
            VALUE "FileDescription",  "Haiku Setup"
            VALUE "ProductName",      "Haiku"
            VALUE "InternalName",     "Haiku Setup"
            VALUE "OriginalFilename", "Haiku Setup.exe"
            VALUE "FileVersion",      "0.1.0"
            VALUE "ProductVersion",   "0.1.0"
            VALUE "Comments",         "Builds Haiku from your own Fire Emblem Fates dump"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#;
    std::fs::write(format!("{out}/setup.rc"), rc).unwrap();
    let object = format!("{out}/setup-rc.o");
    let windres = var("WINDRES").unwrap_or_else(|_| "windres".to_owned());
    let status = std::process::Command::new(&windres)
        .args([&format!("{out}/setup.rc"), "--include-dir", &out, "-O", "coff", "-o", &object])
        .status()
        .expect("windres");
    assert!(status.success(), "windres failed");
    println!("cargo:rustc-link-arg-bins={object}");
}
