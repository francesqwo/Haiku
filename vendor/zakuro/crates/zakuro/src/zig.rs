//! Zig's C compiler, zig cc, downloaded into Zakuro's own folder when the
//! computer has no C compiler and the player says yes, so that recompiling
//! a game needs nothing installed by hand.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use recomp3ds::compile::{no_window, Compiler}; // nowindow: no_window

/// the release Zakuro downloads.
const VERSION: &str = "0.17.0";

/// the list of mirrors the Zig project asks tools to download from, before
/// ziglang.org itself.
const MIRRORS: &str = "https://ziglang.org/download/community-mirrors.txt";

/// a release's archive for one system, as ziglang.org/download/index.json
/// gives it.
struct Archive {
    name: &'static str,
    size: u64,
    sha256: &'static str,
}

/// the archive for the system Zakuro runs on, none where Zig has none.
fn archive() -> Option<Archive> {
    let (name, size, sha256) = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => ("zig-x86_64-linux-0.17.0.tar.xz", 57_332_648, "1cbe9df9f27e6b78d14ccbca43b6703a404ef79ef1c463de901d7f088d4e2026"),
        ("linux", "aarch64") => ("zig-aarch64-linux-0.17.0.tar.xz", 52_877_280, "9e8d11661d4ae3bd57702a3832781e23ad151dde5798e16a5ccd503f65234ff8"),
        ("windows", "x86_64") => ("zig-x86_64-windows-0.17.0.zip", 100_268_137, "b5663f69581dcf391293fbf16c06cb80d81d806545ce618b4d0bab7f0eb8c428"),
        ("windows", "aarch64") => ("zig-aarch64-windows-0.17.0.zip", 95_992_853, "0a59d91fa1cb40cf068e9b0954434ce973500c7a2ea749f1e01af62cdab52d26"),
        ("macos", "x86_64") => ("zig-x86_64-macos-0.17.0.tar.xz", 59_317_572, "4f9a1c5269aa17ebda5e6d3c2b89d6cbf36f7d2b22a0306e9ab98f25f95529c6"),
        ("macos", "aarch64") => ("zig-aarch64-macos-0.17.0.tar.xz", 53_985_220, "b607e9b9234790a008116ae5bdb71c6243b84b9fb42a53a9e70fde41c06c536a"),
        _ => return None,
    };
    Some(Archive { name, size, sha256 })
}

/// how many bytes the download is, none when there is nothing to download
/// for this system.
pub fn download_size() -> Option<u64> {
    archive().map(|archive| archive.size)
}

/// where Zakuro keeps the tools it downloads: the local application data on
/// Windows, which does not roam with the user, and the data folder
/// elsewhere.
pub fn tools_dir(data_dir: Option<&Path>) -> Option<PathBuf> {
    if cfg!(windows) {
        let local = std::env::var_os("LOCALAPPDATA").filter(|dir| !dir.is_empty())?;
        Some(PathBuf::from(local).join("zakuro").join("tools"))
    } else {
        data_dir.map(|dir| dir.join("tools"))
    }
}

fn folder(tools: &Path) -> PathBuf {
    tools.join(format!("zig-{VERSION}"))
}

/// Zig's program in its folder.
const PROGRAM: &str = if cfg!(windows) { "zig.exe" } else { "zig" };

/// zigdl: the file in Zig's folder while it is being unpacked; a folder
/// with it is a download that didn't finish.
const UNPACKING: &str = ".unpacking";

/// the compiler, when it has been downloaded into tools.
pub fn installed(tools: &Path) -> Option<Compiler> {
    let program = folder(tools).join(PROGRAM);
    // zigdl: not one being unpacked, or whose unpacking stopped midway
    (program.is_file() && !folder(tools).join(UNPACKING).exists()).then(|| Compiler::new(program, &["cc"]))
}

/// the program downloads go through, curl, which Linux, macOS and Windows
/// 10 and later mostly have, or else wget, which some Linux systems have
/// instead.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Fetcher {
    Curl,
    Wget,
}

impl Fetcher {
    fn find() -> Option<Fetcher> {
        let runs = |program: &str| {
            no_window(&mut Command::new(program)).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|status| status.success())
        };
        [Fetcher::Curl, Fetcher::Wget].into_iter().find(|fetcher| runs(fetcher.program()))
    }

    fn program(self) -> &'static str {
        match self {
            Fetcher::Curl => "curl",
            Fetcher::Wget => "wget",
        }
    }

    /// a command downloading url to path, or to its output without one,
    /// giving up after seconds.
    fn command(self, url: &str, path: Option<&Path>, seconds: u32) -> Command {
        let mut command = Command::new(self.program());
        no_window(&mut command); // nowindow: no console window from Haiku
        match self {
            Fetcher::Curl => {
                command.args(["-fL", "--silent", "--show-error", "--connect-timeout", "20", "--max-time", &seconds.to_string()]);
                if let Some(path) = path {
                    command.arg("-o").arg(path);
                }
            }
            // wget has no limit on the whole download, a stall ends it
            Fetcher::Wget => {
                command.args(["--quiet", "--timeout=20", &format!("--read-timeout={}", seconds.min(60)), "-O"]);
                match path {
                    Some(path) => command.arg(path),
                    None => command.arg("-"),
                };
            }
        }
        command.arg(url);
        command
    }
}

/// whether Zakuro can download Zig here, having curl or wget to.
pub fn can_download() -> bool {
    Fetcher::find().is_some()
}

/// downloads Zig into tools and unpacks it, telling progress how much of the
/// archive has arrived, 0 to 1. the mirrors the Zig project lists go first,
/// in a random order, and ziglang.org last, as it asks of tools, and what
/// arrives has to match the SHA-256 above whichever served it.
pub fn download(tools: &Path, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<Compiler, String> {
    download_then(tools, progress, &|| {}, cancel)
}

/// zigdl: download, telling unpacking when the archive is all there and
/// checked and the unpacking (a minute or more) starts, so a progress bar
/// doesn't sit at 100% meanwhile.
pub fn download_then(tools: &Path, progress: &dyn Fn(f32), unpacking: &dyn Fn(), cancel: &AtomicBool) -> Result<Compiler, String> {
    let mut archive = archive().ok_or("Zakuro has no compiler to download for this system")?;
    let fetcher = Fetcher::find().ok_or("Zakuro downloads with curl or wget, and this computer has neither")?;
    let sources = match test_source() {
        // zigdl: a test's stand-in server, in a build with debug assertions
        Some((source, size, sha256)) => {
            log::warn!("zigdl: HAIKU_ZIG_TEST: Zig from {source} instead ({size} bytes)");
            (archive.size, archive.sha256) = (size, sha256);
            vec![source]
        }
        None => {
            let mut sources = mirrors(fetcher);
            sources.push(format!("https://ziglang.org/download/{VERSION}"));
            sources
        }
    };
    from_sources(fetcher, tools, &archive, sources, progress, unpacking, cancel)
}

/// downloads archive from the first of sources that has it whole, and
/// unpacks it into tools.
fn from_sources(fetcher: Fetcher, tools: &Path, archive: &Archive, sources: Vec<String>, progress: &dyn Fn(f32), unpacking: &dyn Fn(), cancel: &AtomicBool) -> Result<Compiler, String> {
    std::fs::create_dir_all(tools).map_err(|error| format!("could not create {}, {error}", tools.display()))?;
    let partial = tools.join(format!("{}.part", archive.name));
    let mut last = String::new();
    for source in sources {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let url = format!("{}/{}", source.trim_end_matches('/'), archive.name);
        log::info!("downloading Zig from {url}");
        match fetch(fetcher, &url, &partial, archive.size, progress, cancel).and_then(|()| verify(&partial, archive)) {
            Ok(()) => {
                log::info!("zigdl: Zig arrived and its SHA-256 matches; unpacking it");
                unpacking();
                let started = std::time::Instant::now();
                let unpacked = unpack(&partial, tools);
                let _ = std::fs::remove_file(&partial);
                unpacked?;
                log::info!("zigdl: Zig unpacked into {} in {:.1} s", folder(tools).display(), started.elapsed().as_secs_f32());
                // an antivirus can take the program away once it is there
                return installed(tools).ok_or_else(|| format!("it was unpacked, but {} is gone from its folder (an antivirus may have removed it)", PROGRAM));
            }
            Err(error) => {
                log::warn!("Zig from {url}: {error}");
                last = error;
            }
        }
    }
    let _ = std::fs::remove_file(&partial);
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".to_owned());
    }
    Err(format!("could not download Zig, from ziglang.org or any of its mirrors, the last said {last}"))
}

/// zigdl: where a test serves its stand-in for Zig's archive, from
/// HAIKU_ZIG_TEST="<base url> <size> <sha256>" (the archive keeps its
/// name). Only a build with debug assertions reads it: a release or ship
/// build never downloads from anywhere but the Zig project's mirrors.
fn test_source() -> Option<(String, u64, &'static str)> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let value = std::env::var("HAIKU_ZIG_TEST").ok()?;
    let mut words = value.split_whitespace();
    let (source, size, sha256) = (words.next()?.to_owned(), words.next()?.parse().ok()?, words.next()?);
    Some((source, size, Box::leak(sha256.to_ascii_lowercase().into_boxed_str())))
}

/// the mirrors the Zig project lists, in a random order, none when the list
/// can't be had.
fn mirrors(fetcher: Fetcher) -> Vec<String> {
    let listed = fetcher.command(MIRRORS, None, 30).stderr(Stdio::null()).output();
    let mut mirrors: Vec<String> = match listed {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with("https://"))
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    };
    // a shuffle seeded by the clock, so that the downloads spread over them
    let mut seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |time| time.as_nanos() as u64) | 1;
    for i in (1..mirrors.len()).rev() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        mirrors.swap(i, (seed % (i as u64 + 1)) as usize);
    }
    mirrors
}

/// downloads url to path, following how much of size has arrived.
fn fetch(fetcher: Fetcher, url: &str, path: &Path, size: u64, progress: &dyn Fn(f32), cancel: &AtomicBool) -> Result<(), String> {
    let _ = std::fs::remove_file(path);
    let mut child = fetcher
        .command(url, Some(path), 1800)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("{} could not start, {error}", fetcher.program()))?;
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("cancelled".to_owned());
        }
        let arrived = std::fs::metadata(path).map_or(0, |metadata| metadata.len());
        progress((arrived as f32 / size as f32).min(1.0));
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => {
                let mut error = String::new();
                if let Some(mut stderr) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut stderr, &mut error);
                }
                return Err(error.trim().to_owned());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(250)),
            Err(error) => return Err(error.to_string()),
        }
    }
}

/// whether the archive at path is the one Zakuro knows, by its size and its
/// SHA-256.
fn verify(path: &Path, archive: &Archive) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let size = file.metadata().map_err(|error| error.to_string())?.len();
    if size != archive.size {
        return Err(format!("{size} bytes arrived, not {}", archive.size));
    }
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(|error| error.to_string())?;
    let hash: String = hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect();
    if hash != archive.sha256 {
        return Err(format!("its SHA-256 is {hash}, not the one Zakuro knows"));
    }
    Ok(())
}

/// unpacks the archive into its folder in tools with tar, which reads the
/// zip on Windows too. the archive holds a single folder, whose contents go
/// straight into zig-<version> (tar's --strip-components).
///
/// zigdl: it used to be unpacked into a folder beside it and that folder
/// renamed into place, but Windows refuses to rename a folder while any
/// file in it is open, by anyone, and an antivirus, the search indexer or
/// OneDrive open the new files as tar writes them (zig.exe, 190 MB, takes
/// a while to scan): the rename then failed with "Access is denied (os
/// error 5)" after a minute of unpacking (GitHub issue #8). Nothing is
/// renamed now; the .unpacking file in the folder says it isn't finished,
/// and is deleted last.
fn unpack(archive: &Path, tools: &Path) -> Result<(), String> {
    let target = folder(tools);
    // what an earlier download left: a half-unpacked folder, or the
    // staging folder the renaming way used
    let _ = std::fs::remove_dir_all(&target);
    let _ = std::fs::remove_dir_all(tools.join(format!("zig-{VERSION}.unpacking")));
    std::fs::create_dir_all(&target).map_err(|error| format!("its folder {} can't be made, {error}", target.display()))?;
    let unpacking = target.join(UNPACKING);
    std::fs::write(&unpacking, b"").map_err(|error| format!("{} can't be written, {error}", unpacking.display()))?;
    let xz = archive.to_string_lossy().ends_with(".tar.xz.part");
    // its messages are read: started without a console window (nowindow),
    // what it says would go nowhere
    let output = no_window(&mut Command::new("tar"))
        .arg(if xz { "-xJf" } else { "-xf" })
        .arg(archive)
        .arg("--strip-components=1")
        .arg("-C")
        .arg(&target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("tar could not start, {error}"))?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        for line in said.lines() {
            log::warn!("zigdl: tar: {line}");
        }
        let first = said.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("no message");
        return Err(format!("it couldn't be unpacked ({}; tar said: {first})", output.status));
    }
    if !target.join(PROGRAM).is_file() {
        return Err(format!("it was unpacked, but {PROGRAM} isn't in it"));
    }
    // last: the folder is whole. if this fails the next start downloads again
    std::fs::remove_file(&unpacking).map_err(|error| format!("{} can't be deleted, {error}", unpacking.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_archive_is_named_for_the_release() {
        let archive = archive().expect("a Zig for the system the tests run on");
        assert!(archive.name.contains(VERSION));
        assert_eq!(archive.sha256.len(), 64);
    }

    #[test]
    fn a_downloaded_zig_is_called_as_zig_cc() {
        let tools = std::env::temp_dir().join(format!("zakuro-zig-{}", std::process::id()));
        assert!(installed(&tools).is_none());
        let program = folder(&tools).join(if cfg!(windows) { "zig.exe" } else { "zig" });
        std::fs::create_dir_all(program.parent().unwrap()).unwrap();
        std::fs::write(&program, b"").unwrap();
        assert_eq!(installed(&tools), Some(Compiler::new(&program, &["cc"])));
        std::fs::remove_dir_all(tools).unwrap();
    }

    #[test]
    fn wget_writes_where_curl_would() {
        let args = |command: Command| command.get_args().map(|arg| arg.to_string_lossy().into_owned()).collect::<Vec<_>>();
        let curl = args(Fetcher::Curl.command("https://x/z", Some(Path::new("/t/z.part")), 1800));
        assert!(curl.windows(2).any(|pair| pair == ["-o", "/t/z.part"]) && curl.last().map(String::as_str) == Some("https://x/z"));
        let wget = args(Fetcher::Wget.command("https://x/z", Some(Path::new("/t/z.part")), 1800));
        assert!(wget.windows(2).any(|pair| pair == ["-O", "/t/z.part"]) && wget.last().map(String::as_str) == Some("https://x/z"));
        assert!(args(Fetcher::Wget.command("https://x/list", None, 30)).windows(2).any(|pair| pair == ["-O", "-"]));
    }

    /// zigdl: an archive laid out as Zig's (one folder holding the program
    /// and a lib folder), made with the tar the download unpacks with, and
    /// its size and SHA-256.
    fn stand_in(dir: &Path) -> (PathBuf, Archive) {
        use sha2::{Digest, Sha256};
        let top = dir.join("src").join("zig-x86_64-test-0.17.0");
        std::fs::create_dir_all(top.join("lib").join("std")).unwrap();
        std::fs::write(top.join(PROGRAM), b"a stand-in for zig").unwrap();
        std::fs::write(top.join("lib").join("std").join("std.zig"), b"// std").unwrap();
        let name = if cfg!(windows) { "zig-x86_64-test-0.17.0.zip" } else { "zig-x86_64-test-0.17.0.tar.xz" };
        let path = dir.join(name);
        let made = Command::new("tar")
            .arg(if cfg!(windows) { "-acf" } else { "-cJf" })
            .arg(&path)
            .arg("-C")
            .arg(dir.join("src"))
            .arg("zig-x86_64-test-0.17.0")
            .status()
            .unwrap();
        assert!(made.success());
        let bytes = std::fs::read(&path).unwrap();
        let sha256: String = Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect();
        (path, Archive { name: Box::leak(name.to_owned().into_boxed_str()), size: bytes.len() as u64, sha256: Box::leak(sha256.into_boxed_str()) })
    }

    /// zigdl: serves file at /<its name> on 127.0.0.1 to the next few
    /// requests, a 404 for anything else; the base URL.
    fn serve(file: PathBuf) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let name = format!("/{}", file.file_name().unwrap().to_string_lossy());
        std::thread::spawn(move || {
            for stream in listener.incoming().take(8) {
                let Ok(mut stream) = stream else { continue };
                let mut request = Vec::new();
                let mut byte = [0u8; 1];
                while !request.ends_with(b"\r\n\r\n") && stream.read(&mut byte).is_ok_and(|n| n == 1) {
                    request.push(byte[0]);
                }
                let line = String::from_utf8_lossy(&request).lines().next().unwrap_or("").to_owned();
                let body = if line.split_whitespace().nth(1) == Some(name.as_str()) { std::fs::read(&file).ok() } else { None };
                let _ = match body {
                    Some(body) => stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).and_then(|()| stream.write_all(&body)),
                    None => stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
                };
            }
        });
        base
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zakuro-zigdl-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// zigdl: the whole download, from a server on 127.0.0.1 (never the
    /// internet), with the curl or wget and the tar the player has: a source
    /// without it is passed over, the archive is checked and unpacked into
    /// zig-<version>, and nothing else is left in tools.
    #[test]
    fn zig_is_downloaded_checked_and_unpacked_into_its_folder() {
        let Some(fetcher) = Fetcher::find() else { return };
        let dir = scratch("download");
        let (path, archive) = stand_in(&dir);
        let base = serve(path);
        let tools = dir.join("tools");
        let unpacked = std::sync::atomic::AtomicBool::new(false);
        let never = AtomicBool::new(false);
        let sources = vec![format!("{base}/missing"), base];
        let compiler = from_sources(fetcher, &tools, &archive, sources, &|_| {}, &|| unpacked.store(true, Ordering::Relaxed), &never).unwrap();
        assert_eq!(compiler, Compiler::new(folder(&tools).join(PROGRAM), &["cc"]));
        assert!(unpacked.load(Ordering::Relaxed));
        assert!(folder(&tools).join("lib").join("std").join("std.zig").is_file());
        let left: Vec<String> = std::fs::read_dir(&tools).unwrap().map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left, [format!("zig-{VERSION}")]);
        assert_eq!(installed(&tools), Some(compiler));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// zigdl, issue #8: something keeping the new zig open as it is
    /// unpacked (an antivirus scanning it, OneDrive, the indexer) made the
    /// rename into place fail with "Access is denied" on Windows. Nothing is
    /// renamed now, so it doesn't matter.
    #[test]
    fn a_program_holding_the_new_zig_open_does_not_stop_the_install() {
        let Some(fetcher) = Fetcher::find() else { return };
        let dir = scratch("held");
        let (path, archive) = stand_in(&dir);
        let base = serve(path);
        let tools = dir.join("tools");
        let done = std::sync::Arc::new(AtomicBool::new(false));
        // a scanner: opens zig as soon as it is there, sharing everything as
        // scanners do, and keeps it open until the download is over
        let scanner = {
            let (tools, done) = (tools.clone(), done.clone());
            std::thread::spawn(move || {
                let mut held = Vec::new();
                while !done.load(Ordering::Relaxed) {
                    for program in [folder(&tools).join(PROGRAM), tools.join(format!("zig-{VERSION}.unpacking")).join("zig-x86_64-test-0.17.0").join(PROGRAM)] {
                        if held.is_empty() {
                            if let Ok(file) = std::fs::File::open(&program) {
                                held.push(file);
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                !held.is_empty()
            })
        };
        let never = AtomicBool::new(false);
        let result = from_sources(fetcher, &tools, &archive, vec![base], &|_| {}, &|| {}, &never);
        done.store(true, Ordering::Relaxed);
        let held = scanner.join().unwrap();
        assert!(result.is_ok(), "{result:?}");
        assert!(held, "the scanner never caught zig");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// zigdl: a folder whose unpacking didn't finish isn't a compiler, and
    /// tar's own words say why it couldn't unpack.
    #[test]
    fn a_half_unpacked_zig_is_not_installed_and_tar_says_why_it_failed() {
        let dir = scratch("half");
        let tools = dir.join("tools");
        std::fs::create_dir_all(folder(&tools)).unwrap();
        std::fs::write(folder(&tools).join(PROGRAM), b"").unwrap();
        std::fs::write(folder(&tools).join(UNPACKING), b"").unwrap();
        assert!(installed(&tools).is_none());
        let broken = dir.join(if cfg!(windows) { "broken.zip.part" } else { "broken.tar.xz.part" });
        std::fs::write(&broken, b"not an archive at all").unwrap();
        let error = unpack(&broken, &tools).unwrap_err();
        assert!(error.contains("couldn't be unpacked") && error.contains("tar said:"), "{error}");
        assert!(installed(&tools).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_archive_of_the_wrong_size_or_hash_is_turned_down() {
        let path = std::env::temp_dir().join(format!("zakuro-zig-archive-{}", std::process::id()));
        std::fs::write(&path, b"abc").unwrap();
        let wrong_size = Archive { name: "x", size: 4, sha256: "" };
        assert!(verify(&path, &wrong_size).unwrap_err().contains("3 bytes arrived"));
        let wrong_hash = Archive { name: "x", size: 3, sha256: "00" };
        assert!(verify(&path, &wrong_hash).unwrap_err().contains("SHA-256"));
        let right = Archive { name: "x", size: 3, sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad" };
        assert!(verify(&path, &right).is_ok());
        std::fs::remove_file(path).unwrap();
    }
}
