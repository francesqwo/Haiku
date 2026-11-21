//! moflexhd: a movie's pictures (issue #7, higher-resolution MOFLEX movies).
//!
//! The movie player has each picture converted (YUV to RGB) into the same
//! texture, so every picture is a texture of its own to the 2D renderer (it
//! knows textures by their address and their texels). A texture the
//! converter writes keeps only its last few pictures, decoded (lib.rs
//! `moflexhd_keep_few`) and on the GPU (render.rs `texture_view`), instead
//! of every picture until a limit far away: 4096 decoded textures, and 600
//! frames on the GPU. Any other texture is kept as before.
//! `FATES_MOVIE_STREAM=0` keeps a movie's pictures too, as before.

use crate::prim::TexRef;

/// pictures of one texture kept: the one being drawn, the one before (two
/// eyes, a picture in between) and one to spare.
pub const KEPT_VERSIONS: usize = 3;

/// whether the texture is a movie's picture, whose old pictures go.
pub fn few_kept(tex: TexRef) -> bool {
    !streaming_off() && zakuro_core::moflexhd::is_picture(tex.addr)
}

/// FATES_MOVIE_STREAM=0: every picture kept, as before (tests).
pub fn streaming_off() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| std::env::var_os("FATES_MOVIE_STREAM").is_some_and(|v| v == "0"))
}
