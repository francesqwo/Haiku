//! Fire Emblem Fates Special Edition USA, recompiled ahead of time by 3dsrecomp and run by Zakuro. it
//! still reads the game for everything but the code, so it takes the path
//! to a copy of it the way Zakuro does.

// qol3: a Windows program, so no console window opens beside the game;
// console.rs keeps redirected and console output working
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::ffi::c_void;
use std::ptr::addr_of;

mod console; // qol3
mod install;

// codemods: Haiku's overrides by address and file (build.rs)
include!(concat!(env!("OUT_DIR"), "/override_names.rs"));

extern "C" {
    static recomp_abi: u32;
    static recomp_generation: u32;
    static recomp_entry_count: u32;
    static recomp_entries: u8;
    static recomp_module_count: u32;
    static recomp_modules: u8;
    static recomp_origins: u8;
    // hooks: the overrides' tails registry (recomp.h, CALL_THEN)
    fn recomp_tails_forget(tls: u32);
    // codemods: the registry patch modules and plugins' after-hooks share
    static mut recomp_tails_pending: u32;
    fn recomp_tail_defer(ctx: *mut c_void, ret: u32, sp: u32, tail: zakuro_core::codemods::Tail, name: *const std::ffi::c_char, words: *const u32);
    fn recomp_tails_due(ctx: *mut c_void);
    fn recomp_tail_count(tail: zakuro_core::codemods::Tail, name: *const std::ffi::c_char, how: i32);
    static mut recomp_tails_counting: u8;
    // wide: overrides/widescreen.c asks the host for the top screen's aspect
    static mut fates_wide_aspect_hook: Option<extern "C" fn() -> f32>;
    // wide2: and tells the native renderer of the wallpapers it stretched
    static mut fates_wide2_rect_hook: Option<extern "C" fn(u32, u32, f32, f32)>;
    // gfx-2d: overrides/gfx_units.c, map sprites snapped at the render scale
    static mut fates_units_snap_hook: Option<extern "C" fn() -> i32>;
    // hfr: overrides/timing.c
    static mut fates_hfr_max_step: u32;
    static fates_hfr_scene_step: u32;
    // os: the native OS bridge, defined by overrides/os_fs.c
    static mut os_native_hook: *const c_void;
    static os_native_abi: u32;
    // gfx: the hook table overrides/gfx_native.c calls through
    static mut fates_gfx_hooks: fates_gfx::Hooks;
    static mut fates_gfx_hooks_3d: fates_gfx::Hooks3D;
    // gfx-shadow: overrides/gfx_shadow.c
    static mut fates_gfx_hooks_shadow: fates_gfx::shadow::Hooks;
    // gfx-post: overrides/gfx_post.c
    static mut fates_gfx_hooks_post: fates_gfx::post::Hooks;
    // gfx-eft: overrides/gfx_eft.c
    static mut fates_gfx_hooks_eft: fates_gfx::HooksEft;
    // gfx-terrain: overrides/gfx_scene2d.c
    static mut fates_gfx_hooks_scene2d: fates_gfx::scene2d::Hooks;
    // gfx-gaps: overrides/gfx_unit_icon.c
    static mut fates_gfx_hooks_unit_icon: fates_gfx::unit_icon::Hooks;
    // gfx-final: overrides/gfx_capture.c
    static mut fates_gfx_hooks_capture: fates_gfx::capture::Hooks;
    // mods: overrides/mods.c, the Main Menu's Mods entry
    static mut haiku_mods_hook: Option<unsafe extern "C" fn(*mut c_void, u32)>;
    // randomizer: overrides/randomizer.c, the New Game randomizer
    static mut haiku_rando_hook: Option<unsafe extern "C" fn(*mut c_void, u32, u32, u32) -> u32>;
    static mut haiku_rando_active: u32;
    // supports: overrides/supports.c, Corrin S-supports anyone, Kana's gender
    static mut haiku_supports_hook: Option<unsafe extern "C" fn(*mut c_void, u32, u32, u32) -> u32>;
    static mut haiku_supports_text: u32;
    // cameras: overrides/cameras.c, the battle free camera and castle walking
    static mut fates_cameras_hook: Option<unsafe extern "C" fn(*mut zakuro::cameras::CamFrame)>;
    // qol: overrides/qol.c, Skip Story, the SpotPass questions, the boon/bane panel
    static mut qol_skip_story: u32;
    static mut qol_skip_spotpass: u32;
    static mut qol_show_choices: u32;
    static mut qol_skip_cutscenes: u32;
    static mut qol_choice_mode: u32;
    static mut qol_skips_started: u32;
    static mut qol_cutscenes_skipped: u32;
    static mut qol_choices_shown: u32;
    static mut qol_spotpass_answered: u32;
    static mut qol_chara_draws: u32;
    static mut qol_chara_selected: u32;
    static mut qol_chara_selected_at: u32;
    static mut qol_chara_boon: u32;
    static mut qol_chara_bane: u32;
    static mut qol_chara_boon_text: u32;
    static mut qol_chara_bane_text: u32;
    static mut qol_movies_skipped: u32;
    static mut qol_skip_tutorial_maps: u32;
    static mut qol_skip_supports: u32;
    // info-ui: overrides/info_ui.c's words (targeting lines, growth rates)
    static mut info_ui_words: [u32; 512];
    // cheats: overrides/cheats.c's words (the Cheats tab)
    static mut cheats_words: [u32; 1024];
    static mut qol_story_types: u32;
    static mut qol_castle_items: u32;
    static mut qol_dialog_open: u32;
    static mut qol_skip_ours: u32;
    static mut qol_quit_request: u32;
    // qol3: the map's opening talk, the title menu's Exit to Desktop
    static mut qol_opening_types: u32;
    static mut qol_title_exit: u32;
    static mut qol_title_exit_request: u32;
    static mut qol_title_plain: u32;
    static mut qol_title_plain_shown: u32;
    // qol4: Options in the title menu, the map menu's items, the soft reset,
    // the extra data's message
    static mut qol_title_options: u32;
    static mut qol_title_options_request: u32;
    static mut qol_map_items: u32;
    static mut qol_soft_reset: u32;
    static mut qol_restart_request: u32;
    static mut qol_restarts: u32;
    static mut qol_ext_read_quiet: u32;
    static mut qol_ext_read_hidden: u32;
    static mut boot3_procthread_guard: u32; // boot3 (overrides/boot3.c)
    static mut boot3_procthread_held: u32;
    static mut boot3_procthread_sleeps: u32;
    static mut boot3_procthread_gave_up: u32;
    // transblur: overrides/transblur.c, the battle transitions' zoom blur
    static mut haiku_transition_blur: u32;
    static mut haiku_transition_unblurred: u32;
    // drawdist: overrides/drawdist.c, Full Draw Distance
    static mut drawdist_full: u32;
    static mut drawdist_batch: fates_gfx::drawdist::Batch;
}

// qol: where each of overrides/qol.c's words is
fn qol_word(word: zakuro::qol::Word) -> *mut u32 {
    use std::ptr::addr_of_mut;
    use zakuro::qol::Word;
    // SAFETY: the addresses of plain words of the linked game build
    unsafe {
        match word {
            Word::SkipStory => addr_of_mut!(qol_skip_story),
            Word::SkipSpotpass => addr_of_mut!(qol_skip_spotpass),
            Word::ShowChoices => addr_of_mut!(qol_show_choices),
            Word::SkipCutscenes => addr_of_mut!(qol_skip_cutscenes),
            Word::ChoiceMode => addr_of_mut!(qol_choice_mode),
            Word::SkipsStarted => addr_of_mut!(qol_skips_started),
            Word::CutscenesSkipped => addr_of_mut!(qol_cutscenes_skipped),
            Word::ChoicesShown => addr_of_mut!(qol_choices_shown),
            Word::SpotpassAnswered => addr_of_mut!(qol_spotpass_answered),
            Word::CharaDraws => addr_of_mut!(qol_chara_draws),
            Word::CharaSelected => addr_of_mut!(qol_chara_selected),
            Word::CharaSelectedAt => addr_of_mut!(qol_chara_selected_at),
            Word::CharaBoon => addr_of_mut!(qol_chara_boon),
            Word::CharaBane => addr_of_mut!(qol_chara_bane),
            Word::CharaBoonText => addr_of_mut!(qol_chara_boon_text),
            Word::CharaBaneText => addr_of_mut!(qol_chara_bane_text),
            Word::MoviesSkipped => addr_of_mut!(qol_movies_skipped),
            Word::SkipTutorialMaps => addr_of_mut!(qol_skip_tutorial_maps),
            Word::SkipSupports => addr_of_mut!(qol_skip_supports),
            Word::StoryTypes => addr_of_mut!(qol_story_types),
            Word::CastleItems => addr_of_mut!(qol_castle_items),
            Word::DialogOpen => addr_of_mut!(qol_dialog_open),
            Word::SkipOurs => addr_of_mut!(qol_skip_ours),
            Word::QuitRequest => addr_of_mut!(qol_quit_request),
            Word::OpeningTypes => addr_of_mut!(qol_opening_types), // qol3
            Word::TitleExit => addr_of_mut!(qol_title_exit),
            Word::TitleExitRequest => addr_of_mut!(qol_title_exit_request),
            Word::TitlePlain => addr_of_mut!(qol_title_plain),
            Word::TitlePlainShown => addr_of_mut!(qol_title_plain_shown),
            Word::TitleOptions => addr_of_mut!(qol_title_options), // qol4
            Word::TitleOptionsRequest => addr_of_mut!(qol_title_options_request),
            Word::MapItems => addr_of_mut!(qol_map_items),
            Word::SoftReset => addr_of_mut!(qol_soft_reset),
            Word::RestartRequest => addr_of_mut!(qol_restart_request),
            Word::Restarts => addr_of_mut!(qol_restarts),
            Word::ExtReadQuiet => addr_of_mut!(qol_ext_read_quiet),
            Word::ExtReadHidden => addr_of_mut!(qol_ext_read_hidden),
            Word::ProcThreadGuard => addr_of_mut!(boot3_procthread_guard), // boot3
            Word::ProcThreadHeld => addr_of_mut!(boot3_procthread_held),
            Word::ProcThreadSleeps => addr_of_mut!(boot3_procthread_sleeps),
            Word::ProcThreadGaveUp => addr_of_mut!(boot3_procthread_gave_up),
            Word::TransitionBlur => addr_of_mut!(haiku_transition_blur), // transblur
            Word::TransitionUnblurred => addr_of_mut!(haiku_transition_unblurred),
            Word::FullDrawDistance => addr_of_mut!(drawdist_full), // drawdist
        }
    }
}

// info-ui: where overrides/info_ui.c's words are
fn info_ui_words_ptr() -> *mut u32 {
    // SAFETY: the address of a static array of the game build
    std::ptr::addr_of_mut!(info_ui_words).cast::<u32>()
}

// cheats: where overrides/cheats.c's words are
fn cheats_words_ptr() -> *mut u32 {
    // SAFETY: the address of a static array of the game build
    std::ptr::addr_of_mut!(cheats_words).cast::<u32>()
}

// hfr: the game's timing overrides, for the frontend's frame rate settings
fn hfr_set_max_step(step: u32) {
    // SAFETY: a plain word the game's code reads on the same thread
    unsafe { std::ptr::write_volatile(std::ptr::addr_of_mut!(fates_hfr_max_step), step) }
}
fn hfr_scene_step() -> u32 {
    // SAFETY: as above
    unsafe { std::ptr::read_volatile(std::ptr::addr_of!(fates_hfr_scene_step)) }
}

// native-final: the shipped build is pure; emulation is for development
// comparisons only
#[cfg(all(feature = "pure", feature = "emulation"))]
compile_error!("`pure` (the default) and `emulation` exclude each other: build with --no-default-features --features emulation");

fn main() {
    console::attach(); // qol3: before anything is printed
    console::wait_for_restart(); // qol3: the Mods screen's restart
    // Portable, process-local storage. This runs before the runtime creates
    // threads and does not change the user's Windows environment.
    // FATES_ROOT overrides the portable folder, for development runs
    let root = match std::env::var_os("FATES_ROOT") {
        Some(root) => std::path::PathBuf::from(root),
        None => std::env::current_exe().expect("executable location")
            .parent().expect("executable directory").to_path_buf(),
    };
    // os: `Haiku --install <dump> [dlc.cia ...]` extracts the game into
    // data/ once; `--verify-data` checks data/ against the dump
    if let Some(code) = install::run(&root) {
        std::process::exit(code);
    }
    // the project is called Haiku: settings, saves and caches moved from
    // userdata/zakuro and cache/zakuro, once
    for (base, old, new) in [("userdata", "zakuro", "haiku"), ("cache", "zakuro", "haiku")] {
        let (old, new) = (root.join(base).join(old), root.join(base).join(new));
        if old.is_dir() && !new.exists() {
            if let Err(error) = std::fs::rename(&old, &new) {
                eprintln!("could not move {} to {}: {error}", old.display(), new.display());
            }
        }
    }
    std::env::set_var("APPDATA", root.join("userdata"));
    std::env::set_var("LOCALAPPDATA", root.join("cache"));
    // os: the game is read from data/ as plain files; the ROM image is only
    // a fallback for a setup that hasn't been installed yet
    let data = root.join("data");
    let game = if zakuro_fs::datadir::is_game_dir(&data) {
        data
    } else {
        eprintln!("no game in {}, run `Haiku --install <your dump>`; trying game/Fates-USA.cxi", data.display());
        root.join("game/Fates-USA.cxi")
    };
    std::env::set_var("FATES_DEFAULT_ROM", game);
    std::env::set_var("FATES_ORIGINAL_UI", "1");
    // os: the overrides that replace SDK calls into the OS reach the
    // native platform layer through this hook
    // SAFETY: set once, before any recompiled code runs
    unsafe {
        if let Some(hook) = zakuro_core::pcos::bridge::hook(os_native_abi) {
            os_native_hook = hook as *const c_void;
        }
    }
    // SAFETY: the tables 3dsrecomp wrote beside the code build.rs links in
    let code = unsafe {
        zakuro::Linked::new(
            0x0004000000179800,
            recomp_abi,
            addr_of!(recomp_entries) as *const c_void,
            recomp_entry_count,
            addr_of!(recomp_modules) as *const c_void,
            recomp_module_count,
        )
        // so that functions a mod or an update changed run in the interpreter
        .with_origins(recomp_generation, addr_of!(recomp_origins) as *const c_void)
    };
    // hooks: a loaded state or an ended thread drops the overrides' tails
    // waiting in it; FATES_HOOKS_STATS=1 counts how each tail ran, printed
    // at exit. SAFETY: set before the game's code runs
    zakuro_core::recompiled::set_tails_forget(recomp_tails_forget);
    zakuro::haiku_codemods::set_override_names(OVERRIDE_NAMES); // codemods
    // codemods: SAFETY: the code's own registry, whose C types these are
    // (recomp.h); a Context pointer is passed through as it came
    unsafe {
        zakuro_core::codemods::set_imports(zakuro_core::codemods::Imports {
            tails_pending: std::ptr::addr_of_mut!(recomp_tails_pending),
            tails_counting: std::ptr::addr_of_mut!(recomp_tails_counting),
            tail_defer: std::mem::transmute::<unsafe extern "C" fn(*mut c_void, u32, u32, zakuro_core::codemods::Tail, *const std::ffi::c_char, *const u32), _>(recomp_tail_defer),
            tails_due: std::mem::transmute::<unsafe extern "C" fn(*mut c_void), _>(recomp_tails_due),
            tail_count: recomp_tail_count,
        });
    }
    if std::env::var("FATES_HOOKS_STATS").is_ok_and(|value| value == "1") {
        unsafe { recomp_tails_counting = 1 };
    }
    // mods: the Main Menu's Mods entry (FATES_MODS_TITLE=0 leaves it out),
    // and no mod's code changes: the code is recompiled ahead of time, so
    // code a mod changed could not run; the Mods screen says so instead
    if std::env::var("FATES_MODS_TITLE").map_or(true, |value| value != "0") {
        // SAFETY: set once, before any recompiled code runs
        unsafe { haiku_mods_hook = Some(zakuro::haiku_mods::title_hook) };
    }
    zakuro_core::mods::allow_code_patches(false);
    // randomizer: the New Game randomizer's hooks (FATES_RANDOMIZER=0 leaves
    // them out). SAFETY: set once, before any recompiled code runs
    if std::env::var("FATES_RANDOMIZER").map_or(true, |value| value != "0") {
        unsafe {
            zakuro_core::randomizer::set_active_word(std::ptr::addr_of_mut!(haiku_rando_active));
            haiku_rando_hook = Some(zakuro_core::randomizer::hook);
        }
    }
    // supports: Corrin S-supports anyone and Kana's gender (FATES_SUPPORTS=0
    // leaves the hooks out). SAFETY: set once, before any recompiled code runs
    if std::env::var("FATES_SUPPORTS").map_or(true, |value| value != "0") {
        unsafe {
            zakuro_core::supports::set_text_word(std::ptr::addr_of_mut!(haiku_supports_text));
            haiku_supports_hook = Some(zakuro_core::supports::hook);
        }
    }
    zakuro::hfr::set_game_timing(zakuro::hfr::GameTiming { set_max_step: hfr_set_max_step, scene_step: hfr_scene_step });
    zakuro::qol::set_hooks(zakuro::qol::Hooks { word: qol_word }); // qol
    zakuro::info_ui::set_hooks(zakuro::info_ui::Hooks { words: info_ui_words_ptr }); // info-ui
    zakuro::cheats::set_hooks(zakuro::cheats::Hooks { words: cheats_words_ptr }); // cheats
    // cpu: the game's code runs only as recompiled native code. reaching
    // anything that would need Zakuro's ARM interpreter stops with the
    // address and a guest callstack; FATES_ALLOW_INTERPRETER=1 lets the
    // interpreter run it instead, for diagnostics only
    let allow = std::env::var("FATES_ALLOW_INTERPRETER").is_ok_and(|value| value == "1");
    zakuro::native_cpu::enable(!allow);
    // wide: the game's cameras follow the shape the GPU draws the top
    // screen at. SAFETY: set before the game's code runs, read only by it
    unsafe { fates_wide_aspect_hook = Some(zakuro::wide_aspect_hook) };
    // cameras: the free camera's and the castle walk's input, from the
    // frontend. SAFETY: set before the game's code runs
    unsafe { fates_cameras_hook = Some(zakuro::cameras::game_hook) };
    // Haiku's icon on the window and the taskbar (the exe's own icon is made
    // from the same picture by build.rs; builder: assets/icon/haiku.png)
    let _ = zakuro::WINDOW_ICON_PNG.set(include_bytes!("../../assets/icon/haiku.png"));
    // gfx-hfr: the native renderer is the default (pc.native_graphics, the
    // options' Renderer: Native / Compatibility); FATES_NATIVE_GFX and
    // FATES_GFX_NATIVE3D still override it, =0 the emulated GPU
    // native-final: the pure build has no emulated GPU, so the native
    // renderer is the only one, whatever the environment says
    if !cfg!(feature = "emulation") {
        std::env::set_var("FATES_NATIVE_GFX", "1");
        std::env::set_var("FATES_GFX_NATIVE3D", "1");
    }
    if std::env::var_os("FATES_NATIVE_GFX").is_none() {
        let native = zakuro::native_graphics_setting();
        std::env::set_var("FATES_NATIVE_GFX", if native { "1" } else { "0" });
        if native && std::env::var_os("FATES_GFX_NATIVE3D").is_none() {
            std::env::set_var("FATES_GFX_NATIVE3D", "1");
        }
    }
    // gfx: the native renderer, when FATES_NATIVE_GFX asks for it
    // SAFETY: nothing runs game code yet, so nothing reads the table
    fates_gfx::install(unsafe { &mut *std::ptr::addr_of_mut!(fates_gfx_hooks) });
    fates_gfx::install_3d(unsafe { &mut *std::ptr::addr_of_mut!(fates_gfx_hooks_3d) });
    // gfx-shadow
    fates_gfx::shadow::install(unsafe { &mut *std::ptr::addr_of_mut!(fates_gfx_hooks_shadow) });
    fates_gfx::install_post(unsafe { &mut *std::ptr::addr_of_mut!(fates_gfx_hooks_post) }); // gfx-post
    // gfx-eft: the effect hook
    fates_gfx::install_eft(unsafe { &mut *std::ptr::addr_of_mut!(fates_gfx_hooks_eft) });
    // gfx-terrain: 2D inside scene passes (map sprites, gauges, cursor)
    fates_gfx::scene2d::install(unsafe { &mut *std::ptr::addr_of_mut!(fates_gfx_hooks_scene2d) });
    // gfx-gaps: map unit sprites and HP gauges
    fates_gfx::install_unit_icon(unsafe { &mut *std::ptr::addr_of_mut!(fates_gfx_hooks_unit_icon) });
    // wide2: wallpapers drawn at their own size, their edges extended
    fates_gfx::install_wide2(unsafe { &mut *std::ptr::addr_of_mut!(fates_wide2_rect_hook) });
    unsafe { fates_units_snap_hook = Some(fates_gfx::units_snap) }; // gfx-2d
    // gfx-final: unit-icon textures rendered natively
    fates_gfx::install_capture(unsafe { &mut *std::ptr::addr_of_mut!(fates_gfx_hooks_capture) });
    // drawdist: far grass in batches, the batch overrides/drawdist.c fills
    fates_gfx::drawdist::set_batch(unsafe { std::ptr::addr_of_mut!(drawdist_batch) });
    zakuro::run(Some(code));
}
