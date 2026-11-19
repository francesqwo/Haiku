# Haiku 1.3

Haiku 1.3 is one program, `Haiku.exe`. It sets up, updates and plays the game. The game's code is now built once on your PC, so most future updates need no recompiling. The US, European and Japanese versions can be installed side by side and switched in the options, and each version keeps its own language and voices.

Haiku is a native Windows port of Fire Emblem Fates. The game's code is statically recompiled to x86-64 and runs on native PC code for graphics (Vulkan), audio, files, saves and input. Nothing is emulated. See the [README](https://github.com/tyle-beep/Haiku#features) for the full feature list.

**This release contains no game files.** You need your own legally obtained copy of Fire Emblem Fates Special Edition (US, European or Japanese), dumped and decrypted by you. Haiku builds the game's code from it on your PC.

## Downloads

| File | Size | What it is |
|---|---|---|
| `Haiku.zip` | 48.1 MB (48,087,603 bytes) | Haiku: `Haiku.exe` and everything it needs to build the game's code from your dump. Extract it and run `Haiku.exe`. |
| `Haiku-toolchain-source-w64devkit-2.11.0.zip` | 152.2 MB (152,193,966 bytes) | The source code of the bundled compiler (GCC 16.2.0, GNU Binutils 2.47, GMP, MPFR, MPC and MinGW-w64 v14.0.0, as w64devkit 2.11.0 builds them), offered under the GNU GPL version 3. Unchanged from 1.0. You don't need it to play. |

SHA-256:

```
5d3a04e8c6905c380b361ee62cc146849c6efdfed4270a4162b17eec893e49bf  Haiku.zip
2c9acf323c86b948c6fec9cba11158ce011d8edd2ce8f55b357f1d730080a557  Haiku-toolchain-source-w64devkit-2.11.0.zip
```

## Updating from 1.2, 1.1 or 1.0

1. Close Haiku.
2. Extract the new `Haiku.zip` over your Haiku folder, replacing the files.
3. Run `Haiku.exe` (not `Haiku Setup.exe`: Haiku 1.3 removes it).

The first start after updating builds the game's code once, from the game files already installed. It takes about as long as your first build did (about 10 to 30 minutes), and Haiku shows it in the game's own window. Your dump isn't needed. Then the game starts by itself.

What the update changes in your folder:

- **`Haiku Setup.exe` is removed**, with the parts of `setup` only it used (`setup\engine`, `setup\overrides`, `setup\eur`, `setup\jpn`). Its log moves to `setup-work\setup-1.x.log`. Haiku Setup is now part of `Haiku.exe`: Options > Interface > Haiku Setup opens it.
- **Your game moves into `games\USA`** (or `games\EUR`, `games\JPN`, for the version you had): its `data` and `assets` folders, your `mods` folder and its list (`mods.toml`), and the caches. Each folder is moved as it is, not copied.
- **Your saves and settings stay in `userdata\haiku`**. Your mods and Japanese voices are kept as they are.

## What's new

### One program, and updates without recompiling

- **`Haiku.exe` is the only program.** It is the engine, Haiku's own changes to the game and Haiku Setup. It holds no part of the game.
- **The game's code is built on your PC**, once, from your installed game, into `games\<version>\data\code\game.dll`. `Haiku.exe` loads it when it starts.
- **Most updates now play at once.** An update that only changes Haiku keeps your built game code. When an update does need the game's code built again, `Haiku.exe` does it by itself when you start it, and only the parts that changed are compiled.
- **The update screen is in the game's own look.** It opens in the game's window, with your window settings, drawn with the game's menus and font (in Japanese on the Japanese game). It shows each step, the files compiled and the time left. Then the game starts in the same window.
  - **Esc** or **B** asks whether to stop. What was compiled so far is kept, and the next start carries on from there.
  - If the build fails, the screen says why, with **Try Again**, **Open the Log** and **Quit**.
- The first setup still uses Haiku Setup's own screens: select your dump, then your DLC, then Build.

### All three versions side by side

- **One Haiku folder can hold the US, European and Japanese versions at once.** Each has its own folder, `games\USA`, `games\EUR` and `games\JPN`, with its game files, its built code and its mods.
- **Options > Interface > Game Version** picks the version that plays. **Restart with This Version** restarts the game as that version. The row shows when two or more versions are installed.
- **Each version keeps its own game language and voices.** For example, the European version can stay in French with Japanese voices while the US version plays in English with English voices. Switching back restores each version's own choice.
- **Each version has its own mods folder**, `games\<version>\mods`, because most mods fit one version only. Saves are kept per version, as before. The other settings (graphics, controls, the window and so on) are shared.

### Add a version, Remove a version

- **Add a version…** on Haiku Setup's Ready screen installs another version from its dump (and its DLC), beside the ones you have. The versions already installed aren't changed.
- **Remove a version…** deletes one version's folder: its game files and code, Haiku's files made from it, its caches and its mods. Setup shows what is deleted and what is kept, with sizes, and asks before it deletes. Your saves and settings, and the other versions, are kept. The only version installed can't be removed.
- **Removing a version deletes its mods folder** too (`games\<version>\mods`). Move any mods you want to keep before removing it.
- Haiku Setup is in the game's Options > Interface > Haiku Setup. It still has **Rebuild**, **Change dump or DLC** and the Japanese voices.

### Mods

- **Mods that add new code to the game (like V.O.W Edition) now work.**

## Fixes

- **Fixed save files sometimes never loading** (the save and load screens spinning forever), most often with mods on (#10).

## Known issues

- **Picking a mod's other code patch** can only be done in `games\<version>\mods.toml` (for example `code_patch = 1` in the mod's entry). The Mods screen has no control for it yet.
- **Not checked in game:** Nyx Veil Fix's veil, and Unofficial Gay Fates' paralogues and endings. These mods load, but we didn't reach those parts.
- **Haiku's compiler isn't offered for code mods** if your Haiku folder's path has letters outside plain English (A to Z). Zig still works there.
- With the Uncensored Patch, a save state taken during its subtitled opening movie doesn't resume. Save before or after the movie.
- Under a translation, the same-gender S-supports use general rules rather than hand-checked edits, so a line may keep a word for the wrong gender.
- Team If's translation has been tested less than the Uncensored Patch.
- Still being looked into: #3 (stuck on the Initializing screen) and #1 (camera judder entering and leaving battles).

## Notes

- Requirements: Windows 10 or 11 (64-bit), a GPU with Vulkan, and about 3 GB of disk space for each version you install.
- Haiku only goes online for online battles, or if you choose to download Zig for code mods. Haiku's own compiler needs no download, and Haiku Setup never uses the network.

## Legal

Haiku is an unofficial fan project, not affiliated with or endorsed by Nintendo or Intelligent Systems. Fire Emblem and Fire Emblem Fates are trademarks of Nintendo / Intelligent Systems. Haiku is MIT-licensed and provided as is, without warranty. The bundled compiler is GPL-3.0; its source is the second download above. Fan translations and other mods belong to their authors and are not part of Haiku.
