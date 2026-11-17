/* os: native nn::fs.
 *
 * The SDK's file system client (nn::fs::ipc::FileSystem, FsFile, Directory)
 * builds an IPC command in the thread's TLS and sends it to fs:USER with
 * svcSendSyncRequest. These overrides replace each of those stubs with a
 * direct call into the native PC platform layer (zakuro-core pcos::fs),
 * which reads the game's data from data/romfs as plain files and keeps
 * saves under userdata/ as plain files, with no SVC, no command buffer and
 * no emulated service in between.
 *
 * Everything above these stubs (nn::fs::FileInputStream, the RomFS archive
 * code, the game's own save code) is the game's own recompiled code, which
 * does no I/O of its own.
 *
 * The bridge: the host sets os_native_hook at startup (source/src/main.rs).
 * A call returns nonzero when the native layer handled it, with the result
 * in r0 and outputs written to guest memory; zero hands the call back to
 * the original stub (IPC), which happens for handles the native layer does
 * not know and when FATES_NATIVE_OS=0.
 */
#include "overrides.h"

/* the bridge into the host, see zakuro-core/src/pcos/bridge.rs */
typedef int (*OsNativeHook)(Context *ctx, uint32_t op);
RECOMP_EXPORT OsNativeHook os_native_hook = 0;
/* the version of the call numbers below, which the host checks */
RECOMP_EXPORT const uint32_t os_native_abi = 1;

#define OS_NATIVE(op, address) do { \
    if (os_native_hook && os_native_hook(ctx, (op))) RETURN_TO(ctx->r[14]); \
    CALL(RECOMP_ORIGINAL(address)); \
} while (0)

/* call numbers, the same as pcos::bridge::op */
enum {
    OS_FS_INITIALIZE = 0x100,
    OS_FS_OPEN_FILE,
    OS_FS_OPEN_FILE_DIRECTLY,
    OS_FS_OPEN_ARCHIVE,
    OS_FS_CLOSE_ARCHIVE,
    OS_FS_CONTROL_ARCHIVE,
    OS_FS_FORMAT_SAVE_DATA,
    OS_FS_IS_SDMC_DETECTED,
    OS_FS_IS_SDMC_WRITABLE,
    OS_FS_GET_PRIORITY,
    OS_FS_SET_PRIORITY,
    OS_FS_CREATE_FILE,
    OS_FS_DELETE_FILE,
    OS_FS_RENAME_FILE,
    OS_FS_OPEN_DIRECTORY,
    OS_FS_CREATE_EXT_SAVE_DATA,
    OS_FS_DELETE_EXT_SAVE_DATA,
    OS_FS_GET_ARCHIVE_RESOURCE,
    OS_FILE_SET_PRIORITY = 0x120,
    OS_FILE_OPEN_LINK,
    OS_FILE_READ,
    OS_FILE_CLOSE,
    OS_FILE_WRITE,
    OS_FILE_GET_SIZE,
    OS_FILE_SET_SIZE,
    OS_DIRECTORY_READ = 0x130,
    OS_DIRECTORY_CLOSE,
};

/* nn::fs::ipc::FileSystem */
RECOMP_OVERRIDE(0x001163D8) { OS_NATIVE(OS_FS_INITIALIZE, 0x001163D8); }          /* InitializeWithSdkVersion */
RECOMP_OVERRIDE(0x002300F4) { OS_NATIVE(OS_FS_OPEN_FILE, 0x002300F4); }           /* OpenFile */
RECOMP_OVERRIDE(0x0012009C) { OS_NATIVE(OS_FS_OPEN_FILE_DIRECTLY, 0x0012009C); }  /* OpenFileDirectly */
RECOMP_OVERRIDE(0x00116284) { OS_NATIVE(OS_FS_OPEN_ARCHIVE, 0x00116284); }        /* OpenArchive */
RECOMP_OVERRIDE(0x001162E4) { OS_NATIVE(OS_FS_CLOSE_ARCHIVE, 0x001162E4); }       /* CloseArchive */
RECOMP_OVERRIDE(0x00116314) { OS_NATIVE(OS_FS_CONTROL_ARCHIVE, 0x00116314); }     /* ControlArchive (commit) */
RECOMP_OVERRIDE(0x00116380) { OS_NATIVE(OS_FS_FORMAT_SAVE_DATA, 0x00116380); }    /* FormatSaveData */
RECOMP_OVERRIDE(0x0012AB88) { OS_NATIVE(OS_FS_IS_SDMC_DETECTED, 0x0012AB88); }    /* IsSdmcDetected */
RECOMP_OVERRIDE(0x0022FFD4) { OS_NATIVE(OS_FS_IS_SDMC_WRITABLE, 0x0022FFD4); }    /* IsSdmcWritable */
RECOMP_OVERRIDE(0x00133964) { OS_NATIVE(OS_FS_GET_PRIORITY, 0x00133964); }        /* GetPriority */
RECOMP_OVERRIDE(0x0010CA90) { OS_NATIVE(OS_FS_SET_PRIORITY, 0x0010CA90); }        /* SetPriority */
RECOMP_OVERRIDE(0x0022FE34) { OS_NATIVE(OS_FS_CREATE_FILE, 0x0022FE34); }         /* CreateFile */
RECOMP_OVERRIDE(0x0022FE90) { OS_NATIVE(OS_FS_DELETE_FILE, 0x0022FE90); }         /* DeleteFile */
RECOMP_OVERRIDE(0x0022FEE0) { OS_NATIVE(OS_FS_RENAME_FILE, 0x0022FEE0); }         /* RenameFile */
RECOMP_OVERRIDE(0x0022FF70) { OS_NATIVE(OS_FS_OPEN_DIRECTORY, 0x0022FF70); }      /* OpenDirectory */
RECOMP_OVERRIDE(0x0023000C) { OS_NATIVE(OS_FS_CREATE_EXT_SAVE_DATA, 0x0023000C); }/* CreateExtSaveData */
RECOMP_OVERRIDE(0x00230068) { OS_NATIVE(OS_FS_DELETE_EXT_SAVE_DATA, 0x00230068); }/* DeleteExtSaveData */
RECOMP_OVERRIDE(0x002300A4) { OS_NATIVE(OS_FS_GET_ARCHIVE_RESOURCE, 0x002300A4); }/* GetArchiveResource */

/* nn::fs::ipc::FsFile */
RECOMP_OVERRIDE(0x00230168) { OS_NATIVE(OS_FILE_SET_PRIORITY, 0x00230168); }
RECOMP_OVERRIDE(0x00230198) { OS_NATIVE(OS_FILE_OPEN_LINK, 0x00230198); }
RECOMP_OVERRIDE(0x002301D0) { OS_NATIVE(OS_FILE_READ, 0x002301D0); }
RECOMP_OVERRIDE(0x00230228) { OS_NATIVE(OS_FILE_CLOSE, 0x00230228); }
RECOMP_OVERRIDE(0x00230254) { OS_NATIVE(OS_FILE_WRITE, 0x00230254); }
RECOMP_OVERRIDE(0x002302B4) { OS_NATIVE(OS_FILE_GET_SIZE, 0x002302B4); }
RECOMP_OVERRIDE(0x002302EC) { OS_NATIVE(OS_FILE_SET_SIZE, 0x002302EC); }

/* nn::fs::ipc::Directory */
RECOMP_OVERRIDE(0x0023031C) { OS_NATIVE(OS_DIRECTORY_READ, 0x0023031C); }
RECOMP_OVERRIDE(0x00230370) { OS_NATIVE(OS_DIRECTORY_CLOSE, 0x00230370); }
