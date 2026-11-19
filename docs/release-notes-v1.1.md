# Haiku 1.1

Haiku now plays all three versions of Fire Emblem Fates Special Edition: the US release, the European release with five text languages, and the Japanese release (*Fire Emblem if*). This update also adds Japanese voices, Divine Pulse, save states and support for fan translations, and fixes the battle skies.

Haiku is a native Windows port of Fire Emblem Fates. The game's code is statically recompiled to x86-64 and runs on native PC code for graphics (Vulkan), audio, files, saves and input. Nothing is emulated. See the [README](https://github.com/tyle-beep/Haiku#features) for the full feature list.

**This release contains no game files.** You need your own legally obtained copy of Fire Emblem Fates Special Edition (US, European or Japanese), dumped and decrypted by you. Haiku Setup builds the game from it on your PC.

## Downloads

| File | Size | What it is |
|---|---|---|
| `Haiku.zip` | 54.5 MB (54,491,578 bytes) | Haiku Setup, the launcher and builder, with everything it needs to build Haiku from your dump. Extract it and run `Haiku Setup.exe`. |
| `Haiku-toolchain-source-w64devkit-2.11.0.zip` | 152.2 MB (152,193,966 bytes) | The source code of the bundled compiler (GCC 16.2.0, GNU Binutils 2.47, GMP, MPFR, MPC and MinGW-w64 v14.0.0, as w64devkit 2.11.0 builds them), offered under the GNU GPL version 3. Unchanged from 1.0. You don't need it to play. |

SHA-256:

```
b7ab965bdd0fa2fbfeb8d484e5a0473e9e788aa81b079b86cf95228e4f8029ea  Haiku.zip
2c9acf323c86b948c6fec9cba11158ce011d8edd2ce8f55b357f1d730080a557  Haiku-toolchain-source-w64devkit-2.11.0.zip
```

## Updating from 1.0

Extract the new `Haiku.zip` over your Haiku folder, replacing the files, then run `Haiku Setup.exe` and build again: it rebuilds Haiku from your dump with the new version. Your saves, settings and mods are kept.

## What's new

### The European Special Edition

- **Five languages**: English, French, German, Spanish and Italian. Pick one under Options > Interface > Game Language.
- **Haiku's own menus and text in every language**, including its added lines. They are made from your dump when Haiku Setup builds the game.
- **Button prompts** for Xbox, PlayStation and keyboard in every language.
- **The European DLC** installs with its own text in all five languages. The region-free DLC works too, in English.

### The Japanese Special Edition (*Fire Emblem if*)

- **The Japanese game runs natively**, with its Japanese script and voices. Haiku's menus are in Japanese, in the game's own font.
- **Content the Western releases don't have:**
  - **My Room's petting minigame**, played by dragging with the mouse;
  - the **Festival of Bonds** maps and the **Cipher** units, from the Japanese DLC;
  - **Corrin's three speaking styles** per gender, chosen in the avatar creator as in Japan.
- **Haiku's features work here too:** cheats, the randomizer, Divine Pulse, save states, the bottom screen, widescreen, high frame rates, HD packs, the amiibo battles and Sunglasses.
- **Menu Language** (Options > Interface) switches Haiku's own menus between Japanese and English.

### Japanese voices

*(Asked for in #5.)*

- **Play the US or European game with the Japanese voices.** On Haiku Setup's Play screen, click **Add Japanese voices…** and pick a dump that has them: the Japanese game, or an undub. Setup only copies the voice and sound files, and your game isn't changed.
- **The DLC's Japanese voices** come from the Japanese DLC, if you have it.
- **Switch between English and Japanese** on the Play screen or under Options > Audio > Voices.

### Fan translations as mods

- **Team If's translation** and the **Uncensored Patch** (v1.8) work as mods. Put the patch in the `mods` folder as you downloaded it: the zip or the extracted folder. Haiku recognises it and never runs any patcher program.
- **On the Japanese game** you get the translated script with all the Japan-only content. The patch's DLC files are laid over your own Japanese DLC, so you don't need its patched DLC file.
- **On the US and European games** you get the translated text. Haiku trims it to what those games can read.
- **Haiku's own additions stay** under a translation: its menus switch to English, and its added lines (Kana's question, same-gender S-supports) are made in English from the translation's own lines.
- **Speaking Style:** with a translation on the US or European game, Corrin can use the Japanese script's three speaking styles. Choose one in the avatar creator or under Options > Gameplay. The option is hidden without a translation.
- **Not included:** these translations belong to their authors, and Haiku doesn't include them. Get them from their authors.

### Divine Pulse

- **Turn back time on the map**, as in the later Fire Emblem games: Options > Gameplay > Divine Pulse, off by default.
- **A timeline of moments** is kept before each action. Pick one to go back to it, from the map menu (just above Return to Title), with the **P** key, or when a battle is lost.
- **Charges:** Story, Fixed or Unlimited.
- **The music keeps playing** through a rewind, and the rewind has its own effect on screen.
- **Off in online battles.**

### Save states

- **8 slots with thumbnails**, under Options > Cheats > Save States. **F5** saves and **F8** loads.
- **The music carries on** from where the state was saved.
- **A state remembers the setup it was made with** (mods, voices, language). It won't load under a different one, and tells you what changed. Texture packs never block a load.
- **Off in online battles.**

### Code mods

- **Large code mods build and run natively.** Mods that rewrite many of the game's functions used to fail with "the code could not be compiled".
- **Each code mod gets its own saves**, so a mod that changes the game can't damage your normal save files.
- **A code mod made for another version** of the game (US, European or Japanese) is refused with a clear message, and its other files are left out with it.
- **No console windows.** Compiling a code mod, or downloading its compiler, no longer flashes command prompt windows.

### Haiku Setup

- **Recognises which version a dump is** (US, European or Japanese) and builds that version.
- **Checks the DLC fits the game** before installing anything, and says why if it doesn't. For example, the European DLC is refused for the US game.
- **Japanese voices and the Japanese DLC's voices** can be added from the Play screen.

## Fixes

- **Battle skies** (#6): the sky dome in battles showed a stretched, blurry rock wall instead of the sky. The renderer now draws these skies as the six-sided cube maps they are: the sunset sky with clouds and distant mountains in the Prologue, the night sky over Krakenburg, the Chapter 5 sky, and the river's reflection. Looking up with the free battle camera shows real sky too.
- **Language option on the US game:** it did nothing, because the US game only has English text. It is now hidden there, and shown on the European and Japanese games, where it works.
- **Building from source on Windows:** a checkout with Windows line endings (Git for Windows' default `core.autocrlf`) built a Haiku.exe without the field's far grass (Full Draw Distance). Shader sources now always check out with Unix line endings, and the renderer works with either.

## Notes

- Each version of the game has its own saves. A US save doesn't carry over to the European or Japanese game.
- The automated tests cover each version from boot through the early chapters, My Castle, the shop, supports, the Dragon's Gate and DLC maps. The Japanese game's tests also cover Revelation, the amiibo battles, Kana's paralogue and the Festival of Bonds. Later chapters have had less testing: please report anything that goes wrong.
- Online battles between different versions of the game haven't been tested.
- Requirements are unchanged: Windows 10 or 11 (64-bit), a GPU with Vulkan, and about 3 GB of disk space. Japanese voices take about 400 MB more.

## Known issues

- With the Uncensored Patch, a save state taken during its subtitled opening movie doesn't resume. Save before or after the movie.
- Under a translation, the same-gender S-supports use general rules rather than hand-checked edits, so a line may keep a word for the wrong gender.
- Team If's translation has been tested less than the Uncensored Patch.
- Still being looked into: #3 (stuck on the Initializing screen) and #1 (camera judder entering and leaving battles).

## Legal

Haiku is an unofficial fan project, not affiliated with or endorsed by Nintendo or Intelligent Systems. Fire Emblem and Fire Emblem Fates are trademarks of Nintendo / Intelligent Systems. Haiku is MIT-licensed and provided as is, without warranty. The bundled compiler is GPL-3.0; its source is the second download above. Fan translations and other mods belong to their authors and are not part of Haiku.
