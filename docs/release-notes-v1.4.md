# Haiku 1.4

Haiku 1.4 can update itself. From this release on, a new version of Haiku is offered in the game, checked and installed for you, with nothing to download by hand. 1.4 also makes the Enemy Targeting Lines work with V.O.W Edition, fixes rare long freezes when moving a unit (#12), and makes more mods work.

Haiku is a native Windows port of Fire Emblem Fates. The game's code is statically recompiled to x86-64 and runs on native PC code for graphics (Vulkan), audio, files, saves and input. Nothing is emulated. See the [README](https://github.com/tyle-beep/Haiku#features) for the full feature list.

**This release contains no game files.** You need your own legally obtained copy of Fire Emblem Fates Special Edition (US, European or Japanese), dumped and decrypted by you. Haiku builds the game's code from it on your PC.

## Downloads

| File | Size | What it is |
|---|---|---|
| `Haiku.zip` | 48.3 MB (48,263,388 bytes) | Haiku: `Haiku.exe` and everything it needs to build the game's code from your dump. Extract it and run `Haiku.exe`. |
| `Haiku.zip.sig` | 64 bytes | The signature of `Haiku.zip`, which Haiku's in-game updater checks. You don't need it to install by hand. |
| `Haiku-toolchain-source-w64devkit-2.11.0.zip` | 152.2 MB (152,193,966 bytes) | The source code of the bundled compiler (GCC 16.2.0, GNU Binutils 2.47, GMP, MPFR, MPC and MinGW-w64 v14.0.0, as w64devkit 2.11.0 builds them), offered under the GNU GPL version 3. Unchanged from 1.0. You don't need it to play. |

SHA-256:

```
d9bb8f0f313cccac074a1b621796086c8a07029959454ef6d76e0bdd542599b4  Haiku.zip
2c9acf323c86b948c6fec9cba11158ce011d8edd2ce8f55b357f1d730080a557  Haiku-toolchain-source-w64devkit-2.11.0.zip
```

## Updating from 1.3

Haiku 1.3 can't update itself, so this one time, update by hand, the usual way:

1. Close Haiku.
2. Extract the new `Haiku.zip` over your Haiku folder, replacing the files.
3. Run `Haiku.exe`.

The game's code you already built is kept, so the game starts straight away. Your saves, settings, mods and Japanese voices are kept as they are.

**From 1.4 on, updates come in the game** (below): no more extracting by hand.

From 1.2, 1.1 or 1.0, the steps are the same. The first start then builds the game's code once from your installed game and removes the old `Haiku Setup.exe`, as the 1.3 notes describe.

## What's new

### Haiku updates itself

- **Haiku checks for a new release once a day**, in the background, while you play.
- **When there is one, a notice shows on the game's Main Menu**: "Haiku 1.4.1 is available". Press **U** on the keyboard or **Y** on the controller, or click it.
- **It opens the release's notes**, in the game's own look, with three choices:
  - **Install** downloads the new Haiku, checks it, puts it in place and starts it again by itself.
  - **Later** reminds you the next time Haiku starts.
  - **Skip This Version** stops the notices for that version. You'll hear of the version after it.
- **Every download is checked twice** before anything changes: against the checksum GitHub publishes, and against the signature made with Haiku's own release key. A download that fails either check is never installed.
- **Your saves, settings, mods and game files are never touched.** Only Haiku's own files are replaced, and if anything goes wrong while they are being put in place, the old ones are put back. You can cancel while it downloads.
- **The update check is the only time Haiku goes online outside online battles** (and the Zig download for code mods, if you choose it). It asks GitHub for the latest release and sends nothing about you or your PC, only which version of Haiku it is. Turn it off in **Options > Interface > Check for Updates**.

### V.O.W Edition

- **The Enemy Targeting Lines now work with V.O.W Edition.** Before, they never showed with it, and selecting or moving a unit on the map could freeze the game for about half a second each time.
- **Code mods that change many of the game's functions run a little faster**, V.O.W Edition among them: Haiku now finds the mod's version of a function at once instead of searching for it.

### Mods

- **These mods now work:** Fire Emblem Fates+ 3.5 (with its code patch), Playable Lilith 4.0.1 and Abyssal Bloodline 1.2.1. Fates Icon Project works too.
- **Files that aren't part of a mod are left out:** `__MACOSX` folders, `.DS_Store`, `Thumbs.db`, `desktop.ini` and shortcuts (`.lnk`). A mod zipped on a Mac now works.
- **A code patch in a folder of its own beside `romfs`** (like Fates+'s `Code Patch\code.ips`) is found and used.
- **A US mod with a few Japanese-named files is no longer mistaken for a Japanese fan translation.**

## Fixes

- **Fixed rare freezes of 10 to 15 seconds when hovering over or moving a unit**, with `haiku.log` filling with "access to unmapped guest address" lines (#12). The Enemy Targeting Lines' look ahead at the enemy's moves could go wrong and run for seconds. It now stops at the first sign of trouble.
- **`haiku.log` no longer fills up with "unmapped guest address" warnings.** The first few are named, then one line a minute says how many more there were.

## Known issues

- **Picking a mod's other code patch** can only be done in `games\<version>\mods.toml` (for example `code_patch = 1` in the mod's entry). The Mods screen has no control for it yet.
- **Not checked in game:** Nyx Veil Fix's veil, Unofficial Gay Fates' paralogues and endings, and Playable Lilith's Lilith herself (she joins late in the story). These mods load, but we didn't reach those parts.
- **Haiku's compiler isn't offered for code mods** if your Haiku folder's path has letters outside plain English (A to Z). Zig still works there.
- With the Uncensored Patch, a save state taken during its subtitled opening movie doesn't resume. Save before or after the movie.
- Under a translation, the same-gender S-supports use general rules rather than hand-checked edits, so a line may keep a word for the wrong gender.
- Team If's translation has been tested less than the Uncensored Patch.
- Still being looked into: #3 (stuck on the Initializing screen) and #1 (camera judder entering and leaving battles).

## Notes

- Requirements: Windows 10 or 11 (64-bit), a GPU with Vulkan, and about 3 GB of disk space for each version you install.
- Haiku only goes online for online battles, for the update check (which can be turned off), or if you choose to download Zig for code mods. Haiku's own compiler needs no download, and Haiku Setup never uses the network.

## Legal

Haiku is an unofficial fan project, not affiliated with or endorsed by Nintendo or Intelligent Systems. Fire Emblem and Fire Emblem Fates are trademarks of Nintendo / Intelligent Systems. Haiku is MIT-licensed and provided as is, without warranty. The bundled compiler is GPL-3.0; its source is the third download above. Fan translations and other mods belong to their authors and are not part of Haiku.
