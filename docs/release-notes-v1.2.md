# Haiku 1.2

Haiku 1.2 fixes mods and code mods that didn't work in 1.1: the code-mod compiler download (#8), and mods laid out the way they are downloaded (#4). Code mods can now use the compiler that comes with Haiku, with no download. Haiku Setup now names all three releases of the game.

Haiku is a native Windows port of Fire Emblem Fates. The game's code is statically recompiled to x86-64 and runs on native PC code for graphics (Vulkan), audio, files, saves and input. Nothing is emulated. See the [README](https://github.com/tyle-beep/Haiku#features) for the full feature list.

**This release contains no game files.** You need your own legally obtained copy of Fire Emblem Fates Special Edition (US, European or Japanese), dumped and decrypted by you. Haiku Setup builds the game from it on your PC.

## Downloads

| File | Size | What it is |
|---|---|---|
| `Haiku.zip` | 54.6 MB (54,593,370 bytes) | Haiku Setup, the launcher and builder, with everything it needs to build Haiku from your dump. Extract it and run `Haiku Setup.exe`. |
| `Haiku-toolchain-source-w64devkit-2.11.0.zip` | 152.2 MB (152,193,966 bytes) | The source code of the bundled compiler (GCC 16.2.0, GNU Binutils 2.47, GMP, MPFR, MPC and MinGW-w64 v14.0.0, as w64devkit 2.11.0 builds them), offered under the GNU GPL version 3. Unchanged from 1.0. You don't need it to play. |

SHA-256:

```
3e3efb4a7a04f48767abb4975e5781a60b972f6e692cfe66a663a5696c28f368  Haiku.zip
2c9acf323c86b948c6fec9cba11158ce011d8edd2ce8f55b357f1d730080a557  Haiku-toolchain-source-w64devkit-2.11.0.zip
```

## Updating from 1.1

1. Close Haiku.
2. Extract the new `Haiku.zip` over your Haiku folder, replacing the files. Extract all of it, not just `Haiku Setup.exe`: the compiler choice for code mods needs the new files in `setup\w64devkit`.
3. Run `Haiku Setup.exe`. It opens on "Haiku is ready." Click **Rebuild**. It builds Haiku again from your dump, as the first time did (about 10 to 30 minutes). If your dump has moved, Setup asks for it again.

Extracting alone isn't enough: until you rebuild, `Haiku.exe` and Setup's **Play** button still start Haiku 1.1.

Your saves, settings, mods and Japanese voices are kept. Code mods you had turned on need compiling once more, because Haiku 1.2 recompiles them differently: their note on the Mods screen says so. Choose **Apply and Restart** there.

## What's new

### A choice of compiler for code mods

- **When a code mod needs a compiler**, Haiku now asks which one to use:
  - **Download Zig**: a one-time download of 96 MB. It compiles a code mod in under a minute.
  - **Use Haiku's compiler**: the one that comes with Haiku Setup. Nothing is downloaded, but a big code mod takes several minutes.
- **Your answer is kept.** Change it on the Mods page with **Code Mod Compiler** (Ask, Zig or Haiku's). The row shows when a mod has code.
- **Haiku's compiler is the backup.** If Zig can't be downloaded, unpacked or started, Haiku compiles the code mod with its own compiler, and the mod's note says so.
- While Haiku's compiler works, the dialog counts the files and the time, so it never looks stuck.

### Mods as you download them

*(#4)*

- **Mods made of the game's own folders** (such as `GameData`, `m` or `bu`) with no `romfs` folder around them now load. Before, Haiku said they had no files for the game. Nyx Veil Fix, Expanded Fates and Project Re-Fates are laid out like this.
- **A mod's folder for your release**, named by the game's title id, is now found up to two folders down. So a mod that holds several releases can go in the `mods` folder as you extracted it, for example Unofficial Gay Fates. Folders of optional files are left alone.
- **A mod with more than one code patch:** Haiku uses `code.ips` (or the one the mod's readme names) and offers the others as the mod's `code_patch` option. The mod's note on the Mods screen says which patch is used and how to pick another.
- **A code mod that can't be compiled no longer stops the others.** It is left out with a note that says why, and your other mods, other code mods included, still run. **Apply and Restart** now restarts even then, so your other changes take effect.

## Fixes

- **The compiler download** (#8): after reaching 100%, the download could sit for a minute and then fail with "The compiler couldn't be downloaded". This happened when another program had one of the new files open, such as an antivirus, the search indexer or OneDrive. Haiku now unpacks the compiler straight into place, so another program can't block it.
  - After 100%, the dialog now says "Unpacking the compiler...".
  - If the download fails, the mod's note says why in a few words, and `userdata\haiku\haiku.log` has the details.
  - The 400 MB `zig-0.17.0.unpacking` folder a failed 1.1 download left behind (in `cache\zakuro\tools`) is deleted the next time Zig is downloaded. You can also delete it yourself.
- **My Castle with Unofficial Gay Fates' code patch** (#4): the castle's menu (X) never opened, so the game was stuck in the castle. The mod's code over a game function that Haiku replaces ran in a loop. This is fixed for any code mod.
- **Code patches refused as "can't be compiled"** (#4): two causes are fixed: a jump into code Haiku had only guessed was code, and one kind of instruction that sets the processor's status. The two errors reported in #4 didn't happen with Unofficial Gay Fates 2.5.2, the version we tested, but both causes are fixed.
- **Haiku Setup and the European and Japanese releases:**
  - The game step no longer asks for a US dump. It names the US, European and Japanese releases.
  - A dump of one route (Birthright or Conquest) is told to use its own region's Special Edition.
  - A Citra or Azahar sdmc folder with the European or Japanese game installed is now found. 1.1 only found the US game there.

## Known issues

- **Picking a mod's other code patch** can only be done in `userdata\haiku\mods.toml` (for example `code_patch = 1` in the mod's entry). The Mods screen has no control for it yet.
- **Not checked in game:** Nyx Veil Fix's veil, and Unofficial Gay Fates' paralogues and endings. These mods load, but we didn't reach those parts.
- **Haiku's compiler isn't offered** if your Haiku folder's path has letters outside plain English (A to Z). Zig still works there.
- With the Uncensored Patch, a save state taken during its subtitled opening movie doesn't resume. Save before or after the movie.
- Under a translation, the same-gender S-supports use general rules rather than hand-checked edits, so a line may keep a word for the wrong gender.
- Team If's translation has been tested less than the Uncensored Patch.
- Still being looked into: #3 (stuck on the Initializing screen) and #1 (camera judder entering and leaving battles).

## Notes

- Requirements are unchanged: Windows 10 or 11 (64-bit), a GPU with Vulkan, and about 3 GB of disk space.
- Haiku only goes online for online battles, or if you choose to download Zig for code mods. Haiku's own compiler needs no download, and Haiku Setup never uses the network.

## Legal

Haiku is an unofficial fan project, not affiliated with or endorsed by Nintendo or Intelligent Systems. Fire Emblem and Fire Emblem Fates are trademarks of Nintendo / Intelligent Systems. Haiku is MIT-licensed and provided as is, without warranty. The bundled compiler is GPL-3.0; its source is the second download above. Fan translations and other mods belong to their authors and are not part of Haiku.
