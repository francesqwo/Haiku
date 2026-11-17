//! os: native nn::fs, what overrides/os_fs.c calls in place of the SDK's
//! fs:USER IPC stubs.
//!
//! Files are host files: the RomFS is the installed data/romfs folder
//! (through the image built from it, which the SDK's own RomFS reader
//! walks), saves and extra data are folders under userdata/. Open files,
//! directories and archives live in the same tables the emulated fs:USER
//! uses (services::fs::FsState), so a handle opened either way works the
//! other way too, and saved states keep working. File and directory
//! handles are still kernel handles (a session object each), because the
//! SDK closes them with svcCloseHandle; that is the one SVC left per file.
//!
//! Unlike the emulated service, a native read doesn't put the thread to
//! sleep for the time a game card would take: it returns when the host
//! read does.

use zakuro_common::result::{errors, ResultCode};
use zakuro_cpu::Bus;

use super::bridge::{op, Call};
use super::stats;
use crate::kernel::object::KObject;
use crate::services::fs::{self as svc, FileBacking, NcchArchivePath, OpenArchive, OpenDirectory};
use crate::services::host_archive::{self, FormatInfo, HostArchive};
use crate::System;

/// the file or directory id behind a kernel handle to one of fs's sessions.
fn session(system: &mut System, handle: u32, service: &str) -> Option<u32> {
    let object = system.kernel.resolve(handle)?;
    match system.kernel.objects.get(object)? {
        KObject::ClientSession(session) if session.service == service => Some(session.subhandle),
        _ => None,
    }
}

fn result(value: Result<(), ResultCode>) -> u32 {
    value.err().map_or(0, |code| code.0)
}

/// runs a native fs call, false when it is one to leave to IPC.
pub fn call(system: &mut System, call: &mut Call, number: u32) -> bool {
    match number {
        op::FS_INITIALIZE..=op::FS_GET_ARCHIVE_RESOURCE => {
            let name = file_system(system, call, number);
            stats::native(name);
            note_error(call, name);
            true
        }
        op::FILE_SET_PRIORITY..=op::FILE_SET_SIZE => {
            let this = call.arg(system, 0);
            let handle = system.memory.read32(this);
            let Some(id) = session(system, handle, "FSFile") else { return false };
            let name = file(system, call, number, id);
            stats::native(name);
            note_error(call, name);
            true
        }
        op::DIRECTORY_READ | op::DIRECTORY_CLOSE => {
            let this = call.arg(system, 0);
            let handle = system.memory.read32(this);
            let Some(id) = session(system, handle, "FSDirectory") else { return false };
            let name = directory(system, call, number, id);
            stats::native(name);
            note_error(call, name);
            true
        }
        _ => false,
    }
}

/// savehang: a failed call, kept for pcos::hang's dump.
fn note_error(call: &Call, name: &str) {
    let result = call.ctx.r[0];
    if result & 0x8000_0000 != 0 {
        super::hang::note_fs_error(name, "", result);
    }
}

/// nn::fs::ipc::FileSystem, the archive-level calls. the name to count it
/// under back.
fn file_system(system: &mut System, call: &mut Call, number: u32) -> &'static str {
    match number {
        // InitializeWithSdkVersion(this, version)
        op::FS_INITIALIZE => {
            call.ret(0);
            "fs.Initialize"
        }
        // OpenFile(this, Handle* out, transaction, u64 archive, path type,
        //          path, path size, flags, attributes)
        op::FS_OPEN_FILE => {
            let out = call.arg(system, 1);
            let archive = call.arg64(system, 4);
            let (path_type, path, path_size, flags) =
                (call.arg(system, 6), call.arg(system, 7), call.arg(system, 8), call.arg(system, 9));
            let open = system.services.fs.archives.get(&archive).cloned();
            let opened = match open {
                Some(OpenArchive { host: Some(host), .. }) => {
                    super::hang::note_save_open(); // savehang
                    svc::open_host_file(system, &host, path_type, path_size, path, flags)
                }
                Some(open) => svc::open_file(system, open.id, open.ncch_path, path_type, path_size, path).ok_or(errors::FS_NOT_FOUND),
                None => Err(errors::FS_ARCHIVE_NOT_MOUNTED),
            };
            finish_open(system, call, out, opened);
            "fs.OpenFile"
        }
        // OpenFileDirectly(this, Handle* out, transaction, archive id,
        //   archive path type, archive path, archive path size, file path
        //   type, file path, file path size, flags, attributes)
        op::FS_OPEN_FILE_DIRECTLY => {
            let out = call.arg(system, 1);
            let archive_id = call.arg(system, 3);
            let (archive_path, archive_path_size) = (call.arg(system, 5), call.arg(system, 6));
            let (file_type, file_path, file_size, flags) =
                (call.arg(system, 7), call.arg(system, 8), call.arg(system, 9), call.arg(system, 10));
            let archive_bytes = svc::path_bytes(system, archive_path_size, archive_path);
            let opened = if svc::host_archive_for(system, archive_id, &archive_bytes).is_some() {
                svc::open_host_archive(system, archive_id, &archive_bytes)
                    .and_then(|host| svc::open_host_file(system, &host, file_type, file_size, file_path, flags))
            } else {
                let ncch = if archive_id == svc::ARCHIVE_NCCH { NcchArchivePath::decode(system, archive_path_size, archive_path) } else { None };
                svc::open_file(system, archive_id, ncch, file_type, file_size, file_path).ok_or(errors::FS_NOT_FOUND)
            };
            finish_open(system, call, out, opened);
            "fs.OpenFileDirectly"
        }
        // OpenArchive(this, u64* out, archive id, path type, path, path size)
        op::FS_OPEN_ARCHIVE => {
            let out = call.arg(system, 1);
            let archive_id = call.arg(system, 2);
            let (path, path_size) = (call.arg(system, 4), call.arg(system, 5));
            let bytes = svc::path_bytes(system, path_size, path);
            let host = if svc::host_archive_for(system, archive_id, &bytes).is_some() {
                match svc::open_host_archive(system, archive_id, &bytes) {
                    Ok(host) => Some(host),
                    Err(code) => {
                        log::debug!("os: fs archive 0x{archive_id:08X} {bytes:02X?} -> {code:?}");
                        call.ret(code.0);
                        return "fs.OpenArchive";
                    }
                }
            } else {
                None
            };
            let ncch_path = if archive_id == svc::ARCHIVE_NCCH { NcchArchivePath::decode(system, path_size, path) } else { None };
            let handle = system.services.fs.add_archive(OpenArchive { id: archive_id, ncch_path, host });
            log::debug!("os: fs opened archive 0x{archive_id:08X} -> {handle}");
            system.memory.write32(out, handle as u32);
            system.memory.write32(out + 4, (handle >> 32) as u32);
            call.ret(0);
            "fs.OpenArchive"
        }
        // CloseArchive(this, u64 archive)
        op::FS_CLOSE_ARCHIVE => {
            let archive = call.arg64(system, 2);
            system.services.fs.archives.remove(&archive);
            call.ret(0);
            "fs.CloseArchive"
        }
        // ControlArchive(this, u64 archive, action, ...): the action games
        // use commits save data, which host files are on every write
        op::FS_CONTROL_ARCHIVE => {
            call.ret(0);
            "fs.ControlArchive"
        }
        // FormatSaveData(this, archive id, path type, path, path size,
        //   blocks, directories, files, directory buckets, file buckets,
        //   duplicate data)
        op::FS_FORMAT_SAVE_DATA => {
            let archive_id = call.arg(system, 1);
            let (path, path_size) = (call.arg(system, 3), call.arg(system, 4));
            let info = FormatInfo {
                total_size: call.arg(system, 5).wrapping_mul(512),
                directories: call.arg(system, 6),
                files: call.arg(system, 7),
                duplicate_data: call.arg(system, 10) & 0xFF != 0,
            };
            let bytes = svc::path_bytes(system, path_size, path);
            let formatted = match svc::host_archive_for(system, archive_id, &bytes) {
                Some(archive) if archive_id == svc::ARCHIVE_SAVEDATA => archive.format(info),
                _ => Err(errors::FS_INVALID_PATH),
            };
            log::info!("os: fs format save data 0x{archive_id:08X} {info:?} -> {formatted:?}");
            call.ret(result(formatted));
            "fs.FormatSaveData"
        }
        // IsSdmcDetected / IsSdmcWritable(this, bool* out): the SD card is
        // a folder, always there
        op::FS_IS_SDMC_DETECTED | op::FS_IS_SDMC_WRITABLE => {
            let out = call.arg(system, 1);
            system.memory.write8(out, 1);
            call.ret(0);
            "fs.IsSdmc"
        }
        op::FS_GET_PRIORITY => {
            let out = call.arg(system, 1);
            let priority = system.services.fs.priority;
            system.memory.write32(out, priority);
            call.ret(0);
            "fs.GetPriority"
        }
        op::FS_SET_PRIORITY => {
            system.services.fs.priority = call.arg(system, 1);
            call.ret(0);
            "fs.SetPriority"
        }
        // CreateFile(this, transaction, u64 archive, path type, path, path
        //   size, attributes, s64 size)
        op::FS_CREATE_FILE => {
            let archive = call.arg64(system, 2);
            let (path_type, path, path_size) = (call.arg(system, 4), call.arg(system, 5), call.arg(system, 6));
            let size = call.arg64(system, 8);
            let path = svc::decode_path(system, path_type, path_size, path);
            let created = svc::archive_of(system, archive).and_then(|host| host.create_file(&path, size));
            log::debug!("os: fs create '{path}' 0x{size:X} bytes -> {created:?}");
            call.ret(result(created));
            "fs.CreateFile"
        }
        // DeleteFile(this, transaction, u64 archive, path type, path, size)
        op::FS_DELETE_FILE => {
            let archive = call.arg64(system, 2);
            let (path_type, path, path_size) = (call.arg(system, 4), call.arg(system, 5), call.arg(system, 6));
            let path = svc::decode_path(system, path_type, path_size, path);
            let deleted = svc::archive_of(system, archive).and_then(|host| host.delete_file(&path));
            log::debug!("os: fs delete '{path}' -> {deleted:?}");
            call.ret(result(deleted));
            "fs.DeleteFile"
        }
        // RenameFile(this, transaction, u64 source archive, type, path,
        //   size, u64 destination archive, type, path, size)
        op::FS_RENAME_FILE => {
            let archive = call.arg64(system, 2);
            let (source_type, source, source_size) = (call.arg(system, 4), call.arg(system, 5), call.arg(system, 6));
            let (dest_type, dest, dest_size) = (call.arg(system, 10), call.arg(system, 11), call.arg(system, 12));
            let source = svc::decode_path(system, source_type, source_size, source);
            let dest = svc::decode_path(system, dest_type, dest_size, dest);
            let renamed = svc::archive_of(system, archive).and_then(|host| host.rename_file(&source, &dest));
            log::debug!("os: fs rename '{source}' -> '{dest}' -> {renamed:?}");
            call.ret(result(renamed));
            "fs.RenameFile"
        }
        // OpenDirectory(this, Handle* out, u64 archive, path type, path, size)
        op::FS_OPEN_DIRECTORY => {
            let out = call.arg(system, 1);
            let archive = call.arg64(system, 2);
            let (path_type, path, path_size) = (call.arg(system, 4), call.arg(system, 5), call.arg(system, 6));
            let path = svc::decode_path(system, path_type, path_size, path);
            match svc::archive_of(system, archive).and_then(|host| host.list(&path)) {
                Ok(entries) => {
                    let id = system.services.fs.add_directory(OpenDirectory { entries, cursor: 0 });
                    let handle = svc::make_session(system, "FSDirectory", id);
                    system.memory.write32(out, handle);
                    call.ret(0);
                }
                Err(code) => call.ret(code.0),
            }
            "fs.OpenDirectory"
        }
        // CreateExtSaveData(this, const spec*, directories, files, s64
        //   size limit, icon, icon size)
        op::FS_CREATE_EXT_SAVE_DATA => {
            let spec = call.arg(system, 1);
            let path = svc::path_bytes(system, 12, spec);
            let info = FormatInfo { total_size: 0, directories: call.arg(system, 2), files: call.arg(system, 3), duplicate_data: false };
            let created = match svc::host_archive_for(system, svc::ARCHIVE_EXTDATA, &path) {
                Some(archive) if archive.exists() => Ok(()),
                Some(archive) => archive.format(info),
                None => Err(errors::FS_INVALID_PATH),
            };
            log::info!("os: fs create extra data {path:02X?} -> {created:?}");
            call.ret(result(created));
            "fs.CreateExtSaveData"
        }
        op::FS_DELETE_EXT_SAVE_DATA => {
            let spec = call.arg(system, 1);
            let path = svc::path_bytes(system, 12, spec);
            let deleted = svc::host_archive_for(system, svc::ARCHIVE_EXTDATA, &path).ok_or(errors::FS_INVALID_PATH).and_then(|archive| archive.delete());
            call.ret(result(deleted));
            "fs.DeleteExtSaveData"
        }
        // GetArchiveResource(this, ArchiveResource* out, media): sector
        // size, cluster size, total and free clusters, plenty
        op::FS_GET_ARCHIVE_RESOURCE => {
            let out = call.arg(system, 1);
            for (i, word) in [0x200u32, 0x4000, 0x8_0000, 0x8_0000].into_iter().enumerate() {
                system.memory.write32(out + 4 * i as u32, word);
            }
            call.ret(0);
            "fs.GetArchiveResource"
        }
        _ => {
            call.ret(errors::FS_NOT_FOUND.0);
            "fs.unknown"
        }
    }
}

/// a file just opened goes to *out as a handle.
fn finish_open(system: &mut System, call: &mut Call, out: u32, opened: Result<u32, ResultCode>) {
    match opened {
        Ok(id) => {
            let handle = svc::make_session(system, "FSFile", id);
            system.memory.write32(out, handle);
            call.ret(0);
        }
        Err(code) => call.ret(code.0),
    }
}

/// nn::fs::ipc::FsFile, on the file with the id.
fn file(system: &mut System, call: &mut Call, number: u32, id: u32) -> &'static str {
    let backing = system.services.fs.files.get(&id).map(|file| file.backing.clone());
    match number {
        op::FILE_SET_PRIORITY => {
            call.ret(0);
            "file.SetPriority"
        }
        // OpenLinkFile(this, Handle* out), another handle on the file
        op::FILE_OPEN_LINK => {
            let out = call.arg(system, 1);
            let handle = svc::make_session(system, "FSFile", id);
            system.memory.write32(out, handle);
            call.ret(0);
            "file.OpenLinkFile"
        }
        // Read(this, int* out, s64 offset, void* buffer, size)
        op::FILE_READ => {
            let out = call.arg(system, 1);
            let offset = call.arg64(system, 2);
            let (buffer, size) = (call.arg(system, 4), call.arg(system, 5));
            let read = svc::read_file(system, id, offset, size, buffer);
            system.memory.write32(out, read);
            // console timing: as long as the medium takes, after the reads
            // it is still busy with, as the emulated FSFile read waits
            let medium = matches!(backing, Some(FileBacking::Host(_))) as usize;
            // SAFETY: the context is the run's, see bridge::os_native
            let tick = unsafe { crate::recompiled::ticks_of(&mut *call.ctx) };
            let busy = &mut system.services.fs.busy_until[medium];
            *busy = (*busy).max(tick) + crate::kernel::thread::nanos_to_ticks(svc::read_delay(read));
            call.wait = Some(*busy - tick);
            call.ret(0);
            "file.Read"
        }
        // Write(this, int* out, s64 offset, const void* buffer, size, option)
        op::FILE_WRITE => {
            let out = call.arg(system, 1);
            let offset = call.arg64(system, 2);
            let (buffer, size) = (call.arg(system, 4), call.arg(system, 5));
            let written = match backing {
                Some(FileBacking::Host(path)) => {
                    let mut data = vec![0u8; size as usize];
                    system.memory.read_bytes(buffer, &mut data);
                    host_archive::write_at(&path, offset, &data) as u32
                }
                _ => 0,
            };
            system.memory.write32(out, written);
            call.ret(0);
            "file.Write"
        }
        // GetSize(this, s64* out)
        op::FILE_GET_SIZE => {
            let out = call.arg(system, 1);
            let size = backing.map_or(0, |backing| backing.size());
            system.memory.write32(out, size as u32);
            system.memory.write32(out + 4, (size >> 32) as u32);
            call.ret(0);
            "file.GetSize"
        }
        // SetSize(this, s64 size)
        op::FILE_SET_SIZE => {
            let size = call.arg64(system, 2);
            let set = match backing {
                Some(FileBacking::Host(path)) => host_archive::set_size(&path, size),
                _ => Ok(()),
            };
            call.ret(result(set));
            "file.SetSize"
        }
        // Close(this); the handle is closed by the caller
        op::FILE_CLOSE => {
            system.services.fs.files.remove(&id);
            call.ret(0);
            "file.Close"
        }
        _ => {
            call.ret(0);
            "file.unknown"
        }
    }
}

/// nn::fs::ipc::Directory, on the directory with the id.
fn directory(system: &mut System, call: &mut Call, number: u32, id: u32) -> &'static str {
    match number {
        // Read(this, int* out, DirectoryEntry* entries, count)
        op::DIRECTORY_READ => {
            let (out, dest, count) = (call.arg(system, 1), call.arg(system, 2), call.arg(system, 3));
            let entries = match system.services.fs.directories.get_mut(&id) {
                Some(directory) => {
                    let end = (directory.cursor + count as usize).min(directory.entries.len());
                    let batch = directory.entries[directory.cursor..end].to_vec();
                    directory.cursor = end;
                    batch
                }
                None => Vec::new(),
            };
            for (i, entry) in entries.iter().enumerate() {
                let raw = svc::encode_directory_entry(entry);
                system.write_from_service(dest + i as u32 * svc::DIRECTORY_ENTRY_SIZE, &raw);
            }
            system.memory.write32(out, entries.len() as u32);
            call.ret(0);
            "directory.Read"
        }
        _ => {
            system.services.fs.directories.remove(&id);
            call.ret(0);
            "directory.Close"
        }
    }
}


/// the path under user_dir that path, a save folder or a file in one under
/// some other user folder, stands for: from its first `savedata`, `extdata`,
/// `nand` or `sdmc` component on. None when it is not one.
fn rebase(path: &std::path::Path, user_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let parts: Vec<_> = path.components().collect();
    let at = parts.iter().position(|part| matches!(part.as_os_str().to_str(), Some("savedata" | "extdata" | "nand" | "sdmc")))?;
    let mut out = user_dir.to_path_buf();
    out.extend(parts[at..].iter());
    Some(out)
}

/// points the open archives and files of a state that was loaded at this
/// run's user folder, see state::load_state.
pub fn rebase_host_paths(system: &mut System) {
    let user_dir = system.services.fs.user_dir.clone();
    let mut moved = 0;
    let mut made = 0;
    for archive in system.services.fs.archives.values_mut() {
        if let Some(host) = &mut archive.host {
            // hooks: what the old folder was formatted with, if it is there
            let info = host.format_info();
            if let Some(root) = rebase(host.root(), &user_dir) {
                if root != host.root() {
                    *host = HostArchive::at(root);
                    moved += 1;
                }
            }
            // hooks: the state has the archive open, so the title formatted
            // it before the state was saved; a user folder without it (a
            // fresh one) would fail every access ("Unable to access save
            // data" after loading a state taken while the save was being
            // made). It is made again, empty, as the format left it.
            if !host.exists() && host.format(info.unwrap_or_default()).is_ok() {
                made += 1;
            }
        }
    }
    if made > 0 {
        log::info!("os: {made} save archives the loaded state has open were missing from the user folder, made empty");
    }
    for file in system.services.fs.files.values_mut() {
        if let FileBacking::Host(path) = &mut file.backing {
            if let Some(new) = rebase(path, &user_dir) {
                if &new != path {
                    *path = new;
                    moved += 1;
                }
            }
        }
    }
    if moved > 0 {
        log::info!("os: {moved} save archives and files of the loaded state now use {}", user_dir.display());
    }
}
