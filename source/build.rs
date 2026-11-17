use std::env::var;

#[path = "icon.rs"]
mod icon; // builder

fn main() {
    let dir = var("CARGO_MANIFEST_DIR").unwrap();
    let family = var("CARGO_CFG_TARGET_FAMILY").unwrap_or_default();
    let env = var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    // worktrees share the one prebuilt archive: FATES_RECOMP_DIR points at
    // the directory holding librecomp.a when it is not beside this file
    println!("cargo:rerun-if-env-changed=FATES_RECOMP_DIR");
    let code = var("FATES_RECOMP_DIR").unwrap_or_else(|_| format!("{dir}/code"));
    println!("cargo:rerun-if-changed={code}/librecomp.a");
    if env == "msvc" {
        // MSVC's linker looks for recomp.lib, the same archive by its name
        let out = var("OUT_DIR").unwrap();
        std::fs::copy(format!("{code}/librecomp.a"), format!("{out}/recomp.lib")).unwrap();
        println!("cargo:rustc-link-search=native={out}");
    } else {
        println!("cargo:rustc-link-search=native={code}");
    }
    println!("cargo:rustc-link-lib=static=recomp");
    // native-final: FATES_LINK_MAP=<file> writes the linker's map, which
    // lists every function linked into Haiku.exe (dev/native-final/linked.py)
    println!("cargo:rerun-if-env-changed=FATES_LINK_MAP");
    if let Ok(map) = var("FATES_LINK_MAP") {
        if env != "msvc" {
            println!("cargo:rustc-link-arg-bins=-Wl,-Map={map}");
        }
    }
    if family == "unix" {
        println!("cargo:rustc-link-lib=m");
    }
    // codemods: the overrides by address and file, for the Mods screen's
    // notes about a code mod changing a function Haiku has its own version of
    let overrides = format!("{dir}/../overrides");
    println!("cargo:rerun-if-changed={overrides}");
    let mut names = String::from("pub const OVERRIDE_NAMES: &[(u32, &str)] = &[\n");
    if let Ok(entries) = std::fs::read_dir(&overrides) {
        let mut paths: Vec<_> = entries.flatten().map(|entry| entry.path()).filter(|path| path.extension().is_some_and(|ext| ext == "c")).collect();
        paths.sort();
        for path in paths {
            let file = path.file_name().unwrap().to_string_lossy().into_owned();
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            for part in text.split("RECOMP_OVERRIDE(").skip(1) {
                let literal = part.split(')').next().unwrap_or("").trim();
                if let Some(hex) = literal.strip_prefix("0x").or_else(|| literal.strip_prefix("0X")) {
                    if let Ok(address) = u32::from_str_radix(hex, 16) {
                        names += &format!("    (0x{address:08X}, {file:?}),\n");
                    }
                }
            }
        }
    }
    names += "];\n";
    let out = var("OUT_DIR").unwrap();
    std::fs::write(format!("{out}/override_names.rs"), names).unwrap();
    // the code runs on the main thread, whose stack is 8 MiB on Linux and
    // 1 MiB on Windows unless it asks for more
    if family == "windows" {
        let stack = 8 << 20;
        match env.as_str() {
            "msvc" => println!("cargo:rustc-link-arg-bins=/STACK:{stack}"),
            _ => println!("cargo:rustc-link-arg-bins=-Wl,--stack,{stack}"),
        }
    }
    // the program's name, Haiku, in its version information (Explorer,
    // Task Manager, the taskbar), compiled with the toolchain's windres
    // builder: the exe's icon is made here from assets/icon/haiku.png
    // (icon.rs), the same picture main.rs gives the window
    println!("cargo:rerun-if-changed=haiku.rc");
    println!("cargo:rerun-if-changed=icon.rs");
    println!("cargo:rerun-if-changed=../assets/icon/haiku.png");
    if family == "windows" && env == "gnu" {
        let out = var("OUT_DIR").unwrap();
        let png = std::path::PathBuf::from(format!("{dir}/../assets/icon/haiku.png"));
        if let Err(error) = icon::write_ico(&png, std::path::Path::new(&format!("{out}/haiku.ico"))) {
            panic!("Haiku's icon: {error}");
        }
        let object = format!("{out}/haiku-rc.o");
        let windres = var("WINDRES").unwrap_or_else(|_| "windres".to_owned());
        let built = std::process::Command::new(&windres)
            .args([&format!("{dir}/haiku.rc"), "--include-dir", &out, "-O", "coff", "-o", &object])
            .status();
        match built {
            Ok(status) if status.success() => println!("cargo:rustc-link-arg-bins={object}"),
            Ok(status) => println!("cargo:warning=windres failed ({status}); Haiku.exe has no version information"),
            Err(error) => println!("cargo:warning=no windres ({error}); Haiku.exe has no version information"),
        }
    }
}
