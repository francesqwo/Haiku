//! gfx-shadow: the engine's CommandAlter layer, natively.
//!
//! What CommandAlter is (USA SE v2, from the disassembly):
//! - Graphics::Render calls CommandAlter::Clear (0x00106640) right before
//!   the upper display is built, remembers the command-buffer offsets around
//!   prim::RenderUpper, and, with stereo 3D on, calls CommandBuffer::Copy
//!   (0x00106B18): it copies that command range for the right eye and runs
//!   CommandAlter::Execute (0x0010B924) on the copy.
//! - While the upper display is built, the engine registers rewrites of the
//!   commands it just wrote, by command-buffer offset, through
//!   AlterImpl::AddBuffer (0x0013C1A0, the data copied at once): type 1 a
//!   3x4 matrix (prim::Context::UpdateWorld, via AddMtx34), type 2 a 4x4
//!   matrix (prim::Context::UpdateState, the projection, via AddMtx44),
//!   type 3 a vector, type 4 raw command words (CommandAlter::AddCommand:
//!   LightSet::MakeAttitudeCommand registers the light set's right-eye
//!   attitude words at +0x46C over the left-eye ones at +0x6C it wrote;
//!   StealthCallback does the same), and type 5 callbacks
//!   (CommandAlter::AddCallback: RenderState::Change's camera, captures,
//!   BloomContext, TextureSetCallback, MovieStereoDrawCallback).
//! - Execute applies them to the copy only. The left eye, the picture this
//!   port shows, is the command list as written.
//!
//! So CommandAlter is the right eye's camera. It does not rebind the shadow
//! receivers' texture (FieldActorCallback does, see shadow.rs).
//!
//! Natively: the native draws are built from engine state, so the left eye
//! needs the words the alterations replace, not the alterations: the light
//! attitude words (attitude(.., Eye::Left)) are what setup.rs ports of
//! MakeAttitudeCommand. The alterations are recorded here per frame, in
//! command order, with their data as AddBuffer copied it, and attitude(..,
//! Eye::Right) gives a native right eye its light set; the port draws no
//! right eye (stereo is off, the 0x410 target is ignored), so Execute is
//! only counted. FATES_GFX_ALTER_TRACE=N prints one frame's alterations
//! from game frame N on.

use std::ffi::c_void;

use zakuro_core::System;

use crate::prim::r32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eye {
    Left,
    Right,
}

/// AlterConst::Type as Execute dispatches it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Mtx34,
    Mtx44,
    Vec4,
    Words,
    Callback,
    Other(u32),
}

impl Kind {
    fn from_raw(t: u32) -> Kind {
        match t {
            1 => Kind::Mtx34,
            2 => Kind::Mtx44,
            3 => Kind::Vec4,
            4 => Kind::Words,
            5 => Kind::Callback,
            n => Kind::Other(n),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Alteration {
    pub kind: Kind,
    /// the command-buffer offset it rewrites at
    pub offset: u32,
    /// the data as AddBuffer copied it (words), or the callback object
    pub data: Vec<u32>,
    pub callback: u32,
}

#[derive(Default)]
pub struct Alter {
    /// this frame's, in command order
    pub list: Vec<Alteration>,
    pub frames: u64,
    pub recorded: u64,
    pub executed: u64,
    pub by_kind: [u64; 6],
    traced: bool,
}

impl Alter {
    fn record(&mut self, a: Alteration) {
        self.recorded += 1;
        let k = match a.kind {
            Kind::Mtx34 => 0,
            Kind::Mtx44 => 1,
            Kind::Vec4 => 2,
            Kind::Words => 3,
            Kind::Callback => 4,
            Kind::Other(_) => 5,
        };
        self.by_kind[k] += 1;
        if self.list.len() < 0x200 {
            self.list.push(a);
        }
    }
}

/// LightSet::MakeAttitudeCommand (0x00546464): the light set's attitude
/// command words for an eye (+0x68 their count; +0x6C the left eye's, which
/// it writes; +0x46C the right eye's, which it hands CommandAlter).
pub fn attitude(system: &mut System, light_set: u32, eye: Eye) -> Vec<u32> {
    let count = r32(system, light_set + 0x68).min(0x100);
    let at = light_set + if eye == Eye::Left { 0x6C } else { 0x46C };
    (0..count).map(|i| r32(system, at + 4 * i)).collect()
}

pub(crate) unsafe extern "C" fn hook_buffer(ctx: *mut c_void, data: u32, bytes: u32, kind: u32, offset: u32) {
    let system = unsafe { crate::system(ctx) };
    let words = (bytes / 4).min(0x100);
    let data: Vec<u32> = (0..words).map(|i| r32(system, data + 4 * i)).collect();
    crate::with(|g| g.alter.record(Alteration { kind: Kind::from_raw(kind), offset, data, callback: 0 }));
}

pub(crate) unsafe extern "C" fn hook_callback(ctx: *mut c_void, callback: u32) {
    let system = unsafe { crate::system(ctx) };
    let offset = 0;
    let vtable = if callback != 0 { r32(system, callback) } else { 0 };
    crate::with(|g| g.alter.record(Alteration { kind: Kind::Callback, offset, data: vec![vtable], callback }));
}

pub(crate) unsafe extern "C" fn hook_clear(_ctx: *mut c_void) {
    crate::with(|g| {
        let frames = g.stats.frames;
        let a = &mut g.alter;
        let wanted = std::env::var("FATES_GFX_ALTER_TRACE").ok().and_then(|v| v.parse::<u64>().ok());
        if wanted.is_some_and(|n| frames >= n) && !a.traced && !a.list.is_empty() {
            a.traced = true;
            for (i, x) in a.list.iter().enumerate() {
                eprintln!("gfx-alter frame {frames} {i}: {:?} at {:X} {:08X?}", x.kind, x.offset, &x.data[..x.data.len().min(8)]);
            }
        }
        a.frames += 1;
        a.list.clear();
    });
}

pub(crate) unsafe extern "C" fn hook_execute(_ctx: *mut c_void, _copy: u32, _offset: u32) {
    crate::with(|g| {
        g.alter.executed += 1;
        if g.alter.executed == 1 {
            eprintln!(
                "gfx-alter: the right eye's copy ran with {} alterations; the native renderer draws no right eye (stereo off)",
                g.alter.list.len()
            );
        }
    });
}
