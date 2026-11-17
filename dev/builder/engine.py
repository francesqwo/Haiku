"""builder: turns cargo's link of the ship Haiku.exe into Haiku Setup's
prebuilt engine: every input of the link except the recompiled game code.

  python -I dev/builder/engine.py <cargo log with --print link-args> <engine dir>

The log is `cargo rustc --profile ship --bin Haiku -- --print link-args
-C save-temps` (dev/builder/package.sh). Its linker line names ~820 Rust
objects (thin LTO's codegen units: all of Haiku's Rust and its crates), a
few rlibs (import stubs), the resource object (icon, version) and system
libraries, and `-lrecomp` with `-L <dir of librecomp.a>`. This copies the
inputs into <engine>/obj, <engine>/rlib and <engine>/lib, and writes
<engine>/link.txt, one argument per line, paths relative to <engine>, the
librecomp folder as @RECOMP@ and the output as @OUT@. Haiku Setup runs the
same gcc with them (pipeline.rs `link`), so its Haiku.exe is linked exactly
as cargo links the developers'.
"""
import re
import shutil
import sys
from pathlib import Path


def main():
    log, engine = Path(sys.argv[1]), Path(sys.argv[2])
    line = next(l for l in log.read_text(encoding="utf-8", errors="replace").splitlines() if l.startswith('"x86_64-w64-mingw32-gcc"') or l.startswith('"gcc"'))
    args = [a.replace("\\\\", "\\") for a in re.findall(r'"((?:[^"\\]|\\.)*)"', line)]
    if engine.exists():
        shutil.rmtree(engine)
    for sub in ("obj", "rlib", "lib"):
        (engine / sub).mkdir(parents=True)
    out = []
    objects = 0
    i = 1  # args[0] is the linker
    while i < len(args):
        a = args[i]
        if a == "-L":
            path = Path(args[i + 1])
            if (path / "librecomp.a").exists() or path.name == "code":
                out += ["-L", "@RECOMP@"]
            else:
                # an import library folder (windows-rs's libwindows.0.52.0.a)
                for lib in path.glob("*.a"):
                    shutil.copy2(lib, engine / "lib" / lib.name)
                out += ["-L", "lib"]
            i += 2
            continue
        if a == "-o":
            out += ["-o", "@OUT@"]
            i += 2
            continue
        if a.endswith(".o") and Path(a).is_file():
            name = "rc.o" if "haiku-rc" in Path(a).name else f"{objects:04d}.o"
            objects += a.endswith(".o") and "haiku-rc" not in Path(a).name
            target = engine / ("" if name == "rc.o" else "obj") / name
            shutil.copy2(a, target)
            out.append(target.relative_to(engine).as_posix())
        elif a.endswith(".rlib") and Path(a).is_file():
            target = engine / "rlib" / re.sub(r"-[0-9a-f]{16}", "", Path(a).name)
            shutil.copy2(a, target)
            out.append(target.relative_to(engine).as_posix())
        elif a.startswith("-Wl,-Map="):
            pass
        else:
            out.append(a)
        i += 1
    (engine / "link.txt").write_text("\n".join(out) + "\n", encoding="utf-8", newline="\n")
    size = sum(p.stat().st_size for p in engine.rglob("*") if p.is_file())
    print(f"engine: {objects} objects, {len(list((engine / 'rlib').iterdir()))} rlibs, {size / 1e6:.1f} MB")


if __name__ == "__main__":
    main()
