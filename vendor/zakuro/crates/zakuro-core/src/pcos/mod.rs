//! os: the native PC platform layer that replaces the emulated 3DS OS,
//! the way N64Recomp's ultramodern replaces libultra. Game overrides
//! (overrides/os_*.c) call into it through the bridge instead of making
//! SVCs and IPC requests; `stats` measures what still goes through
//! emulation.

// os2: the APT/srv/cec/ndm start-up requests, natively
pub mod apt;
pub mod bridge;
// native-final: the configuration page and the shared page's clock
pub mod clock;
// native-final: counters of anything emulated a session reached
pub mod emu;
pub mod fs;
// savehang: the hang watchdog and the thread dump
pub mod hang;
// native-final: the per-frame housekeeping (VBlank, framebuffers, clock)
pub mod frame;
// native-final: the controller state in HID's shared memory
pub mod hid;
// gate: the online services, natively and offline
pub mod net;
// gate: the library-applet requests (error display and pickers), natively
pub mod applet;
// gate: StreetPass and the amiibo reader, natively
pub mod local;
// offline-msg: the message shown when an online feature cannot be used
pub mod online;
// online: local wireless (nwm::UDS), natively, carried by crate::netplay
pub mod uds;
// os2: the GPU service calls, natively
pub mod gsp;
// os2: the native scheduler and synchronisation
pub mod sched;
// native-final: the process's memory/information calls, and the one svc entry
pub mod process;
// native-rt: the scheduling modes (deterministic, real time) and their counts
pub mod rt;
pub mod rtpace; // rtpace: dev probes for real-time pacing
pub mod svc;
// audio: the native sound layer (nn::snd's DSP driver, nn::aacdec)
pub mod snd;
pub mod stats;
pub mod sync;
pub mod sys;
// os2: the game's threads as host threads
pub mod threads;
pub mod y2r;
