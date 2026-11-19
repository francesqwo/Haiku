Haiku
=====

Fire Emblem Fates, recompiled for PC: the game's own code, statically
recompiled to native x86-64 and run on native PC code (graphics, audio,
files, saves, input), with an options menu, controllers, widescreen, high
frame rates and the bottom screen on one display.

This download holds no part of the game. The game's code is built on your
PC from your own copy:

- your dump of Fire Emblem Fates Special Edition (the US, European or
  Japanese release), decrypted: a .3ds
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
2. Start Haiku.exe. The first time, it opens Haiku Setup.
3. Select your dump, then your DLC (or continue without it: the DLC maps
   won't be available).
4. Build. Haiku Setup checks your dump, installs the game's files into
   data\, makes Haiku's own files from them, recompiles the game's code
   and compiles it with the compiler in setup\w64devkit. This takes about
   ten minutes to half an hour, once. If it stops, start Haiku.exe again:
   it carries on where it stopped.
5. Play. From then on Haiku.exe starts the game straight away. Haiku Setup
   (Rebuild, Change dump or DLC, Add a version, Remove a version, the
   Japanese voices) is in the game's Options > Interface > Haiku Setup.

What the setup makes, next to Haiku.exe:
  games\USA\            the version of the game you set up (games\EUR\
                        or games\JPN\ for those), with:
    data\                the game's files, from your dump (about 2.4 GB),
                        and data\code, the game's code built from them
    assets\              Haiku's own files, made from them
    mods\                that version's mods (and mods.toml, their list)
    cache\, setup-work\  what Haiku keeps to start and build quickly
  userdata\haiku\       your saves and settings (made when you play), for
                        every version
  setup-work\setup.log  the setup's log
Your dump is only read; it can be deleted afterwards (the game's code is
built from data\ when it has to be built again).


More than one version
---------------------

The US, European and Japanese versions can all be installed in the same
Haiku folder. In Haiku Setup (Options > Interface > Haiku Setup), choose
Add a version... and give it a dump of another version (and its DLC, if
you have it): it is installed beside the others, in its own folder under
games\, and nothing of the others changes. Then Options > Interface >
Game Version picks the one that plays (Restart with This Version applies
it).

Each version keeps its own game language and its own voices (English or
Japanese), and its own mods folder, games\<version>\mods, because most
mods fit one version only. Saves are kept for each version. The other
settings are shared.

Remove a version... in Haiku Setup deletes one version's folder,
games\<version>\: its game files and code, Haiku's files made from it,
its caches and its mods. Your saves and settings (userdata\haiku) and the
other versions are kept. The only version installed can't be removed.


Updating
--------

Extract a new release over this folder, replacing files, and start
Haiku.exe. Usually it plays at once: the game's code stays as it is.
When a release needs it built again, Haiku.exe does that by itself first,
in the game's window (only what changed is compiled), then starts the
game. Your saves, settings and mods are never touched.

From Haiku 1.0, 1.1 or 1.2: extract over that folder the same way.
Haiku.exe finds the game already installed, builds the game's code once
(about as long as the first build), and removes what only the old Haiku
Setup.exe used (Haiku Setup.exe itself, setup\engine, setup\overrides);
its log moves to setup-work\setup-1.x.log. The game's files, Haiku's files
made from them, your mods folder and its list move into games\<version>\
(the version you had), as they are; saves and settings stay in userdata\.


Japanese voices (optional)
--------------------------

Haiku can play the game with the Japanese cast's voices (the text stays
English). In Haiku Setup (the first time, or Options > Interface > Haiku
Setup), on the Play screen choose "Add Japanese voices...",
then select a dump that has them: Fire Emblem if (the Japanese release,
for example the Special Edition, CTR-P-BFZJ), or a Western "undub". Haiku
Setup takes only its voice and sound files (about 360 MB) into
data\voices-ja and checks them; nothing else of that dump is used, and
your game's own files are not changed. Then switch between English and
Japanese on the Play screen, or in the game's Options > Audio > Voices
(the change applies when the game restarts). Each version of the game
keeps its own choice.


Playing
-------

Esc (or the controller's Guide button) opens the options. Mods go in the
mods folder of the version they are for, games\<version>\mods (the Mods
screen's Open Mods Folder opens it), as you downloaded them; none come
with Haiku. Controllers, keyboard, widescreen, frame rate and the bottom
screen are all in the options.

Code mods need a C compiler. Haiku asks first, and offers to download Zig
(96 MB, fast) or to use the compiler in setup\w64devkit, which came with
Haiku (no download, slower on big code mods). The Mods page's Code Mod
Compiler row changes the choice.


Fan translations of the Japanese game
-------------------------------------

Haiku can use the fan translations made for the Japanese release, Fire
Emblem if: Team If's translation (the "FireEmblemIf-FanTranslation"
project on GitHub) and cris1010's "Fire Emblem Fates Uncensored Patch"
(Team If's work continued, on the Varis Hangout forum). Neither comes with
Haiku; get them from their own pages.

- Team If: put the release's zip in the mods folder as it is.
- Uncensored Patch: it is a RAR, which Haiku can't open. Extract it with
  7-Zip (the password is on the patch's own page) and put the extracted
  folder, or just its rom folder, in the mods folder.
- Then turn it on in the Mods screen and Apply and Restart. Haiku lays the
  Japanese files out for this game as the patch's own patcher does; the
  patcher (IfPatcher.exe) is never run.
- Haiku's own lines (Kana's gender, same-gender S-supports, End Turn, the
  Sunglasses) are added to the translation's text, and a line it lacks is
  the US game's own.
- Its title logo is used; its Japanese fonts aren't (the game's own draw
  the translation, and the few signs they can't show, such as o with a
  bar, are written as the nearest they can). To use them anyway, add
  options = { fonts = 1 }  to its entry in games\<version>\mods.toml.
- Don't load a save state made with the translation off while it's on (or
  the reverse); the game's own saves are fine either way.
- On this US game the petting minigame stays cut, the Japan-only DLC
  can't run, DLC maps keep their English text, and a code patch made for
  the Japanese game (such as the endings patch) is left out.


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
