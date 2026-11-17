Haiku
=====

Fire Emblem Fates, recompiled for PC: the game's own code, statically
recompiled to native x86-64 and run on native PC code (graphics, audio,
files, saves, input), with an options menu, controllers, widescreen, high
frame rates and the bottom screen on one display.

This download holds no part of the game. Haiku is built on your PC from
your own copy:

- your dump of Fire Emblem Fates Special Edition (USA), decrypted: a .3ds
  (.cci), .cia or .cxi of the cartridge, or Citra's or Azahar's sdmc
  folder with it installed;
- optionally, your DLC: a decrypted .cia, or the same sdmc folder.

Dump them from your own 3DS with GodMode9 (decrypted). Haiku never needs
or asks for console keys.


Setting up
----------

1. Extract this folder somewhere you can write to, for example
   C:\Games\Haiku. No administrator rights are needed, and nothing is
   installed outside the folder.
2. Start "Haiku Setup.exe".
3. Select your dump, then your DLC (or continue without it: the DLC maps
   won't be available).
4. Build. Haiku Setup checks your dump, installs the game's files into
   data\, makes Haiku's own files from them, recompiles the game's code
   and compiles it with the compiler in setup\w64devkit. This takes about
   ten minutes to half an hour, once. If it stops, start it again: it
   carries on where it stopped.
5. Play. Next time, start Haiku.exe (or Haiku Setup, which offers Play,
   Rebuild and Change dump or DLC).

What the build makes, next to Haiku Setup.exe:
  Haiku.exe, SDL3.dll   the game
  data\                 the game's files, from your dump (about 2.4 GB)
  assets\               Haiku's own files, made from them
  userdata\haiku\       your saves and settings (made when you play)
  Haiku Setup.log       what the setup did
Your dump is only read; it can be deleted afterwards (keep it to rebuild).


Playing
-------

Esc (or the controller's Guide button) opens the options. Mods go in the
mods folder next to Haiku.exe; none come with Haiku. Controllers,
keyboard, widescreen, frame rate and the bottom screen are all in the
options.


Legal
-----

Haiku is a fan project. It is not affiliated with or endorsed by Nintendo or
Intelligent Systems. Fire Emblem and Fire Emblem Fates are
trademarks of Nintendo / Intelligent Systems. This download holds none of
the game: no code, art, audio, text or keys. You need your own legally
obtained copy, dumped by you. Haiku is provided as is, without warranty of
any kind (LICENSES\Haiku-MIT.txt).

Project page and source: @REPO_URL@


Licences
--------

LICENSES\ lists everything in this download and its licence: Haiku (MIT),
Zakuro (the 3DS runtime Haiku grew from, MIT), 3dsrecomp (MIT), SDL3
(zlib), SDL_GameControllerDB (zlib), the Rust crates (Rust-crates.txt and
their licence texts, Rust-crates-licences.txt), and the compiler (GCC and
Binutils, GPL 3, with the GCC Runtime Library Exception; MinGW-w64).

The compiler's source code is offered beside Haiku.zip on the release
page, as Haiku-toolchain-source-w64devkit-2.11.0.zip
(LICENSES\Toolchain.txt says what it holds and where else to get it).
