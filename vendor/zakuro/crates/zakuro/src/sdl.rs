//! input: a thin, hand-written binding to the parts of SDL3 the controllers
//! need, its gamepad API. SDL3 is loaded at run time from SDL3.dll beside the
//! executable (libSDL3.so.0 / libSDL3.0.dylib elsewhere), so a missing or
//! broken library only costs the SDL backend, never the program: the caller
//! falls back to gilrs.
//!
//! Only plain C types cross the boundary. Signatures follow SDL 3.2+'s
//! headers (SDL_gamepad.h, SDL_events.h, SDL_init.h, SDL_hints.h).

#![allow(non_camel_case_types, non_snake_case, dead_code)]

use std::ffi::{c_char, c_int, c_void, CStr, CString};

pub type SDL_JoystickID = u32;
pub type SDL_Gamepad = c_void;

pub const SDL_INIT_GAMEPAD: u32 = 0x0000_2000;

pub const SDL_EVENT_GAMEPAD_BUTTON_DOWN: u32 = 0x651;
pub const SDL_EVENT_GAMEPAD_BUTTON_UP: u32 = 0x652;
pub const SDL_EVENT_GAMEPAD_ADDED: u32 = 0x653;
pub const SDL_EVENT_GAMEPAD_REMOVED: u32 = 0x654;
pub const SDL_EVENT_GAMEPAD_REMAPPED: u32 = 0x655;

/// SDL_GamepadType.
pub mod pad_type {
    pub const UNKNOWN: i32 = 0;
    pub const STANDARD: i32 = 1;
    pub const XBOX360: i32 = 2;
    pub const XBOXONE: i32 = 3;
    pub const PS3: i32 = 4;
    pub const PS4: i32 = 5;
    pub const PS5: i32 = 6;
    pub const SWITCH_PRO: i32 = 7;
    pub const JOYCON_LEFT: i32 = 8;
    pub const JOYCON_RIGHT: i32 = 9;
    pub const JOYCON_PAIR: i32 = 10;
    pub const GAMECUBE: i32 = 11;
    pub const STEAM: i32 = 12;
}

/// SDL_GamepadButtonLabel.
pub mod label {
    pub const UNKNOWN: i32 = 0;
    pub const A: i32 = 1;
    pub const B: i32 = 2;
    pub const X: i32 = 3;
    pub const Y: i32 = 4;
    pub const CROSS: i32 = 5;
    pub const CIRCLE: i32 = 6;
    pub const SQUARE: i32 = 7;
    pub const TRIANGLE: i32 = 8;
}

/// SDL_GamepadButton, positional: SOUTH is the bottom face button whatever
/// is printed on it.
pub const BUTTON_COUNT: usize = 26;
/// SDL_GamepadAxis.
pub const AXIS_LEFTX: c_int = 0;
pub const AXIS_LEFTY: c_int = 1;
pub const AXIS_RIGHTX: c_int = 2;
pub const AXIS_RIGHTY: c_int = 3;
pub const AXIS_LEFT_TRIGGER: c_int = 4;
pub const AXIS_RIGHT_TRIGGER: c_int = 5;

/// one SDL_Event, a 128-byte union; the fields read here sit at the same
/// offsets in every gamepad event: type at 0, which at 16, button at 20,
/// down at 21.
#[repr(C, align(8))]
pub struct SDL_Event(pub [u8; 128]);

impl SDL_Event {
    pub fn kind(&self) -> u32 {
        u32::from_ne_bytes(self.0[0..4].try_into().unwrap())
    }
    pub fn which(&self) -> SDL_JoystickID {
        u32::from_ne_bytes(self.0[16..20].try_into().unwrap())
    }
    pub fn button(&self) -> u8 {
        self.0[20]
    }
}

macro_rules! api {
    ($($name:ident : fn($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        /// the SDL3 functions used, resolved from the loaded library.
        #[allow(non_snake_case)]
        pub struct Sdl {
            _library: Library,
            $(pub $name: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)*
        }

        impl Sdl {
            /// loads SDL3 and resolves every function, or says what is missing.
            pub fn load() -> Result<Sdl, String> {
                let library = Library::open()?;
                $(
                    let $name = {
                        let symbol = library.symbol(concat!(stringify!($name), "\0"))
                            .ok_or_else(|| format!("SDL3 has no {}", stringify!($name)))?;
                        // SAFETY: the symbol is SDL3's exported function of this name,
                        // whose C signature the declaration above copies
                        unsafe { std::mem::transmute::<*mut c_void, unsafe extern "C" fn($($arg),*) $(-> $ret)?>(symbol) }
                    };
                )*
                Ok(Sdl { _library: library, $($name,)* })
            }
        }
    };
}

api! {
    SDL_Init: fn(u32) -> bool;
    SDL_QuitSubSystem: fn(u32);
    SDL_SetHint: fn(*const c_char, *const c_char) -> bool;
    SDL_GetError: fn() -> *const c_char;
    SDL_GetVersion: fn() -> c_int;
    SDL_GetRevision: fn() -> *const c_char;
    SDL_free: fn(*mut c_void);
    SDL_PollEvent: fn(*mut SDL_Event) -> bool;
    SDL_GetGamepads: fn(*mut c_int) -> *mut SDL_JoystickID;
    SDL_OpenGamepad: fn(SDL_JoystickID) -> *mut SDL_Gamepad;
    SDL_CloseGamepad: fn(*mut SDL_Gamepad);
    SDL_GetGamepadName: fn(*mut SDL_Gamepad) -> *const c_char;
    SDL_GetGamepadType: fn(*mut SDL_Gamepad) -> c_int;
    SDL_GetRealGamepadType: fn(*mut SDL_Gamepad) -> c_int;
    SDL_GetGamepadVendor: fn(*mut SDL_Gamepad) -> u16;
    SDL_GetGamepadProduct: fn(*mut SDL_Gamepad) -> u16;
    SDL_GetGamepadButton: fn(*mut SDL_Gamepad, c_int) -> bool;
    SDL_GetGamepadAxis: fn(*mut SDL_Gamepad, c_int) -> i16;
    SDL_GetGamepadButtonLabel: fn(*mut SDL_Gamepad, c_int) -> c_int;
    SDL_GetGamepadConnectionState: fn(*mut SDL_Gamepad) -> c_int;
    SDL_RumbleGamepad: fn(*mut SDL_Gamepad, u16, u16, u32) -> bool;
    SDL_SetGamepadPlayerIndex: fn(*mut SDL_Gamepad, c_int) -> bool;
    SDL_SetGamepadLED: fn(*mut SDL_Gamepad, u8, u8, u8) -> bool;
    SDL_IOFromConstMem: fn(*const c_void, usize) -> *mut c_void;
    SDL_AddGamepadMappingsFromIO: fn(*mut c_void, bool) -> c_int;
    SDL_AddGamepadMappingsFromFile: fn(*const c_char) -> c_int;
}

impl Sdl {
    pub fn error(&self) -> String {
        // SAFETY: SDL_GetError returns a valid, possibly empty, C string
        unsafe { cstr((self.SDL_GetError)()) }
    }

    pub fn set_hint(&self, name: &str, value: &str) {
        let (name, value) = (CString::new(name).unwrap(), CString::new(value).unwrap());
        // SAFETY: both are valid C strings SDL copies
        unsafe { (self.SDL_SetHint)(name.as_ptr(), value.as_ptr()) };
    }

    /// SDL's version as major.minor.patch, and its revision.
    pub fn version(&self) -> String {
        // SAFETY: plain calls without arguments
        let (version, revision) = unsafe { ((self.SDL_GetVersion)(), cstr((self.SDL_GetRevision)())) };
        format!("{}.{}.{} ({revision})", version / 1_000_000, version / 1000 % 1000, version % 1000)
    }

    /// the instance ids of the gamepads connected now.
    pub fn gamepads(&self) -> Vec<SDL_JoystickID> {
        let mut count = 0;
        // SAFETY: SDL returns an array of count ids, or null, freed with SDL_free
        unsafe {
            let ids = (self.SDL_GetGamepads)(&mut count);
            if ids.is_null() {
                return Vec::new();
            }
            let list = std::slice::from_raw_parts(ids, count.max(0) as usize).to_vec();
            (self.SDL_free)(ids.cast());
            list
        }
    }
}

/// a C string SDL owns, copied, empty for null.
///
/// # Safety
/// `text` is null or a valid NUL-terminated string.
pub unsafe fn cstr(text: *const c_char) -> String {
    if text.is_null() {
        String::new()
    } else {
        CStr::from_ptr(text).to_string_lossy().into_owned()
    }
}

/// the shared library SDL3 is in.
struct Library(*mut c_void);

// SAFETY: a module handle is only an address, usable from any thread
unsafe impl Send for Library {}

#[cfg(windows)]
mod os {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    extern "system" {
        pub fn LoadLibraryW(name: *const u16) -> *mut c_void;
        pub fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    }
}

#[cfg(unix)]
mod os {
    use std::ffi::{c_char, c_int, c_void};
    extern "C" {
        pub fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
        pub fn dlsym(module: *mut c_void, name: *const c_char) -> *mut c_void;
    }
}

impl Library {
    /// SDL3 from beside the executable first, then wherever the system finds it.
    #[cfg(windows)]
    fn open() -> Result<Library, String> {
        use std::os::windows::ffi::OsStrExt;
        let beside = std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.join("SDL3.dll")));
        let candidates = beside.into_iter().map(|path| path.into_os_string()).chain([std::ffi::OsString::from("SDL3.dll")]);
        for name in candidates {
            let wide: Vec<u16> = name.encode_wide().chain([0]).collect();
            // SAFETY: a NUL-terminated wide path
            let module = unsafe { os::LoadLibraryW(wide.as_ptr()) };
            if !module.is_null() {
                return Ok(Library(module));
            }
        }
        Err("SDL3.dll was not found beside the program or on the system".into())
    }

    #[cfg(unix)]
    fn open() -> Result<Library, String> {
        const RTLD_NOW: c_int = 2;
        let names: &[&str] = if cfg!(target_os = "macos") { &["libSDL3.0.dylib", "libSDL3.dylib"] } else { &["libSDL3.so.0", "libSDL3.so"] };
        for name in names {
            let name = CString::new(*name).unwrap();
            // SAFETY: a NUL-terminated name
            let module = unsafe { os::dlopen(name.as_ptr(), RTLD_NOW) };
            if !module.is_null() {
                return Ok(Library(module));
            }
        }
        Err("libSDL3 was not found".into())
    }

    fn symbol(&self, name: &str) -> Option<*mut c_void> {
        // SAFETY: name ends in NUL; a missing symbol comes back null
        #[cfg(windows)]
        let symbol = unsafe { os::GetProcAddress(self.0, name.as_ptr()) };
        #[cfg(unix)]
        let symbol = unsafe { os::dlsym(self.0, name.as_ptr().cast()) };
        (!symbol.is_null()).then_some(symbol)
    }
}
