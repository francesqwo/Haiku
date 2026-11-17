/* os: native system services, see zakuro-core/src/pcos/sys.rs.
 *
 * Each override replaces an SDK function that makes an SVC or sends one IPC
 * request (svcGetSystemTick, cfg:u, APT, ndm:u, boss:U, nim:aoc) with a
 * direct call into the native platform layer, through the bridge that
 * overrides/os_fs.c defines.
 */
#include "overrides.h"

typedef int (*OsNativeHook)(Context *ctx, uint32_t op);
extern OsNativeHook os_native_hook;

#define OS_NATIVE(op, address) do { \
    if (os_native_hook && os_native_hook(ctx, (op))) RETURN_TO(ctx->r[14]); \
    CALL(RECOMP_ORIGINAL(address)); \
} while (0)

/* call numbers, the same as pcos::sys::op */
enum {
    OS_TICK = 0x200,
    OS_CFG_GET_CONFIG = 0x210,
    OS_CFG_GET_REGION = 0x211,
    OS_CFG_GET_TRANSFERABLE_ID = 0x212,
    OS_APPLET_GET_TARGET_PLATFORM = 0x220,
    OS_NDM_NO_OP = 0x230,
    OS_BOSS_INITIALIZE_SESSION = 0x240,
    OS_BOSS_RECEIVE_PROPERTY = 0x241,
    OS_NIM_AOC_INITIALIZE = 0x250,
    OS_AM_GET_NUM_CONTENT_INFOS = 0x260,
    OS_AM_LIST_CONTENT_INFOS = 0x261,
    OS_AM_GET_DATA_TITLE_INFOS = 0x262,
    OS_AM_LIST_DATA_TITLE_TICKET_INFOS = 0x263,
    OS_BOSS_NO_OP = 0x242,
    OS_SRV_GET_SERVICE_HANDLE = 0x270,
    OS_SRV_REGISTER_CLIENT = 0x271,
    OS_SRV_SUBSCRIBE = 0x272,
    OS_HID_GET_IPC_HANDLES = 0x280,
    OS_APPLET_NO_OP = 0x290,
    OS_APPLET_UTILITY = 0x291,
    OS_APPLET_SET_CPU_TIME_LIMIT = 0x292,
    OS_ERR_THROW_FATAL = 0x2A0, /* gate: */
};

/* time: svcGetSystemTick wrappers */
RECOMP_OVERRIDE(0x001334F0) { OS_NATIVE(OS_TICK, 0x001334F0); } /* nngxlowGetSystemTick, falls through into the next */
RECOMP_OVERRIDE(0x001334F4) { OS_NATIVE(OS_TICK, 0x001334F4); } /* .nn.svc.GetSystemTick */
RECOMP_OVERRIDE(0x0013C410) { OS_NATIVE(OS_TICK, 0x0013C410); } /* nnosTickGetSystemCurrent */

/* nn::cfg (cfg:u) */
RECOMP_OVERRIDE(0x00120900) { OS_NATIVE(OS_CFG_GET_CONFIG, 0x00120900); }          /* detail::GetConfig */
RECOMP_OVERRIDE(0x00120904) { OS_NATIVE(OS_CFG_GET_CONFIG, 0x00120904); }          /* detail::IpcUser::GetConfig */
RECOMP_OVERRIDE(0x00242470) { OS_NATIVE(OS_CFG_GET_REGION, 0x00242470); }          /* IpcUser::GetRegion */
RECOMP_OVERRIDE(0x0024242C) { OS_NATIVE(OS_CFG_GET_TRANSFERABLE_ID, 0x0024242C); } /* IpcUser::GetTransferableId */

/* nn::applet (APT). Initialize/Enable/GetLockHandle and the parameter
   calls stay on the emulated APT: the applet thread waits on its events. */
RECOMP_OVERRIDE(0x0030510C) { OS_NATIVE(OS_APPLET_GET_TARGET_PLATFORM, 0x0030510C); }
RECOMP_OVERRIDE(0x00123704) { OS_NATIVE(OS_APPLET_NO_OP, 0x00123704); }               /* NotifyToWait */
RECOMP_OVERRIDE(0x00107C3C) { OS_NATIVE(OS_APPLET_NO_OP, 0x00107C3C); }               /* SetScreenCapturePostPermission */
RECOMP_OVERRIDE(0x0012F344) { OS_NATIVE(OS_APPLET_UTILITY, 0x0012F344); }
RECOMP_OVERRIDE(0x0010EB98) { OS_NATIVE(OS_APPLET_SET_CPU_TIME_LIMIT, 0x0010EB98); }

/* nn::srv: sessions to services are made directly */
RECOMP_OVERRIDE(0x001225A4) { OS_NATIVE(OS_SRV_GET_SERVICE_HANDLE, 0x001225A4); }     /* nn::srv::GetServiceHandle */
RECOMP_OVERRIDE(0x001225D8) { OS_NATIVE(OS_SRV_GET_SERVICE_HANDLE, 0x001225D8); }     /* detail::Service::GetServiceHandle */
RECOMP_OVERRIDE(0x00107678) { OS_NATIVE(OS_SRV_REGISTER_CLIENT, 0x00107678); }
RECOMP_OVERRIDE(0x002F7B44) { OS_NATIVE(OS_SRV_SUBSCRIBE, 0x002F7B44); }

/* nn::hid */
RECOMP_OVERRIDE(0x00116F4C) { OS_NATIVE(OS_HID_GET_IPC_HANDLES, 0x00116F4C); }

/* nn::ndm: no network daemons on PC. The public wrappers are one
   instruction that falls through into the detail function, so both entries
   are replaced. */
RECOMP_OVERRIDE(0x00117068) { OS_NATIVE(OS_NDM_NO_OP, 0x00117068); } /* nn::ndm::SuspendDaemons */
RECOMP_OVERRIDE(0x001170A4) { OS_NATIVE(OS_NDM_NO_OP, 0x001170A4); } /* nn::ndm::OverrideDefaultDaemons */
RECOMP_OVERRIDE(0x0011706C) { OS_NATIVE(OS_NDM_NO_OP, 0x0011706C); } /* SuspendDaemons */
RECOMP_OVERRIDE(0x001170A8) { OS_NATIVE(OS_NDM_NO_OP, 0x001170A8); } /* OverrideDefaultDaemons */
RECOMP_OVERRIDE(0x00244B90) { OS_NATIVE(OS_NDM_NO_OP, 0x00244B90); } /* ResumeDaemons */
RECOMP_OVERRIDE(0x00244BCC) { OS_NATIVE(OS_NDM_NO_OP, 0x00244BCC); } /* EnterExclusiveState */
RECOMP_OVERRIDE(0x00134E04) { OS_NATIVE(OS_NDM_NO_OP, 0x00134E04); } /* LeaveExclusiveState */

/* nn::boss (SpotPass), offline */
RECOMP_OVERRIDE(0x0012E8FC) { OS_NATIVE(OS_BOSS_INITIALIZE_SESSION, 0x0012E8FC); }
RECOMP_OVERRIDE(0x00300150) { OS_NATIVE(OS_BOSS_RECEIVE_PROPERTY, 0x00300150); }
RECOMP_OVERRIDE(0x002FFEB0) { OS_NATIVE(OS_BOSS_NO_OP, 0x002FFEB0); } /* GetTaskIdList */

/* nn::am: the game's downloadable content (am:app), answered from data/dlc.
   nn::am::GetNumContentInfos and GetDataTitleInfos are a no-op instruction
   falling through into the detail function, so both entries are replaced;
   GetAddOnTitleInfos branches to GetDataTitleInfos's detail. */
RECOMP_OVERRIDE(0x00229730) { OS_NATIVE(OS_AM_GET_NUM_CONTENT_INFOS, 0x00229730); }
RECOMP_OVERRIDE(0x00229734) { OS_NATIVE(OS_AM_GET_NUM_CONTENT_INFOS, 0x00229734); }
RECOMP_OVERRIDE(0x00229654) { OS_NATIVE(OS_AM_LIST_CONTENT_INFOS, 0x00229654); }
RECOMP_OVERRIDE(0x002296C4) { OS_NATIVE(OS_AM_GET_DATA_TITLE_INFOS, 0x002296C4); }
RECOMP_OVERRIDE(0x002296C8) { OS_NATIVE(OS_AM_GET_DATA_TITLE_INFOS, 0x002296C8); }
RECOMP_OVERRIDE(0x00229784) { OS_NATIVE(OS_AM_LIST_DATA_TITLE_TICKET_INFOS, 0x00229784); }

/* nn::nim (eShop add-on content), offline */
RECOMP_OVERRIDE(0x0010D44C) { OS_NATIVE(OS_NIM_AOC_INITIALIZE, 0x0010D44C); }

/* gate: nn::err (err:f). FatalErr::Throw(this, const FatalErrInfo&) sends
   the report; the native layer logs it the way the err:f service did. */
RECOMP_OVERRIDE(0x0012B8A8) { OS_NATIVE(OS_ERR_THROW_FATAL, 0x0012B8A8); }
