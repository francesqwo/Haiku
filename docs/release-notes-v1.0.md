# Haiku 1.0

Fire Emblem Fates, recompiled for PC: the first public release.

Haiku is a native Windows port of Fire Emblem Fates (Special Edition, USA). The game's code is statically recompiled to x86-64 and runs on native PC code for graphics (Vulkan), audio, files, saves and input. Nothing is emulated. See the [README](https://github.com/tyle-beep/Haiku#features) for the full feature list.

**This release contains no game files.** You need your own legally obtained copy of Fire Emblem Fates Special Edition (USA), dumped and decrypted by you. Haiku Setup builds the game from it on your PC.

## Downloads

| File | Size | What it is |
|---|---|---|
| `Haiku.zip` | 49.3 MB (49,342,191 bytes) | Haiku Setup, the launcher and builder, with everything it needs to build Haiku from your dump. Extract it and run `Haiku Setup.exe`. |
| `Haiku-toolchain-source-w64devkit-2.11.0.zip` | 152.2 MB (152,193,966 bytes) | The source code of the bundled compiler (GCC 16.2.0, GNU Binutils 2.47, GMP, MPFR, MPC and MinGW-w64 v14.0.0, as w64devkit 2.11.0 builds them), offered under the GNU GPL version 3. You don't need it to play. |

SHA-256:

```
e4215e9c65d7ae41b40ebbb481626682d2a368026877c9ec59082744b3023df9  Haiku.zip
2c9acf323c86b948c6fec9cba11158ce011d8edd2ce8f55b357f1d730080a557  Haiku-toolchain-source-w64devkit-2.11.0.zip
```

## Getting started

1. Dump Fire Emblem Fates Special Edition (USA) from your own 3DS with GodMode9, decrypted (`.3ds`/`.cci`, `.cia` or `.cxi`, or an installed Citra/Azahar sdmc folder). Optionally dump your DLC too.
2. Extract `Haiku.zip` somewhere you can write to, for example `C:\Games\Haiku`.
3. Run `Haiku Setup.exe`, select your dump and your DLC, and build. It takes about 10 to 30 minutes, once.
4. Play. Press Esc for the options.

Requirements: Windows 10 or 11 (64-bit), a GPU with Vulkan, and about 3 GB of disk space.

## Notes

- The automated tests cover the game from boot through Chapter 6, My Castle, the shop, supports, the Dragon's Gate and a DLC map. Every part of the game is recompiled, but the later chapters of each route have had less testing: please report anything that goes wrong.
- Controllers work through SDL3. Virtual Xbox 360 and DualShock 4 pads have been tested; please report how your controller behaves.
- Code mods need a C compiler; Haiku asks before downloading one. Haiku only goes online when you play online or agree to that download, and Haiku Setup never does.
- No mods come with Haiku. Third-party mods, such as HD texture packs, belong to their authors.

## Legal

Haiku is an unofficial fan project, not affiliated with or endorsed by Nintendo or Intelligent Systems. Fire Emblem and Fire Emblem Fates are trademarks of Nintendo / Intelligent Systems. Haiku is MIT-licensed and provided as is, without warranty. The bundled compiler is GPL-3.0; its source is the second download above.
