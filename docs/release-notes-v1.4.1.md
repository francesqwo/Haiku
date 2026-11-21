# Haiku 1.4.1

Haiku 1.4.1 smooths battles, 3D cutscenes and the title screen at 60 FPS too, plays mods' HD movies at full speed (#7), and fixes the menus freezing the game (#14), the Enemy Targeting Lines not following the unit you move (#12), the ground sliding as a battle starts and ends at high frame rates (#1), and the first-time code build that looked stuck (#13).

Haiku is a native Windows port of Fire Emblem Fates. The game's code is statically recompiled to x86-64 and runs on native PC code for graphics (Vulkan), audio, files, saves and input. Nothing is emulated. See the [README](https://github.com/tyle-beep/Haiku#features) for the full feature list.

**This release contains no game files.** You need your own legally obtained copy of Fire Emblem Fates Special Edition (US, European or Japanese), dumped and decrypted by you. Haiku builds the game's code from it on your PC.

## Downloads

| File | Size | What it is |
|---|---|---|
| `Haiku.zip` | 48.4 MB (48,360,781 bytes) | Haiku: `Haiku.exe` and everything it needs to build the game's code from your dump. Extract it and run `Haiku.exe`. |
| `Haiku.zip.sig` | 64 bytes | The signature of `Haiku.zip`, which Haiku's in-game updater checks. You don't need it to install by hand. |
| `Haiku-toolchain-source-w64devkit-2.11.0.zip` | 152.2 MB (152,193,966 bytes) | The source code of the bundled compiler (GCC 16.2.0, GNU Binutils 2.47, GMP, MPFR, MPC and MinGW-w64 v14.0.0, as w64devkit 2.11.0 builds them), offered under the GNU GPL version 3. Unchanged from 1.0. You don't need it to play. |

SHA-256:

```
76fbbec12c366c21ac43d77592da6b699aafcd10a1ce2958b545f1259f7d7771  Haiku.zip
2c9acf323c86b948c6fec9cba11158ce011d8edd2ce8f55b357f1d730080a557  Haiku-toolchain-source-w64devkit-2.11.0.zip
```

## Updating from 1.4

**In the game:** when Haiku has checked for updates (once a day), the Main Menu shows "Haiku 1.4.1 is available". Press **U** on the keyboard or **Y** on the controller, or click it, then choose **Install**. Haiku downloads it, checks it, puts it in place and starts again by itself.

**By hand**, if you turned the update check off or don't want to wait for it:

1. Close Haiku.
2. Extract the new `Haiku.zip` over your Haiku folder, replacing the files.
3. Run `Haiku.exe`.

**The first start after updating builds the game's code once more**, because this release changes some of the game's functions (for the HD movies). Haiku does it by itself in its window, showing its progress, and then the game starts. Only the few parts that changed are compiled, so it takes a minute or two at most (22 seconds on our test PC). If you have more than one version of the game installed (US, European, Japanese), each one does this once, the first time you play it. Your saves, settings, mods and Japanese voices are kept as they are.

**From 1.3 or older:** those versions can't update themselves, so update by hand as above. From 1.2, 1.1 or 1.0 the first start also removes the old `Haiku Setup.exe`, as the 1.3 notes describe.

## What's new

### Smoother battles at 60 FPS

- **Battles, battle transitions, 3D cutscenes and the title screen are now smoothed at 60 FPS too.** The game itself runs these scenes at 30 FPS (on the 3DS as well). Haiku already drew a picture in between each two of the game's above 60 FPS; now it does at 60 FPS as well (Frame Rate 60, a 60 Hz monitor, or an FPS cap of 60), so those scenes show 60 new pictures a second. The game's speed is unchanged, and the map and menus are as before.
- **Options > Display > Smooth 30 FPS Scenes** turns it off, on by default. Smoothing shows those scenes about 25 ms later; turn it off to see the game's own 30 FPS there. Above 60 FPS every scene is smoothed, as before.
- On a PC that can't keep up, Haiku stops smoothing by itself so the game keeps its normal speed.

### HD movies for mods (#7)

- **Mods' MOFLEX movies up to 1024x1024 now play at full speed**, for example 1024x576. Before, a 1024x576 movie could play at a few pictures a second with the widescreen side panels on.
- **A movie Haiku can't play** (over 1024 pixels wide or high, such as 1920x1080) **is skipped** instead of leaving a black screen, with a line in `haiku.log` saying why.
- The game's own movies look exactly as before.

### The first-time code build (#13)

- **The progress screen updates every second**: which file is compiling and for how long, how many are done, and, after a minute, about how long is left. It no longer looks stuck while a slow file compiles.
- **It uses your PC's memory better**: on a PC with little free memory it now compiles several files at once instead of one, and the slowest file starts first instead of holding up the rest.
- **Closing it no longer loses the work**: the next start carries on from the files already compiled.
- **A compiler that stops doing anything** (for example held up by antivirus) is restarted after 5 minutes. A file that runs out of memory is compiled again on its own, with fewer optimisations if it must; the game plays the same.
- `setup-work\setup.log` now records each file's time and memory, and every minute what is running.

## Fixes

- **Opening the Haiku menu or the Mods screen no longer freezes the game** (#14). Each time, Haiku used to read your whole mods folder, your display modes and your sound devices before drawing the next picture: with large mods, a slow drive or antivirus, that could take seconds. It now happens in the background and the menu opens at once.
- **Overlays such as Steam's no longer make Haiku rebuild its display surface every frame** (#14).
- **A picture that takes over 50 ms to show is noted in `haiku.log`**, with where the time went, so a report with a log can say what happened.
- **The Enemy Targeting Lines follow the unit you're moving again, on every map** (#12). In 1.4, if working out the enemy's moves once took a little too long (a slower PC, a code mod, or a background hiccup), Haiku stopped doing it for each tile you hovered, often for the rest of the session, so the lines only showed after a move. Now they are worked out for every tile on every map; on slower PCs Haiku waits until the cursor stops on a tile and spreads the work over several frames, so it doesn't cost frame rate.
- **The ground no longer slides and snaps back as a battle starts and ends** at high frame rates (#1). The pictures Haiku draws between the game's own placed the map's ground slightly off while the camera began to turn. They now lie exactly between the game's pictures, at any aspect ratio.
- **Memory no longer grows while a movie plays.** Every movie picture used to stay cached: about 48 MB a second during the game's opening movie, more for an HD one.

## Known issues

- **Movies over 1024 pixels wide or high (1080p) can't be played.** The game's own movie decoder stops at 1024; such a movie is skipped. Use 1024x576 at most for 16:9.
- **16:9 movies are squashed slightly**: the game draws every movie over the whole 5:3 top screen. For 16:9 footage, letterbox it into **960x576** (exactly 5:3).
- **Picking a mod's other code patch** can only be done in `games\<version>\mods.toml` (for example `code_patch = 1` in the mod's entry). The Mods screen has no control for it yet.
- **Not checked in game:** Nyx Veil Fix's veil, Unofficial Gay Fates' paralogues and endings, and Playable Lilith's Lilith herself (she joins late in the story). These mods load, but we didn't reach those parts.
- **Haiku's compiler isn't offered for code mods** if your Haiku folder's path has letters outside plain English (A to Z). Zig still works there.
- With the Uncensored Patch, a save state taken during its subtitled opening movie doesn't resume. Save before or after the movie.
- Under a translation, the same-gender S-supports use general rules rather than hand-checked edits, so a line may keep a word for the wrong gender.
- Team If's translation has been tested less than the Uncensored Patch.
- The Steam overlay itself couldn't be tested for #14. If a menu still stutters with an overlay on, please attach `userdata\haiku\haiku.log` to the issue.
- If the first-time build still seems stuck on your PC (#13), please attach `setup-work\setup.log` from your Haiku folder to the issue.
- Still being looked into: #3 (stuck on the Initializing screen).

## Notes

- Requirements: Windows 10 or 11 (64-bit), a GPU with Vulkan, and about 3 GB of disk space for each version you install.
- Haiku only goes online for online battles, for the update check (which can be turned off in Options > Interface > Check for Updates), or if you choose to download Zig for code mods. Haiku's own compiler needs no download, and Haiku Setup never uses the network.

## Legal

Haiku is an unofficial fan project, not affiliated with or endorsed by Nintendo or Intelligent Systems. Fire Emblem and Fire Emblem Fates are trademarks of Nintendo / Intelligent Systems. Haiku is MIT-licensed and provided as is, without warranty. The bundled compiler is GPL-3.0; its source is the third download above. Fan translations and other mods belong to their authors and are not part of Haiku.
