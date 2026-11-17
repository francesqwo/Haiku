"""builder: the Rust crates compiled into Haiku.exe's engine and into Haiku
Setup, with their licences, for the release's LICENSES/Rust-crates.txt.

  python -I dev/builder/crates.py <repo>                  the list
  python -I dev/builder/crates.py <repo> --texts          every crate's own
                                                          licence and notice files
(needs cargo on PATH, offline; only the crates built for Windows x64, GNU)
"""
import hashlib
import json
import subprocess
import sys
from pathlib import Path

TARGET = "x86_64-pc-windows-gnu"
OWN = ("haiku", "fates-gfx", "haiku-setup")


def packages(manifest):
    out = subprocess.run(["cargo", "metadata", "--offline", "--format-version", "1", "--filter-platform", TARGET, "--manifest-path", str(manifest)], capture_output=True, text=True, check=True).stdout
    meta = json.loads(out)
    # only what the resolve reaches for this target
    reached = {node["id"] for node in meta["resolve"]["nodes"]}
    return [p for p in meta["packages"] if p["id"] in reached]


def licence_files(package):
    folder = Path(package["manifest_path"]).parent
    names = []
    for f in sorted(folder.iterdir()):
        upper = f.name.upper()
        if f.is_file() and upper.startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE", "UNLICENSE", "COPYRIGHT")):
            names.append(f)
    if package.get("license_file"):
        extra = folder / package["license_file"]
        if extra.is_file() and extra not in names:
            names.append(extra)
    return names


def main():
    repo = Path(sys.argv[1])
    texts = "--texts" in sys.argv[2:]
    seen = {}
    for manifest in (repo / "source" / "Cargo.toml", repo / "builder" / "Cargo.toml"):
        for p in packages(manifest):
            seen[(p["name"], p["version"])] = p
    if not texts:
        print(r"Rust crates compiled into Haiku.exe (setup\engine) and Haiku Setup.exe")
        print("=======================================================================")
        print()
        print("Each crate's licence as its Cargo.toml states it; the licence and notice")
        print("files each crate ships are in Rust-crates-licences.txt. The Rust standard")
        print("library is under MIT OR Apache-2.0. Haiku's own crates (haiku, fates-gfx,")
        print("haiku-setup, MIT, Haiku-MIT.txt), Zakuro's (zakuro*, MIT, Zakuro-MIT.txt)")
        print("and 3dsrecomp's (recomp3ds, recomp-abi, MIT, 3dsrecomp-MIT.txt) are listed too.")
        print()
        for (name, version), p in sorted(seen.items()):
            lic = p.get("license") or ("see " + (p.get("license_file") or "the crate"))
            print(f"{name} {version}: {lic}")
        return
    print("Licence and notice files of the Rust crates in Haiku.exe and Haiku Setup.exe")
    print("=============================================================================")
    print()
    print("Each crate's own LICENSE, COPYING and NOTICE files, as it ships them in its")
    print("package. A text that is the same as an earlier crate's is named, not repeated.")
    print("Haiku's, Zakuro's and 3dsrecomp's crates: Haiku-MIT.txt, Zakuro-MIT.txt,")
    print("3dsrecomp-MIT.txt.")
    first = {}
    for (name, version), p in sorted(seen.items()):
        if name in OWN or name.startswith("zakuro") or name in ("recomp3ds", "recomp-abi"):
            continue
        print()
        print("-" * 78)
        print(f"{name} {version} ({p.get('license') or 'see its licence file'})")
        print("-" * 78)
        files = licence_files(p)
        if not files:
            print(f"(the crate's package holds no licence file; its licence is {p.get('license')}, whose standard text is given by another crate below or above)")
        for f in files:
            body = f.read_text(encoding="utf-8", errors="replace").replace("\r\n", "\n").strip()
            key = hashlib.sha256(body.encode()).hexdigest()
            if key in first:
                print(f"{f.name}: the same text as {first[key]}")
                continue
            first[key] = f"{f.name} of {name} {version}"
            print(f"{f.name}:")
            print()
            print(body)
            print()


if __name__ == "__main__":
    main()
