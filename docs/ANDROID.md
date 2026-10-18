# Android

`android/` builds Chronicle as an Android APK for arm64 phones with Vulkan 1.3 (Android 13 or later),
the Galaxy S26 being the first target. It is laid out as doukutsu-rs builds its APK
(`reference/doukutsu-rs`, `drsandroid/`; `docs/RUST_ANDROID_PLAN.md` compares the two): Gradle runs
CMake, CMake builds SDL from source and links the Rust crate with Corrosion, and SDL's own Java
activity runs it. The APK carries no game data: on first start it asks for the player's disc image and
extracts it on the phone.

## What is in the APK

| Library | Built from | Role |
|---|---|---|
| `libmain.so` | `ps2/src`, `port/src`, `tools/kazarov/port` (the PC port, unchanged in kind) and the Rust crate `android/` | The game. SDL looks up `SDL_main` in it, which the crate defines |
| `libSDL3.so` | SDL at the tag in `android/sdl3-tag.txt` | Window, Vulkan surface, input, audio, file dialog |
| `libc++_shared.so` | The NDK | C++ runtime |

plus SDL's Java activity (`org.libsdl.app`, compiled from the same SDL checkout), `SetupActivity` (the
launcher: the disc and its extraction), `ChronicleActivity` (the game), `ChronicleDocumentsProvider`, and the port's own files as assets (`port/lang`, the message font,
`port/glyphs`).

The renderer is the port's Vulkan renderer, as on the PC. Its shaders are compiled from GLSL to
SPIR-V at build time by the host's `glslangValidator`, which Vulkan on Android takes as is; nothing is
converted for Android. The game's own assets are read in their PS2 formats, as on the PC.

### The Rust crate (`android/src`)

- `logcat.rs`: the game's stdout and stderr to logcat and to `save/logs/chronicle.log` (the run
  before kept as `chronicle-previous.log`), and a fatal-signal handler that writes the signal and a
  backtrace there before Android's own handler ends the process.
- `lib.rs`: `SDL_main`. Opens the log, makes `data/` and `save/` in the
  app's internal storage, installs the port's files, starts the touch controls and calls the game's
  main (`dc_main`, `port/src/main.cpp`) with `--data` and `--save`.
- `storage.rs`: the APK assets listed in `chronicle-assets.txt` are copied into `save/` (the game reads
  `lang/` and `glyphs/` there as well as beside its executable, which an app does not have) whenever
  the list or its contents change. `chronicle_android_resolve_uri` turns the `content://` URI SDL's
  file dialog returns into a path the disc reader opens: `/proc/self/fd/N` for the descriptor SDL
  opened, or, when the provider refuses that, a copy in `disc/disc.iso` made from the descriptor
  (removed at the next start once the data is extracted).
- `touch.rs`: the touch screen as an SDL virtual gamepad. The game sees a controller: button prompts,
  the camera and the menus work as with a pad. This is the model: the default layout clear of the HUD,
  the player's arrangement laid over it (as JSON), and what the fingers on it press. The view is
  `TouchOverlay.java`, which draws it and hands over every touch.
- `host.rs`: what the app's menu asks of the game, which `PumpAndroid` (`port/src/main.cpp`) takes
  between ticks on the game's thread: a pause while the menu is open (sound stopped, the clock
  re-anchored after so nothing is caught up), the speed (1x or 2x the tick rate), and settings changed
  there (`ConfigSetValue`, the same path as the Options screen, so `config.json` follows).
- `memcard.rs`: the game's saves read out of a PS2 memory card (below).
- `jni.rs`: the methods of `Native.java`, registered in `JNI_OnLoad`.
- `setup.rs`: the JNI methods of the setup screen, `SetupActivity` (below): a readable path for the
  picked file's descriptor, what the disc is, and its extraction with progress, through
  `port/src/platform/android_setup.cpp`, which calls dcdata and the first-run screen's
  `FirstRunExtract` as the PC port does.
- `sdl.rs`: the SDL3 declarations the crate uses, written against the pinned SDL.

`cargo test` in `android/` runs the touch layout's tests on the host, against stand-ins for SDL
(`test_stubs.rs`).

### Changes to the C++ for Android

All under `#ifdef __ANDROID__` or `if(ANDROID)`, but one: everything is compiled with `-fsigned-char`,
which changes nothing on x86 or Apple arm64, where `char` is signed already.

| Where | Why |
|---|---|
| `port/CMakeLists.txt`, `-fsigned-char` | The PS2's `char` is signed; arm64 Linux and Android make it unsigned, so the game's `s8` fields never read negative. `SndInitSeTable` then missed the `-128` ending each sound table, ran past them and wrote over `cap_se_info`'s pointers: the crash right after the first start. |
| `port/CMakeLists.txt` | The game is `libmain.so` (`SHARED`), linked with `-Bsymbolic` (the game's `rand`, `srand` and the like must win over bionic's, loaded first) and 16 KiB page alignment. `-Wno-format-security`: the NDK makes it an error and the game passes its message tables to `printf`. `dcdata` is not built |
| `port/include/port.h` | Bionic declares an `__assert` with the game's parameters; the game's is renamed |
| `port/src/runtime.cpp` | Bionic does not declare `rand`/`srand` `noexcept` |
| `port/src/main.cpp` | `dc_main`, which `SDL_main` calls |
| `port/src/platform/firstrun.cpp` | Taps press the first-run buttons (touch makes no mouse events), the disc dialog has no filter (Android filters by MIME type, and disc images are `application/octet-stream`), and a `content://` answer is resolved by the crate off the activity's thread |
| `port/src/platform/android_setup.cpp` | The setup screen's disc identification and extraction, as C functions for `setup.rs` |
| `port/src/main.cpp` (`PumpAndroid`) | The app menu's pause, speed and settings, between ticks |
| `port/src/platform/config.cpp` | `ConfigSetValue`, one setting by name; `video.cover` and `video.title_aspect` (both on every platform; Android's default for `video.cover` is `fit`) |
| `port/src/fireomni.cpp` | The torches' heat shimmer (`CFireOmni::DrawRaster`) is left out for a torch wholly past the 4:3 frame's sides, where it would copy from outside the 640-wide `blender` target; on every platform |
| `android/patches/kazarov-0001-android-surface.patch` | Android takes the window's surface away when the app leaves the screen; the renderer makes a new `VkSurfaceKHR` when it returns or when Vulkan reports the old one lost. The swapchain uses the identity transform, so the compositor turns the landscape image onto the phone's portrait panel (the renderer does not pre-rotate), and Android's resulting `VK_SUBOPTIMAL_KHR` on every frame is not taken for a resize |
| `android/patches/kazarov-0002-safe-area-and-cover-fit.patch` | The HUD anchors to the window's safe area, and `FrameLayout::cover` with `CoverMode::Fit` keeps full-frame pictures 4:3 with black bars (`KZ_GFX_COVER_MODE` tells the game the Kazarov it builds with has it) |

Kazarov is a submodule of another repository, so its Android changes are patches here:
`android/kazarov.cmake` copies `tools/kazarov` into the build tree, applies `android/patches/kazarov-*.patch`
and builds the copy. The submodule itself is never modified. Each patch should go upstream to Kazarov,
and be deleted here once the submodule carries it.

## Building

Needs JDK 17 or later, the Android SDK with the packages in `android/app/packages.txt` (NDK r30,
CMake 3.31.6, build-tools 36, platform 36), Rust with `rustup target add aarch64-linux-android`,
`glslangValidator` and Ninja, and the `tools/kazarov` submodule.

```sh
export ANDROID_HOME=/path/to/android-sdk
cd android
./gradlew assembleRelease        # app/build/outputs/apk/release/app-release.apk
./gradlew assembleDebug          # app-debug.apk, installable beside the release one
```

The first build clones SDL into `android/build/sdl3/` and compiles the whole game (about 700 units),
which takes a while; later builds are incremental. `-Pchronicle.abis=arm64-v8a,x86_64` adds the
emulator's ABI.

A release is signed with the key named by `CHRONICLE_KEYSTORE`, `CHRONICLE_KEYSTORE_PASSWORD`,
`CHRONICLE_KEY_ALIAS` and `CHRONICLE_KEY_PASSWORD`, or with `android/app/chronicle-dev.jks` (password
and alias `chronicle`) when they are unset. That key is public, so it says nothing about who built an
APK; it is there so that every build installs over the last and keeps the extracted disc and the
saves. Releases before 0.5.0 were signed with a throwaway key: going from one of those takes an
uninstall. `.github/workflows/android.yml` builds it on every push with the
`ANDROID_KEYSTORE*` secrets if they exist, and uploads the APK. A push whose `versionName` has no release yet
also publishes the APK as the GitHub release `android-v<version>`.

The native build can also be run without Gradle, which is quicker for working on the C++:

```sh
cmake -S . -B build-android -G Ninja \
    -DCMAKE_TOOLCHAIN_FILE=$ANDROID_HOME/ndk/30.0.16248370/build/cmake/android.toolchain.cmake \
    -DANDROID_ABI=arm64-v8a -DANDROID_PLATFORM=android-33 -DANDROID_STL=c++_shared \
    -DPLATFORM=PC -DDC_BUILD_TESTS=OFF -DCMAKE_BUILD_TYPE=Release
cmake --build build-android --target darkcloud
```

## Installing and first start

```sh
adb install -r android/app/build/outputs/apk/release/app-release.apk
```

or copy the APK to the phone and open it (allow installs from that app when asked).

The game's data comes from your own disc. Make a disc image (ISO) of your PlayStation 2 copy of Dark
Cloud and put it on the phone (Downloads, a memory card, a cloud drive the picker shows). The first
start opens the setup screen (`SetupActivity`) instead of the game:

1. **Your Dark Cloud disc**: tap *Choose disc image* and pick the ISO in Android's file picker.
2. The disc is read and shown: its product code from `SYSTEM.CNF` (`SCUS-97111`, `SCPS-…`,
   `SCES-50295`), its region, the release the extractor knows it as, its languages and the size of its
   data, with a check of the free space. Something that is not a Dark Cloud disc is turned down here.
3. *Extract* copies the data out of the image into the app's storage, with progress and a cancel
   button (a later run resumes where a cancelled one stopped). The game then starts, and every start
   after goes straight to it.

| Disc | Product code | Languages | State |
|---|---|---|---|
| US (NTSC-U) 1.02 | SCUS-97111 | English (US) | Known release (`KnownReleases`) |
| Other US printings | SCUS-… | English (US) | By NTSC's file layout |
| European (PAL) | SCES-50295 | English (UK), French, German, Italian, Spanish | By PAL's layout |
| Japanese (NTSC-J) | SCPS-… | Japanese | **Experimental**: by the product code alone. The port is built from the PAL game's code and has not been run on Japanese data |

The ISO itself is read where it is (through `/proc/self/fd`, like PPSSPP and Dolphin) and left
unchanged; where its provider will not allow that, it is copied into the app first and the copy is
deleted afterwards. Your files are in the Files app under **Chronicle** (`data/`, `save/`), where saves
can be copied out and back. Deleting `data/` brings the setup screen back on the next start; the game's
own first-run screen (`port/src/platform/firstrun.cpp`) remains for data that goes missing while it runs.

## The screen's shape

The game takes the whole panel: the window is fullscreen (immersive, `Config`'s Android default) and SDL
lays it under the camera's cutout. With `video.aspect` `auto` (the default) the 3D fills it at the
phone's own shape, 19.5:9 on the S26, at retail's vertical field of view, so the sides show more of the
world; nothing of the game is stretched:

- **2D** (menus, message windows, the Georama editor) keeps its 4:3 proportions in the centre.
- **The HUD moves to the edges** where it belongs (`docs/PC.md`, "Window aspect"): the dungeon's life,
  weapon and water to the top left, its floor plate and mini map to the top right, the weapon's
  gauge to the bottom left, its hint window to the bottom right, and the town's clock to the top
  right. They stop at the window's safe area (`SDL_GetWindowSafeArea`: the cutout and the gesture
  edges), not at the glass's edge.
- **Full-screen pictures** (cutscene stills) are kept at 4:3 with black bars, `video.cover` `fit`, which
  is Android's default; the menu's *Full-screen pictures* setting stretches them instead.
- **The title, the attract movie and the opening** fill the screen too (`video.title_aspect` `wide`).
  They are framed for 4:3, so more of their scenes shows at the sides than the PS2 showed; the menu's
  *Title & opening* plays them in the 4:3 frame instead.
- **Torches' heat shimmer** past the frame's sides is left out rather than drawn from outside its
  buffer (the fire itself still burns there).

Both come from `android/patches/kazarov-0002-safe-area-and-cover-fit.patch`. The touch controls make room
for the HUD (`android/src/touch.rs`, `Hud`): the shoulders sit on the side edges under its top band,
SELECT and START in the gaps beside the quick items, and the left stick rests above the gauge.

## Touch controls

The controls are an Android view over the game (`TouchOverlay.java`); what they press goes to an SDL
virtual gamepad (`touch.rs`), so the game sees a controller.

| Where (by default) | What |
|---|---|
| Left half, anywhere free | Left stick: it appears under the thumb and follows it past its edge |
| Right half, anywhere free | Right stick (the camera) |
| ✕ ○ □ △, bottom right | Face buttons |
| L2 L1, R2 R1, the side edges under the HUD | Triggers and shoulders |
| SELECT, START, either side of the quick items at the top (bottom centre on a narrower screen) | Select and Start |
| D-pad, left, under L1 | Eight directions |
| **2×**, top left (small) | Speed: the game runs at twice its tick rate while it is lit |
| **☰**, top right (small) | The menu; the phone's Back opens it too |

**Editing** (menu, *Edit touch controls*): the game holds still, touch any control (the 2× and ☰
buttons too) and drag it where you want it; the panel's sliders set the picked control's size (25% to
250% of the default) and opacity, *Reset all* goes back to the default layout and *Done* saves. The
arrangement is kept per app (SharedPreferences) and scales with the window, so it survives rotation and
a different screen. *Touch controls opacity* in the settings sets all of them at once, and *Hide touch
controls* leaves only 2× and ☰.

The pad goes away while a controller is connected (Bluetooth or USB) and comes back when it is
disconnected; 2× and ☰ stay. No ready-made library fitted: RadialGamePad (Lemuroid's) is GPL-3.0 and lays
out radial dials in code, and its successor PadKit is Compose Multiplatform without a runtime editor, so
the view and its editor are written here.

## The menu

☰ or Back pauses the game and opens:

- **Speed** 1× / 2×.
- **Graphics & video settings**: aspect ratio, title & opening, full-screen pictures, interface size,
  frame limit, V-Sync, smooth motion, anisotropic filter, soft focus, text font, FPS counter, volume,
  and the touch controls' opacity. They are the game's own settings (`config.json`), applied at once;
  the rest (audio mode, controls, text, language) are in the game's Options screen, from the town and
  dungeon menus.
- **Edit / hide touch controls** (above).
- **Import saves from a PS2 memory card** (below).
- **Share log**: this run's log, as text, to any app that takes it.
- **Quit**.

## When it crashes

Every run writes `save/logs/chronicle.log` (the Files app shows it under Chronicle). When the last run
crashed, stopped responding, quit with an error, or was killed while on screen (Android's
`ApplicationExitInfo` says which), the next start says so first and offers that run's log: **Share
log** or **Copy log**. A native crash's lines look like a tombstone's
(`#03 pc 0x3a364c libmain.so (…)`); each release carries its unstripped `libmain.so` to read them:

```sh
llvm-symbolizer --obj=libmain-0.5.0-arm64-symbols.so 0x3a364c
```

## Saves from a PS2 memory card

*Import saves from a PS2 memory card* takes a card image from the file picker and brings every Dark
Cloud save on it into the game's load screen (`memcard.rs`):

- **Read:** a raw card image (`.ps2`, `.mcd`, `.bin`), as PCSX2, AetherSX2 and NetherSX2 keep their
  cards and as mcpd dumps a real one, with or without the 16 bytes of ECC after each page; and a
  uLaunchELF save export (`.psu`). Action Replay MAX exports (`.max`) are compressed and not read yet.
- **Found:** each `darkcloudN` in a folder the game made (`BESCES-50295dkcloud`, `BASCUS-97111dkcloud`,
  `BISCPS-15004dkcloud`), checked against its checksums.
- **Put:** each in the lowest numbered save folder free, `saves/<N>/save.dat`, the same bytes the port
  writes (`docs/PC.md`, "Saves and host files"). Nothing is written over, and a save already there
  byte for byte is left out, so importing the same card twice adds nothing.
- **Other releases:** the port's game is PAL's and reads saves marked `darkcloudVer1.9`. A save marked
  otherwise (an NTSC or Japanese card) is given that mark, which its checksums do not cover, so it
  loads; that its contents mean the same to PAL's code is untested, and the dialog says so.

## Save states

Not yet. A save state has to hold everything the running game holds: its arenas, and the port's own
state beside them (the textures and meshes on the GPU, the sound banks, the caches in the host's heap),
which the game's memory points into. Restoring the game's memory alone would leave those pointers
naming objects that no longer exist. The way there is to make the port's host state rebuildable from
the game's memory (textures re-uploaded, banks reloaded) and then snapshot the arenas and the game's
globals; until then the game saves where it always has, with the autosave on arriving in a town.

## Where it stands

The APK builds and its native libraries link for arm64 with the NDK; it has not yet been run on a
device. What to look at on the first run (or in a shared log):

```sh
adb logcat -s Chronicle SDL
```

- The renderer's device report: `RendererInit` names the GPU it chose and, for one it passed over,
  what it lacks. The S26's Adreno 840 and Xclipse 960 are both expected to have everything.
- Frame pacing and the S26's refresh rate (the game ticks at 60 Hz and interpolates between ticks).
- Leaving and returning to the app, and the image's orientation (the surface patch).
- Whether the picked disc is read through `/proc/self/fd` or copied (logcat says which).
- A Japanese disc: whether the game gets past the title with its data.
- An imported NTSC or Japanese save: whether it loads and plays on as it did.
