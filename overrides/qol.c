/* qol: quality-of-life options (USA Special Edition v2).
 *
 * 1. Skip Story. Fates has one skip system for its story: GameSkip, a
 *    singleton at *0x006D8808 (state byte +0, flags +4: bit 0 skip
 *    disabled, bit 1 past the skippable part, bit 2 escape asked).
 *    - An event script marks the parts a player may skip with SkipEnable /
 *      SkipEscape / SkipDisable (GameSkipControl, GameSkipSequenceHelper),
 *      which clear and set those flags.
 *    - The procs that run story (ChapterSequence's ProcSequence, the event
 *      action sequence, the castle's JoinSeq) tick GameSkip from their
 *      Persistent. GameSkip::Tick starts a skip when Start is pressed while
 *      (flags & 3) == 0, no map tutorial is up and no skip runs:
 *      GameSkip::Trigger (0x004EB9FC), the same call the event scripts'
 *      own SkipTrigger command makes.
 *    - While it skips (state != 0) the screen is black and every story proc
 *      runs its skip path: talk is consumed by TalkCodeDisposer's Skip
 *      handlers (so flags, joins, items and BGM changes still happen), event
 *      commands run without waiting, movies and 3D cutscenes are not
 *      played; the script's SkipEscape ends it and the screen fades back in.
 *    With qol_skip_story set, those Persistents call GameSkip::Trigger
 *    themselves the first tick a skippable part is open, instead of waiting
 *    for Start. Nothing else changes: the game's own skip path does the
 *    work, exactly as when a player presses Start at once.
 *    - 3D cutscenes run by the event action proc (ProcAction::Persistent,
 *      0x003CB690) skip on Start themselves (a white fade, then Trigger);
 *      with qol_skip_story the press they wait for is given to them.
 *    - Choices are not story. During a skip the event dialogs answer their
 *      first item without showing (RouteBranchDialog, DialogYesNo,
 *      Dialog1..8 test GameSkip::IsSkip). When the skip was ours, the dialog
 *      is shown instead: the skip is asked to end (escape) and the dialog is
 *      built as if no skip ran. The route choice, and any other choice an
 *      event asks, always appears.
 *    - Support conversations a player opens (RelianceTalkSequence: its
 *      ProcSequence and ProcAction below, and the Library's support viewer)
 *      and My Castle's facilities tick GameSkip from their own procs, which
 *      are left as they are (qol_skip_supports, off, would skip them too).
 *
 *    - Movies an event plays (MoviePlayImpl, the scripts' MoviePlay) skip on
 *      Start or A/B/X/Y in the movie thread's MovieLeave (0x002247A8): it
 *      marks the movie skipped (Movie::SetSkipped, which the script's
 *      MovieIsSkipped reads), blacks out and stops the sound. With
 *      qol_skip_story that press is given to it. Movies a player starts
 *      (Extras' movie viewer, the castle library) and the title's opening
 *      are left alone.
 *
 * 2. The SpotPass questions after avatar creation. link::BossSeq::EntryBind
 *    (0x0041028C) starts link::ProcChange in its entry mode: "Would you like
 *    to receive notifications ... via SpotPass?" and then "gameplay-data
 *    reporting?". With qol_skip_spotpass it answers both No at once, as the
 *    player would: no child proc, SpotPass not registered, and the
 *    gameplay-data reporting bit (GameUserData +0x28 bit 20) cleared, which
 *    is what EntryReportBegin does before it asks.
 *
 * 3. The boon/bane stat panel. title::CharaMenuItem::Draw (0x00433F54) runs
 *    for every row of the avatar's Name / Birthday / Boon / Bane / Talent
 *    menu while it shows; it tells the host which row has the cursor and
 *    the boon and bane chosen (ProcMyUnitEdit at *0x006FEC10, +0x52 and
 *    +0x53), and the host draws the stat changes from the game's own
 *    Personality table.
 *
 * 4. Story only, never the battle. The event running is the event manager's
 *    (event at 0x006D42F0, +8 the running ProcEvent; one at a time,
 *    ProcEvent::CreateBind refuses a second), and its function type is the
 *    byte at ProcEvent +0x5C (ProcEvent::CreateBind stores it): Opening 0x0C,
 *    Complete 0x0D, Ending 0x0F (the story); MapLoad 7, Turn 0x10, TurnAfter
 *    0x12, Reinforce 0x13, Area 0x14, Poke 0x15, Talk 0x17 and the rest
 *    (the battle: phase changes, opening quotes on the map, boss and battle
 *    talk, villages). Skip Story starts a skip only while an event of a
 *    story type runs (qol_story_types, set by the host), or, off any map
 *    (map::Situation at *0x006D7ED0 is null), any event (My Castle's).
 *    Outside an event nothing is started: the enemy phase, which the game
 *    lets Start skip, is never touched.
 *
 * 5. Return to Title and Exit to Desktop in My Castle's menu (the Start
 *    menu: Buildings, Castle Settings, Unit Logbook, Save, Next Battle; the
 *    two go before Next Battle).
 *    ButlerSeq::MainMenu builds it with BasicMenu::Create, whose override
 *    (overrides/mods.c) asks qol_menu_create first: for that menu it adds two
 *    items of MI_Save's class, marked, whose name, help and A this file
 *    answers. A asks the game's own Yes/No (GameDialogYesNo::CreateBind,
 *    bound to the menu's proc as Save's own file menu is); Yes on Return to
 *    Title is the game's own way back to the title after a game over
 *    (MainSequence::Jump(2), as title::ProcGameOver does); Yes on Exit to
 *    Desktop asks the host to close Haiku.
 *
 * 6. qol4: Return to Title (qol5b: and no longer Exit to Desktop, which
 *    stays in the options, the Main Menu and My Castle's menu) in the map's own menu
 *    (map::CreateSystemMenu: Unit, Guide, Options, Suspend, Delegate, End,
 *    and Save / Surrender where a mode has them; it goes before End). It
 *    is built with BasicMenu::Create too, so qol_menu_create adds an item
 *    of TurnEnd's class (the plain BasicMenuItem attribute, so never greyed),
 *    marked, whose name, help and A this file answers, as for My Castle.
 *    Yes on Return to Title is the game's own soft reset (below).
 *
 * 7. qol4: the game's own soft reset. HomeButton::Tick (0x001045B0) restarts
 *    the application on L+R+Start, when the game allows it (the byte at
 *    0x006BD8E8 +1, which the game clears while it must not be stopped:
 *    saving, the HOME menu's own work), through
 *    nn::applet::CTR::detail::RestartApplication (0x00107AFC). On the
 *    console that closes the game and starts it again from its first frame.
 *    On PC the applet manager is gone (the APT request had no answer, so
 *    L+R+Start did nothing), so RestartApplication tells the host
 *    (qol_restart_request), which starts the game again in the same window,
 *    as the console does. Return to Title (the map's menu, the options)
 *    asks for it with qol_soft_reset: the next Tick where the game allows a
 *    soft reset takes that path, exactly as L+R+Start would.
 *
 * 8. qol4: no "Loading extra data..." at each start. link::ProcInit reads
 *    the extra data (ExtSave, on the console's SD card) at each start, in
 *    ProcExtRead, a thread proc whose Begin opens the game's wait message
 *    ("Loading extra data... Please do not turn the power off or remove the
 *    SD Card.", MID_EXT_READ) and whose End closes it. With
 *    qol_ext_read_quiet they do the rest of their work (the HOME button
 *    held off while it reads) without the message. The read itself is
 *    unchanged. The first start's creation notices (MID_EXT_CREATE...) are
 *    left: they come once, and the full replay from boot answers them.
 *
 * Every switch is set by the host from the options; all start off, so a
 * build without the host's settings behaves as the game.
 */
#include "overrides.h"

/* set by the host */
uint32_t qol_skip_story = 0;
uint32_t qol_skip_spotpass = 0;
uint32_t qol_show_choices = 1;   /* choices during our skips are shown */
uint32_t qol_skip_cutscenes = 1; /* 3D cutscenes given their Start */
uint32_t qol_skip_tutorial_maps = 0; /* also while a map tutorial is up (the game's own Start does not skip then); the host sets it on, FATES_QOL_TUTORIAL=0 off */
uint32_t qol_skip_supports = 0;
uint32_t qol_story_types = (1u << 0x0C) | (1u << 0x0D) | (1u << 0x0F); /* Opening, Complete, Ending */
uint32_t qol_castle_items = 0;   /* Return to Title and Exit to Desktop in My Castle's menu */ /* also support conversations a player opens (never, by default) */
uint32_t qol_choice_mode = 1;    /* 1: a skip starts again once the choice is made; 2: at the script's next skippable part */
/* qol3: the map's opening talk: events of these types (MapLoad 7, Turn
   0x10) while the map is still at turn 1 in the player's phase, before the
   player has had a turn, are the pre-battle conversation and are skipped
   too; 0 skips none (round 2's rule) */
uint32_t qol_opening_types = 0;
uint32_t qol_title_exit = 0;         /* qol3: Exit to Desktop in the title's Main Menu (overrides/mods.c) */
uint32_t qol_title_exit_request = 0; /* qol3: it was chosen (the host clears it) */
uint32_t qol_title_options = 0;         /* qol4: Options in the title's Main Menu (overrides/mods.c) */
uint32_t qol_title_options_request = 0; /* qol4: it was chosen (the host clears it) */
uint32_t qol_map_items = 0;             /* qol4: Return to Title and Exit to Desktop in the map's menu */
uint32_t qol_soft_reset = 0;            /* qol4: Return to Title asked: the game's soft reset at its next chance */
uint32_t qol_restart_request = 0;       /* qol4: the game restarts itself (its soft reset); the host clears it */
uint32_t qol_restarts = 0;              /* qol4: times it did */
uint32_t qol_ext_read_quiet = 0;        /* qol4: no "Loading extra data..." message */
uint32_t qol_ext_read_hidden = 0;       /* qol4: times it was left out */

/* read by the host */
uint32_t qol_skips_started = 0;     /* skips Skip Story started */
uint32_t qol_cutscenes_skipped = 0; /* 3D cutscenes it gave Start to */
uint32_t qol_choices_shown = 0;     /* choices shown during its skips */
uint32_t qol_choice_open = 0;       /* a choice shown during a skip waits for the player */
uint32_t qol_dialog_open = 0;       /* an event's choice is up (any, skip or not) */
uint32_t qol_skip_ours = 0;         /* the skip running was started by Skip Story (the host clears it) */
uint32_t qol_quit_request = 0;      /* Exit to Desktop was confirmed (the host clears it) */
uint32_t qol_confirm = 0;           /* our question waiting for Yes or No: 1 title, 2 exit */
uint32_t qol_event_movie = 0;       /* the movie playing was started by an event script */
uint32_t qol_movies_skipped = 0;    /* event movies given their Start */
uint32_t qol_spotpass_answered = 0; /* times the SpotPass questions were answered */
uint32_t qol_chara_draws = 0;       /* CharaMenuItem::Draw calls */
uint32_t qol_chara_selected = 0;    /* the selected row's kind (3 boon, 4 bane) */
uint32_t qol_chara_selected_at = 0; /* qol_chara_draws when that was seen */
uint32_t qol_chara_boon = 0;
uint32_t qol_chara_bane = 0;
uint32_t qol_chara_boon_text = 0;   /* the Boon row's value text (wchar_t[15], as the menu shows it) */
uint32_t qol_chara_bane_text = 0;   /* the Bane row's */

#define GAME_SKIP     0x006D8808u /* GameSkip* */
#define TUTORIAL      0x006D52D4u /* map::Intermediate::Tutorial*, active while +0x4B4 is 0 */
#define PAD           0x006BD96Cu /* the pad: +0x14 the buttons pressed this tick */
#define GAME_USER     0x006DA248u /* GameUserData* */
#define MY_UNIT_EDIT  0x006FEC10u /* title::ProcMyUnitEdit* */
#define SKIP_TRIGGER  0x004EB9FCu /* GameSkip::Trigger(void) */
#define BUTTON_START  0x8u
#define EVENT_RUNNING 0x006D42F8u /* event manager +8: the running ProcEvent */
#define EVENT_TYPE    0x5Cu       /* ProcEvent: its event::Function::Type */
#define SITUATION     0x006D7ED0u /* map::Situation*, null off a map */

#define SITUATION_PHASE 0x08u    /* map::Situation: the force whose phase it is (0 the player's) */
#define SITUATION_TURN  0x0Cu    /* map::Situation: the turn, from 1 (u16; Situation::TurnEnd) */

/* whether the event running is story Skip Story may skip: a story type, or
   any event off a map, or (qol3) the map's opening talk */
static int qol_story_event(Context *ctx) {
    uint32_t event = mem_read32(ctx, EVENT_RUNNING);
    if (!event)
        return 0;
    uint32_t type = mem_read8(ctx, event + EVENT_TYPE);
    if (type < 32 && ((*(volatile uint32_t *)&qol_story_types >> type) & 1))
        return 1;
    uint32_t situation = mem_read32(ctx, SITUATION);
    if (!situation)
        return 1;
    /* qol3: before the battle: turn 1, the player's phase, a MapLoad or
       Turn event (the conversation the map opens with). Later turns' talk,
       the enemy phase's, villages, Talk and boss quotes stay */
    return type < 32 && ((*(volatile uint32_t *)&qol_opening_types >> type) & 1)
        && mem_read16(ctx, situation + SITUATION_TURN) == 1 && mem_read8(ctx, situation + SITUATION_PHASE) == 0;
}
#define REPORT_BIT    0x100000u

/* the GameSkip object when Skip Story should start a skip now: it is on, a
   skippable part is open, no skip runs, no map tutorial is up (as Tick's
   own Start check) */
static uint32_t qol_idle_skip(Context *ctx) {
    if (!*(volatile uint32_t *)&qol_skip_story)
        return 0;
    uint32_t skip = mem_read32(ctx, GAME_SKIP);
    if (!skip || mem_read8(ctx, skip) != 0 || (mem_read32(ctx, skip + 4) & 3))
        return 0;
    uint32_t tutorial = mem_read32(ctx, TUTORIAL);
    if (tutorial && mem_read32(ctx, tutorial + 0x4B4) == 0 && !qol_skip_tutorial_maps)
        return 0;
    if (qol_choice_open || qol_dialog_open)
        return 0;
    return skip;
}

/* GameSkip::Trigger(skip) in place of the Tick the Persistent would make,
   a tail call: it returns to the Persistent's caller. The tick after, Tick
   runs as usual and takes the skip on. */
static void qol_trigger(Context *ctx, uint32_t skip) {
    qol_skips_started++;
    qol_skip_ours = 1;
    ctx->r[0] = skip;
    ctx->r[15] = SKIP_TRIGGER;
    ctx->thumb = 0;
    ctx->budget -= 4;
    CALL(recomp_call);
}

/* ChapterSequence::(anonymous)::ProcSequence::Persistent(void): the
   chapter's flow (openings, map events, endings, interludes) */
RECOMP_OVERRIDE(0x001DA1D8) {
    uint32_t skip = qol_idle_skip(ctx);
    if (skip && !qol_story_event(ctx))
        skip = 0;
    if (skip) {
        qol_trigger(ctx, skip);
        return;
    }
    CALL(RECOMP_ORIGINAL(0x001DA1D8));
}

/* (anonymous)::ProcSequence::Persistent(void): RelianceTalkSequence's
   proc, the support conversation viewer (RelianceTalkSequence::Create sets
   its +0x31 when it made the GameSkip). A support the player opens is not
   skipped unless qol_skip_supports. */
RECOMP_OVERRIDE(0x003CC458) {
    uint32_t skip = qol_skip_supports && mem_read8(ctx, ctx->r[0] + 0x31) ? qol_idle_skip(ctx) : 0;
    if (skip) {
        qol_trigger(ctx, skip);
        return;
    }
    CALL(RECOMP_ORIGINAL(0x003CC458));
}

/* castle::(anonymous)::JoinSeq::Persistent(void): a unit joining at My
   Castle, its scene */
RECOMP_OVERRIDE(0x0046AFBC) {
    uint32_t skip = qol_idle_skip(ctx);
    if (skip && mem_read32(ctx, SITUATION))
        skip = 0;
    if (skip) {
        qol_trigger(ctx, skip);
        return;
    }
    CALL(RECOMP_ORIGINAL(0x0046AFBC));
}

/* after a Persistent the Start given to it is taken back, so nothing else
   sees a press nobody made */
static void qol_start_taken(Context *ctx, const uint32_t *words) {
    mem_write32(ctx, words[0], mem_read32(ctx, words[0]) & ~BUTTON_START);
}

/* (anonymous)::ProcAction::Persistent(void): a 3D cutscene of a support
   conversation (RelianceTalkSequence's file; the story's own 3D cutscenes
   run inside events and are skipped with them). Once its
   countdown +0x58 reaches 0 it skips on Start (Fade::WhiteOut, then
   GameSkip::Trigger when white); the GameSkip it ticks is kept disabled
   meanwhile, so the press reaches only this. */
RECOMP_OVERRIDE(0x003CB690) {
    uint32_t self = ctx->r[0];
    if (*(volatile uint32_t *)&qol_skip_story && qol_skip_supports && qol_skip_cutscenes && mem_read32(ctx, self + 0x58) == 0) {
        uint32_t pad = mem_read32(ctx, PAD);
        if (pad && !(mem_read32(ctx, pad + 0x14) & BUTTON_START)) {
            mem_write32(ctx, pad + 0x14, mem_read32(ctx, pad + 0x14) | BUTTON_START);
            qol_cutscenes_skipped++;
            CALL_THEN(RECOMP_ORIGINAL(0x003CB690), qol_start_taken, pad + 0x14);
            return;
        }
    }
    CALL(RECOMP_ORIGINAL(0x003CB690));
}

/* the event dialogs: during a skip Skip Story started, the skip is asked to
   end and the dialog is built as if none ran (GameSkip's state byte reads 0
   while it is), so the player chooses */
static void qol_choice_built(Context *ctx, const uint32_t *words) {
    uint32_t skip = mem_read32(ctx, GAME_SKIP);
    if (skip && mem_read8(ctx, skip) == 0)
        mem_write8(ctx, skip, (uint8_t)words[0]);
}

/* the skip's state when a dialog must be shown rather than answered */
static uint32_t qol_choice_wanted(Context *ctx) {
    qol_dialog_open = 1;
    if (!*(volatile uint32_t *)&qol_skip_story || !qol_show_choices)
        return 0;
    uint32_t skip = mem_read32(ctx, GAME_SKIP);
    return skip ? mem_read8(ctx, skip) : 0;
}

static void qol_choice(Context *ctx, void (*original)(Context *), uint32_t state) {
    uint32_t skip = mem_read32(ctx, GAME_SKIP);
    qol_choices_shown++;
    /* escape: Tick fades back in and ends the skip. Mode 1 starts the next
       skip once the choice is made (the dialog's item calls below); mode 2
       also sets bit 1, as the script's own SkipEscape does, so the next
       skip waits for the script's next skippable part */
    mem_write32(ctx, skip + 4, mem_read32(ctx, skip + 4) | (qol_choice_mode == 2 ? 6 : 4));
    qol_choice_open = 1;
    mem_write8(ctx, skip, 0);
    CALL_THEN(original, qol_choice_built, state);
}

/* the recompiler finds overrides by their RECOMP_OVERRIDE marks in the
   text, so each is written out */
/* (anonymous)::RouteBranchDialog(CmContext*, title, a, b, c, default) */
RECOMP_OVERRIDE(0x003ABA98) {
    uint32_t state = qol_choice_wanted(ctx);
    if (state) {
        qol_choice(ctx, RECOMP_ORIGINAL(0x003ABA98), state);
        return;
    }
    CALL(RECOMP_ORIGINAL(0x003ABA98));
}

/* (anonymous)::DialogYesNo(CmContext*, text) */
RECOMP_OVERRIDE(0x003A785C) {
    uint32_t state = qol_choice_wanted(ctx);
    if (state) {
        qol_choice(ctx, RECOMP_ORIGINAL(0x003A785C), state);
        return;
    }
    CALL(RECOMP_ORIGINAL(0x003A785C));
}

/* (anonymous)::Dialog1 */
RECOMP_OVERRIDE(0x003AF6E8) {
    uint32_t state = qol_choice_wanted(ctx);
    if (state) {
        qol_choice(ctx, RECOMP_ORIGINAL(0x003AF6E8), state);
        return;
    }
    CALL(RECOMP_ORIGINAL(0x003AF6E8));
}

/* (anonymous)::Dialog2 */
RECOMP_OVERRIDE(0x003AF7F0) {
    uint32_t state = qol_choice_wanted(ctx);
    if (state) {
        qol_choice(ctx, RECOMP_ORIGINAL(0x003AF7F0), state);
        return;
    }
    CALL(RECOMP_ORIGINAL(0x003AF7F0));
}

/* (anonymous)::Dialog3 */
RECOMP_OVERRIDE(0x003AF92C) {
    uint32_t state = qol_choice_wanted(ctx);
    if (state) {
        qol_choice(ctx, RECOMP_ORIGINAL(0x003AF92C), state);
        return;
    }
    CALL(RECOMP_ORIGINAL(0x003AF92C));
}

/* (anonymous)::Dialog4 */
RECOMP_OVERRIDE(0x003AFA9C) {
    uint32_t state = qol_choice_wanted(ctx);
    if (state) {
        qol_choice(ctx, RECOMP_ORIGINAL(0x003AFA9C), state);
        return;
    }
    CALL(RECOMP_ORIGINAL(0x003AFA9C));
}

/* (anonymous)::Dialog5 */
RECOMP_OVERRIDE(0x003AFC40) {
    uint32_t state = qol_choice_wanted(ctx);
    if (state) {
        qol_choice(ctx, RECOMP_ORIGINAL(0x003AFC40), state);
        return;
    }
    CALL(RECOMP_ORIGINAL(0x003AFC40));
}

/* (anonymous)::Dialog6 */
RECOMP_OVERRIDE(0x003AFE14) {
    uint32_t state = qol_choice_wanted(ctx);
    if (state) {
        qol_choice(ctx, RECOMP_ORIGINAL(0x003AFE14), state);
        return;
    }
    CALL(RECOMP_ORIGINAL(0x003AFE14));
}

/* (anonymous)::Dialog7 */
RECOMP_OVERRIDE(0x003B0018) {
    uint32_t state = qol_choice_wanted(ctx);
    if (state) {
        qol_choice(ctx, RECOMP_ORIGINAL(0x003B0018), state);
        return;
    }
    CALL(RECOMP_ORIGINAL(0x003B0018));
}

/* (anonymous)::Dialog8 */
RECOMP_OVERRIDE(0x003B0248) {
    uint32_t state = qol_choice_wanted(ctx);
    if (state) {
        qol_choice(ctx, RECOMP_ORIGINAL(0x003B0248), state);
        return;
    }
    CALL(RECOMP_ORIGINAL(0x003B0248));
}

/* (anonymous)::MoviePlayImpl(CmContext*, name, bind): an event's movie */
RECOMP_OVERRIDE(0x003A8B80) {
    qol_event_movie = qol_story_event(ctx);
    CALL(RECOMP_ORIGINAL(0x003A8B80));
}

/* MovieViewer::PlayMovie(void): Extras' movie viewer */
RECOMP_OVERRIDE(0x0019FD84) {
    qol_event_movie = 0;
    CALL(RECOMP_ORIGINAL(0x0019FD84));
}

/* ProcMuseum::CreateMoviePlay(void): My Castle's library */
RECOMP_OVERRIDE(0x00189D0C) {
    qol_event_movie = 0;
    CALL(RECOMP_ORIGINAL(0x00189D0C));
}

/* (anonymous)::ProcSequence::PlayMovie(void): the title's opening movie */
RECOMP_OVERRIDE(0x0033B228) {
    qol_event_movie = 0;
    CALL(RECOMP_ORIGINAL(0x0033B228));
}

/* is::app::movie::MovieThread::MovieLeave(void), once a picture: an event
   movie not yet skipping (+0x6F0) gets Start for the check it makes (Start,
   or with +0x6F1 any of A/B/X/Y/Start), taken back after */
RECOMP_OVERRIDE(0x002247A8) {
    uint32_t self = ctx->r[0];
    if (*(volatile uint32_t *)&qol_skip_story && qol_event_movie && mem_read8(ctx, self + 0x6F0) == 0) {
        uint32_t pad = mem_read32(ctx, PAD);
        if (pad && !(mem_read32(ctx, pad + 0x14) & BUTTON_START)) {
            mem_write32(ctx, pad + 0x14, mem_read32(ctx, pad + 0x14) | BUTTON_START);
            qol_movies_skipped++;
            CALL_THEN(RECOMP_ORIGINAL(0x002247A8), qol_start_taken, pad + 0x14);
            return;
        }
    }
    CALL(RECOMP_ORIGINAL(0x002247A8));
}

/* the event dialogs' items and cancels: the choice is made */
static void qol_chosen(Context *ctx, uint32_t address) {
    qol_choice_open = 0;
    qol_dialog_open = 0;
    (void)ctx;
    (void)address;
}

/* (anonymous)::EventDialogItem::ACall(void), the items of Dialog1..8 and
   RouteBranchDialog */
RECOMP_OVERRIDE(0x003A9E0C) {
    qol_chosen(ctx, 0x003A9E0C);
    CALL(RECOMP_ORIGINAL(0x003A9E0C));
}

/* (anonymous)::EventDialogItemYes::ACall(void) */
RECOMP_OVERRIDE(0x003AC33C) {
    qol_chosen(ctx, 0x003AC33C);
    CALL(RECOMP_ORIGINAL(0x003AC33C));
}

/* (anonymous)::EventDialogItemNo::ACall(void) */
RECOMP_OVERRIDE(0x003AB770) {
    qol_chosen(ctx, 0x003AB770);
    CALL(RECOMP_ORIGINAL(0x003AB770));
}

/* (anonymous)::EventDialog::BCall(void) */
RECOMP_OVERRIDE(0x003A79E4) {
    qol_chosen(ctx, 0x003A79E4);
    CALL(RECOMP_ORIGINAL(0x003A79E4));
}

/* (anonymous)::EventDialogYesNo::BCall(void) */
RECOMP_OVERRIDE(0x003AAF20) {
    qol_chosen(ctx, 0x003AAF20);
    CALL(RECOMP_ORIGINAL(0x003AAF20));
}

/* link::BossSeq::EntryBind(ProcInst*): the SpotPass and gameplay-data
   reporting questions after avatar creation, answered No */
RECOMP_OVERRIDE(0x0041028C) {
    if (!*(volatile uint32_t *)&qol_skip_spotpass) {
        CALL(RECOMP_ORIGINAL(0x0041028C));
        return;
    }
    uint32_t user = mem_read32(ctx, GAME_USER);
    if (user)
        mem_write32(ctx, user + 0x28, mem_read32(ctx, user + 0x28) & ~REPORT_BIT);
    qol_spotpass_answered++;
    ctx->r[0] = 0;
    ctx->budget -= 4;
    RETURN_TO(ctx->r[14]);
}

/* title::CharaMenuItem::Draw(void): which row has the cursor, and the
   boon and bane chosen, for the host's stat panel */
RECOMP_OVERRIDE(0x00433F54) {
    uint32_t item = ctx->r[0];
    uint32_t menu = mem_read32(ctx, item + 0xC);
    qol_chara_draws++;
    uint32_t kind = mem_read32(ctx, item + 0x38);
    if (kind == 3)
        qol_chara_boon_text = item + 0x5C;
    else if (kind == 4)
        qol_chara_bane_text = item + 0x5C;
    if (menu && mem_read32(ctx, menu + 0x9C) == mem_read32(ctx, item + 0x2C)) {
        qol_chara_selected = kind;
        qol_chara_selected_at = qol_chara_draws;
    }
    uint32_t edit = mem_read32(ctx, MY_UNIT_EDIT);
    if (edit) {
        qol_chara_boon = mem_read8(ctx, edit + 0x52);
        qol_chara_bane = mem_read8(ctx, edit + 0x53);
    }
    CALL(RECOMP_ORIGINAL(0x00433F54));
}

/* ---- My Castle's menu: Return to Title, Exit to Desktop ---------------- */

#define CASTLE_MENU_RETURN 0x00476FB8u /* after bl BasicMenu::Create in ButlerSeq::MainMenu */
#define QOL_NEW            0x002FDC68u /* operator new(unsigned int) */
#define QOL_BASIC_CREATE   0x004FA880u
#define SAVE_ITEM_VTABLE   0x006172F4u /* castle::(anonymous)::MI_Save */
#define DIALOG_YES_NO      0x001E08ECu /* GameDialogYesNo::CreateBind(ProcInst*, const wchar_t*) */
#define MAIN_JUMP          0x001B741Cu /* MainSequence::Jump(Label) */
#define LABEL_TITLE        2u          /* title::ProcGameOver's label: back to the title */
#define QI_MARK  0x38u
#define QI_KIND  0x3Cu
#define QI_NAME  0x40u  /* wchar_t[24] */
#define QI_HELP  0x70u  /* wchar_t[64] */
#define QI_ASK   0xF0u  /* wchar_t[96] */
#define QI_SIZE  0x1B0u
#define QOL_MARK 0x4D4C4F51u /* "QOLM" */
#define QOL_LIST_ROOM 256u

enum { QOL_TITLE = 1, QOL_EXIT = 2, QOL_MAP_TITLE = 3, QOL_MAP_EXIT = 4 }; /* qol4: the map's */

/* qol4: the battle map's menu (map::CreateSystemMenu) */
#define MAP_MENU_RETURN    0x00368E8Cu /* after bl BasicMenu::Create in map::CreateSystemMenu */
#define TURN_END_VTABLE    0x006319A4u /* map::(anonymous)::TurnEnd: End, BasicMenuItem's own attribute */

static void qol_text(Context *ctx, uint32_t at, const char *text, uint32_t room) {
    uint32_t i = 0;
    for (; text[i] && i + 1 < room; i++)
        mem_write16(ctx, at + 2 * i, (uint16_t)(unsigned char)text[i]);
    mem_write16(ctx, at + 2 * i, 0);
}

static int qol_item(Context *ctx, uint32_t item) {
    if (!item)
        return 0;
    uint32_t vtable = mem_read32(ctx, item);
    return (vtable == SAVE_ITEM_VTABLE || vtable == TURN_END_VTABLE) && mem_read32(ctx, item + QI_MARK) == QOL_MARK; /* qol4: or the map's */
}

/* a thread's place in a guest call made from here, which returns into the
   override that made it: what it was (step >> 8), its count, the return
   address to go back to, and two words */
typedef struct {
    uint32_t tls, lr, a, b, step;
    uint32_t map; /* qol4: the map's menu, not My Castle's */
} QolCall;
static QolCall qol_calls[8];

enum { CALL_MENU = 1, CALL_ASK = 2, CALL_TITLE = 3 };

static QolCall *qol_call_find(Context *ctx, uint32_t kind) {
    for (unsigned i = 0; i < sizeof qol_calls / sizeof *qol_calls; i++)
        if (qol_calls[i].lr && qol_calls[i].tls == ctx->tls && qol_calls[i].step >> 8 == kind)
            return &qol_calls[i];
    return 0;
}

static QolCall *qol_call_new(Context *ctx, uint32_t kind) {
    for (unsigned i = 0; i < sizeof qol_calls / sizeof *qol_calls; i++)
        if (!qol_calls[i].lr) {
            qol_calls[i].tls = ctx->tls;
            qol_calls[i].step = kind << 8;
            return &qol_calls[i];
        }
    return 0;
}

/* the guest calls qol_menu_create makes (CALL returns from a void function) */
static void qol_menu_new(Context *ctx) {
    /* operator new(QI_SIZE), returning into BasicMenu::Create */
    ctx->r[0] = QI_SIZE;
    ctx->r[14] = QOL_BASIC_CREATE;
    ctx->r[15] = QOL_NEW;
    ctx->thumb = 0;
    CALL(recomp_call);
}

static void qol_menu_original(Context *ctx) {
    CALL(RECOMP_ORIGINAL(0x004FA880));
}

/* BasicMenu::Create(BasicMenuItem**), asked by overrides/mods.c's override
   first: 1 when this took the call (My Castle's menu, the items added). */
int qol_menu_create(Context *ctx) {
    QolCall *add = qol_call_find(ctx, CALL_MENU);
    if (!add) {
        /* qol4: or the battle map's menu */
        int castle = ctx->r[14] == CASTLE_MENU_RETURN && *(volatile uint32_t *)&qol_castle_items;
        int map = ctx->r[14] == MAP_MENU_RETURN && *(volatile uint32_t *)&qol_map_items;
        if ((!castle && !map) || !(add = qol_call_new(ctx, CALL_MENU)))
            return 0;
        add->lr = ctx->r[14];
        add->a = ctx->r[0];
        add->b = ctx->r[1];
        add->map = map;
    } else {
        /* back from operator new: the item in r0 */
        uint32_t item = ctx->r[0], kind = (add->step & 0xFF) + 1;
        if (item) {
            for (uint32_t at = 0; at < QI_SIZE; at += 4)
                mem_write32(ctx, item + at, 0);
            mem_write32(ctx, item, add->map ? TURN_END_VTABLE : SAVE_ITEM_VTABLE); /* qol4 */
            mem_write32(ctx, item + QI_MARK, QOL_MARK);
            mem_write32(ctx, item + QI_KIND, add->map ? kind + 2 : kind); /* qol4: QOL_MAP_TITLE, QOL_MAP_EXIT */
            if (kind == QOL_TITLE) {
                qol_text(ctx, item + QI_NAME, "Return to Title", 24);
                qol_text(ctx, item + QI_HELP, "Return to the title screen.", 64);
                qol_text(ctx, item + QI_ASK, "Return to the title screen?\nAnything you haven't saved will be lost.", 96);
            } else {
                qol_text(ctx, item + QI_NAME, "Exit to Desktop", 24);
                qol_text(ctx, item + QI_HELP, "Close the game and return to the desktop.", 64);
                qol_text(ctx, item + QI_ASK, "Exit to the desktop?\nAnything you haven't saved will be lost.", 96);
            }
            /* before the last item (Next Battle; qol4: on a map, End), so
               the menu's own wrap-around from the top still lands on it */
            uint32_t count = 0;
            while (count + 1 < QOL_LIST_ROOM && mem_read32(ctx, add->b + 4 * count))
                count++;
            if (count && count + 1 < QOL_LIST_ROOM) {
                uint32_t last = mem_read32(ctx, add->b + 4 * (count - 1));
                mem_write32(ctx, add->b + 4 * (count - 1), item);
                mem_write32(ctx, add->b + 4 * count, last);
                mem_write32(ctx, add->b + 4 * (count + 1), 0);
            }
        }
        add->step++;
    }
    ctx->budget -= 8;
    /* qol5b: the map's menu gets Return to Title only (the user: Exit to
       Desktop stays in the options, the Main Menu and My Castle's menu) */
    if ((add->step & 0xFF) < (add->map ? 1u : 2u)) {
        qol_menu_new(ctx);
        return 1;
    }
    /* the game's own Create with the longer list */
    ctx->r[0] = add->a;
    ctx->r[1] = add->b;
    ctx->r[14] = add->lr;
    add->lr = 0;
    ctx->r[15] = QOL_BASIC_CREATE;
    ctx->thumb = 0;
    qol_menu_original(ctx);
    return 1;
}

/* castle::(anonymous)::MI_Save::GetName(void) */
RECOMP_OVERRIDE(0x00476BA8) {
    if (!qol_item(ctx, ctx->r[0])) {
        CALL(RECOMP_ORIGINAL(0x00476BA8));
        return;
    }
    ctx->r[0] += QI_NAME;
    ctx->budget -= 4;
    RETURN_TO(ctx->r[14]);
}

/* castle::(anonymous)::MI_Save::GetHelp(void) */
RECOMP_OVERRIDE(0x00476B8C) {
    if (!qol_item(ctx, ctx->r[0])) {
        CALL(RECOMP_ORIGINAL(0x00476B8C));
        return;
    }
    ctx->r[0] += QI_HELP;
    ctx->budget -= 4;
    RETURN_TO(ctx->r[14]);
}

/* castle::(anonymous)::MI_Save::ACall(void): for ours, the game's Yes/No
   bound to the menu's proc (as Save's own file menu is), then the decide
   sound with the menu kept, as Save answers */
static int qol_asking(Context *ctx, uint32_t item) {
    return qol_call_find(ctx, CALL_ASK) || (qol_item(ctx, item) && !(mem_read32(ctx, item + 0x34) & 2));
}

/* qol4: shared by My Castle's items and the map's: self is the ACall the
   question returns into */
static void qol_ask(Context *ctx, uint32_t self) {
    QolCall *ask = qol_call_find(ctx, CALL_ASK);
    if (ask) {
        /* back from GameDialogYesNo::CreateBind */
        ctx->r[14] = ask->lr;
        ask->lr = 0;
        /* My Castle's: as Save answers; qol4: the map's, as End answers
           when it asks (the decide sound, the menu kept under the question) */
        ctx->r[0] = ask->map ? 0x100 : 0x108;
        ctx->budget -= 4;
        RETURN_TO(ctx->r[14]);
    }
    uint32_t item = ctx->r[0];
    if (!(ask = qol_call_new(ctx, CALL_ASK))) {
        /* no room: the decide sound, and nothing asked */
        ctx->r[0] = 0x100;
        RETURN_TO(ctx->r[14]);
    }
    ask->lr = ctx->r[14];
    qol_confirm = mem_read32(ctx, item + QI_KIND);
    /* qol4: the map's question is bound to the map menu itself, as End's
       own question is (so No and B go back to the menu) */
    ask->map = qol_confirm >= QOL_MAP_TITLE;
    ctx->r[0] = ask->map ? mem_read32(ctx, item + 0xC) : mem_read32(ctx, mem_read32(ctx, item + 0xC) + 0x10);
    ctx->r[1] = item + QI_ASK;
    ctx->r[14] = self;
    ctx->r[15] = DIALOG_YES_NO;
    ctx->thumb = 0;
    ctx->budget -= 8;
    CALL(recomp_call);
}

RECOMP_OVERRIDE(0x00476B30) {
    if (!qol_asking(ctx, ctx->r[0])) {
        CALL(RECOMP_ORIGINAL(0x00476B30));
        return;
    }
    qol_ask(ctx, 0x00476B30u);
}

/* qol4: map::(anonymous)::TurnEnd::GetName, GetHelp and ACall, for the
   map menu's two items; End itself runs the game's own */
RECOMP_OVERRIDE(0x00379230) {
    if (!qol_item(ctx, ctx->r[0])) {
        CALL(RECOMP_ORIGINAL(0x00379230));
        return;
    }
    ctx->r[0] += QI_NAME;
    ctx->budget -= 4;
    RETURN_TO(ctx->r[14]);
}

RECOMP_OVERRIDE(0x00379218) {
    if (!qol_item(ctx, ctx->r[0])) {
        CALL(RECOMP_ORIGINAL(0x00379218));
        return;
    }
    ctx->r[0] += QI_HELP;
    ctx->budget -= 4;
    RETURN_TO(ctx->r[14]);
}

RECOMP_OVERRIDE(0x00378FE8) {
    if (!qol_asking(ctx, ctx->r[0])) {
        CALL(RECOMP_ORIGINAL(0x00378FE8));
        return;
    }
    qol_ask(ctx, 0x00378FE8u);
}

/* GameDialogItemYes::ACall(void): our question answered Yes.
   Return to Title leaves My Castle the way the castle's own exits do and
   sends the game's main sequence to the title, as a game over does:
   - the castle's result is set to 0x0C (castle::MainSeq::SetResult, its
     +0x98/+0x9C; 0x0C is the result the castle's link menu sets before it
     sends the main sequence elsewhere, ProcLink::PrepareVersus);
   - MainSequence::Jump(2), title::ProcGameOver's way to the title;
   - ProcInst::JumpImm(FreeMoveSequence, 6), MI_BackToOwnCastleYes's way to
     end the free roam, so the castle runs its exit.
   Then Yes answers as it always does (the dialog closes). */
#define CASTLE_MAIN   0x006D1B70u /* castle::MainSeq* */
#define FREE_MOVE     0x006D2A10u /* castle::FreeMoveSequence* */
#define PROC_JUMP_IMM 0x004EE200u /* ProcInst::JumpImm(int) */

static void qol_yes_call(Context *ctx, uint32_t target) {
    ctx->r[14] = 0x00202CDCu;
    ctx->r[15] = target;
    ctx->thumb = 0;
    ctx->budget -= 8;
    CALL(recomp_call);
}

RECOMP_OVERRIDE(0x00202CDC) {
    QolCall *jump = qol_call_find(ctx, CALL_TITLE);
    if (!jump) {
        uint32_t kind = qol_confirm;
        qol_confirm = 0;
        if (kind == QOL_EXIT || kind == QOL_MAP_EXIT) /* qol4: or the map's */
            qol_quit_request = 1;
        if (kind == QOL_MAP_TITLE) /* qol4: the game's own soft reset, at its next chance */
            qol_soft_reset = 1;
        if (kind != QOL_TITLE || !(jump = qol_call_new(ctx, CALL_TITLE))) {
            CALL(RECOMP_ORIGINAL(0x00202CDC));
            return;
        }
        jump->lr = ctx->r[14];
        jump->a = ctx->r[0];
        uint32_t castle = mem_read32(ctx, CASTLE_MAIN);
        if (castle) {
            mem_write8(ctx, castle + 0x98, 0x0C);
            mem_write32(ctx, castle + 0x9C, 0xFFFFFFFFu);
        }
        /* MainSequence::Jump(2) */
        ctx->r[0] = LABEL_TITLE;
        qol_yes_call(ctx, MAIN_JUMP);
        return;
    }
    if ((jump->step & 0xFF) == 0) {
        jump->step++;
        uint32_t free_move = mem_read32(ctx, FREE_MOVE);
        if (free_move) {
            /* ProcInst::JumpImm(free roam, 6) */
            ctx->r[0] = free_move;
            ctx->r[1] = 6;
            qol_yes_call(ctx, PROC_JUMP_IMM);
            return;
        }
    }
    /* Yes's own answer */
    ctx->r[0] = jump->a;
    ctx->r[14] = jump->lr;
    jump->lr = 0;
    CALL(RECOMP_ORIGINAL(0x00202CDC));
}

/* GameDialogItemNo::ACall(void) (B on a Yes/No answers it too) */
RECOMP_OVERRIDE(0x001EBA84) {
    qol_confirm = 0;
    CALL(RECOMP_ORIGINAL(0x001EBA84));
}

/* ---- qol3: the title screen's logo, always plain "Fire Emblem Fates" ---- */

/* Title::Create(ProcInst*) builds the title screen ("Press any button"):
   its logo is the effect Tlp_Title_A (Birthright), _B (Conquest), _C
   (Revelation) by the route last played (GameUserGlobalData +0x14: 0, 1,
   2), and Tlp_Title_P (the plain Fates logo, the one the game shows before
   any route) for anything else. With qol_title_plain the byte reads 0xFF
   while Create runs, so it picks Tlp_Title_P, and is put back after (the
   byte is the saves' own; nothing else reads it on the way). The game's
   own art, no painting over it. */
#define GLOBAL_USER   0x006DA27Cu /* GameUserGlobalData* (GameUserGlobalData::Get) */
#define LAST_ROUTE    0x14u
uint32_t qol_title_plain = 0;       /* the host sets it (FATES_QOL_TITLE_PLAIN=0 leaves the game's choice) */
uint32_t qol_title_plain_shown = 0; /* times the plain logo was asked for in place of a route's */

static void qol_title_route_back(Context *ctx, const uint32_t *words) {
    mem_write8(ctx, words[0] + LAST_ROUTE, (uint8_t)words[1]);
}

RECOMP_OVERRIDE(0x00420B74) {
    uint32_t global = mem_read32(ctx, GLOBAL_USER);
    if (!*(volatile uint32_t *)&qol_title_plain || !global) {
        CALL(RECOMP_ORIGINAL(0x00420B74));
        return;
    }
    uint32_t route = mem_read8(ctx, global + LAST_ROUTE);
    if (route <= 2)
        qol_title_plain_shown++;
    mem_write8(ctx, global + LAST_ROUTE, 0xFF);
    CALL_THEN(RECOMP_ORIGINAL(0x00420B74), qol_title_route_back, global, route);
}

/* ---- qol4: the game's own soft reset ---------------------------------- */

#define HOME_STATE        0x006BD8E8u /* HomeButton's state: +1, a soft reset is allowed now */
#define RESTART_APP       0x00107AFCu /* nn::applet::CTR::detail::RestartApplication(const void*, unsigned int) */

/* HomeButton::Tick(void): Return to Title asked (qol_soft_reset) is
   L+R+Start without the keys: when the game allows it now, its own call,
   RestartApplication(0, 0x300), as the Tick makes it (a tail call; the
   Tick returns 0 after it, as RestartApplication's 0 here). Otherwise the
   game's Tick, and the request waits for a frame that allows it. */
RECOMP_OVERRIDE(0x001045B0) {
    if (!*(volatile uint32_t *)&qol_soft_reset || !mem_read8(ctx, HOME_STATE + 1)) {
        CALL(RECOMP_ORIGINAL(0x001045B0));
        return;
    }
    qol_soft_reset = 0;
    ctx->r[0] = 0;
    ctx->r[1] = 0x300;
    ctx->r[15] = RESTART_APP;
    ctx->thumb = 0;
    ctx->budget -= 8;
    CALL(recomp_call);
}

/* nn::applet::CTR::detail::RestartApplication(const void*, unsigned int):
   the console closes the game and starts it again. The host does that on
   PC (qol_restart_request); the game goes on until it does, a frame at
   most, with the answer a successful restart request gets */
RECOMP_OVERRIDE(0x00107AFC) {
    qol_restart_request = 1;
    qol_restarts++;
    ctx->r[0] = 0;
    ctx->budget -= 4;
    RETURN_TO(ctx->r[14]);
}

/* ---- qol4: no "Loading extra data..." at each start ------------------- */

#define SLEEP_HOME_BIND   0x0010B5E0u /* HomeButton::SleepHomeBind(void) */
#define SLEEP_HOME_UNBIND 0x0010B620u /* HomeButton::SleepHomeUnbind(void) */

/* link::(anonymous)::ProcExtRead::Begin(void): the HOME button held off,
   and (left out here) CommonWaitMessage::Open(MID_EXT_READ) */
RECOMP_OVERRIDE(0x003FEA50) {
    if (!*(volatile uint32_t *)&qol_ext_read_quiet) {
        CALL(RECOMP_ORIGINAL(0x003FEA50));
        return;
    }
    qol_ext_read_hidden++;
    ctx->r[15] = SLEEP_HOME_BIND;
    ctx->thumb = 0;
    ctx->budget -= 4;
    CALL(recomp_call);
}

/* link::(anonymous)::ProcExtRead::End(void): the HOME button let go, and
   (left out here, as the message never opened) CommonWaitMessage::Close */
RECOMP_OVERRIDE(0x003FE9EC) {
    if (!*(volatile uint32_t *)&qol_ext_read_quiet) {
        CALL(RECOMP_ORIGINAL(0x003FE9EC));
        return;
    }
    ctx->r[15] = SLEEP_HOME_UNBIND;
    ctx->thumb = 0;
    ctx->budget -= 4;
    CALL(recomp_call);
}
