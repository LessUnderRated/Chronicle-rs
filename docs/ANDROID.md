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

plus SDL's Java activity (`org.libsdl.app`, compiled from the same SDL checkout), `ChronicleActivity`,
`ChronicleDocumentsProvider`, and the port's own files as assets (`port/lang`, the message font,
`port/glyphs`).

The renderer is the port's Vulkan renderer, as on the PC. Its shaders are compiled from GLSL to
SPIR-V at build time by the host's `glslangValidator`, which Vulkan on Android takes as is; nothing is
converted for Android. The game's own assets are read in their PS2 formats, as on the PC.

### The Rust crate (`android/src`)

- `lib.rs`: `SDL_main`. Sends the game's stdout and stderr to logcat, makes `data/` and `save/` in the
  app's internal storage, installs the port's files, starts the touch controls and calls the game's
  main (`dc_main`, `port/src/main.cpp`) with `--data` and `--save`.
- `storage.rs`: the APK assets listed in `chronicle-assets.txt` are copied into `save/` (the game reads
  `lang/` and `glyphs/` there as well as beside its executable, which an app does not have) whenever
  the list or its contents change. `chronicle_android_resolve_uri` turns the `content://` URI SDL's
  file dialog returns into a path the disc reader opens: `/proc/self/fd/N` for the descriptor SDL
  opened, or, when the provider refuses that, a copy in `disc/disc.iso` made from the descriptor
  (removed at the next start once the data is extracted).
- `touch.rs`: the touch screen as an SDL virtual gamepad, and the triangles of its overlay. The game
  sees a controller: button prompts, the camera and the menus work as with a pad.
- `sdl.rs`: the SDL3 declarations the crate uses, written against the pinned SDL.

`cargo test` in `android/` runs the touch layout's tests on the host, against stand-ins for SDL
(`test_stubs.rs`).

### Changes to the C++ for Android

All under `#ifdef __ANDROID__` or `if(ANDROID)`; the PC builds are unchanged.

| Where | Why |
|---|---|
| `port/CMakeLists.txt` | The game is `libmain.so` (`SHARED`), linked with `-Bsymbolic` (the game's `rand`, `srand` and the like must win over bionic's, loaded first) and 16 KiB page alignment. `-Wno-format-security`: the NDK makes it an error and the game passes its message tables to `printf`. `dcdata` is not built |
| `port/include/port.h` | Bionic declares an `__assert` with the game's parameters; the game's is renamed |
| `port/src/runtime.cpp` | Bionic does not declare `rand`/`srand` `noexcept` |
| `port/src/main.cpp` | `dc_main`, which `SDL_main` calls |
| `port/src/platform/firstrun.cpp` | Taps press the first-run buttons (touch makes no mouse events), the disc dialog has no filter (Android filters by MIME type, and disc images are `application/octet-stream`), and a `content://` answer is resolved by the crate off the activity's thread |
| `port/src/touch_overlay.cpp`, `port/src/gameloop.cpp` | The touch controls are recorded into the overlay list the FPS badge uses, so they are drawn over every presented frame |
| `android/patches/kazarov-0001-android-surface.patch` | Android takes the window's surface away when the app leaves the screen; the renderer makes a new `VkSurfaceKHR` when it returns or when Vulkan reports the old one lost. The swapchain uses the identity transform, so the compositor turns the landscape image onto the phone's portrait panel (the renderer does not pre-rotate), and Android's resulting `VK_SUBOPTIMAL_KHR` on every frame is not taken for a resize |

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
`CHRONICLE_KEY_ALIAS` and `CHRONICLE_KEY_PASSWORD`, or with the debug key when they are unset, so a
local build always installs. `.github/workflows/android.yml` builds it on every push with the
`ANDROID_KEYSTORE*` secrets if they exist, and uploads the APK.

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

On first start the game shows its "game data needed" screen. Tap **DISC IMAGE** and pick your PAL or
NTSC disc image in the system picker; the game extracts it into the app's storage (a few minutes) and
starts. Your files are in the Files app under **Chronicle** (`data/`, `save/`), where saves can be
copied out and back.

## Touch controls

| Where | What |
|---|---|
| Left half, anywhere free | Left stick: it appears under the thumb and follows it past its edge |
| Right half, anywhere free | Right stick (the camera) |
| ✕ ○ □ △, bottom right | Face buttons |
| L1 L2, R1 R2, top corners | Shoulders and triggers |
| SELECT, START, top middle | Select and Start; the phone's Back is Start too |
| D-pad, left middle | Eight directions |

The overlay goes away while a controller is connected (Bluetooth or USB) and comes back when it is
disconnected. Touch makes no mouse events, so the menus' mouse pointer stays still.

## Where it stands

The APK builds and its native libraries link for arm64 with the NDK; it has not yet been run on a
device. What to look at on the first run:

```sh
adb logcat -s Chronicle SDL
```

- The renderer's device report: `RendererInit` names the GPU it chose and, for one it passed over,
  what it lacks. The S26's Adreno 840 and Xclipse 960 are both expected to have everything.
- Frame pacing and the S26's refresh rate (the game ticks at 60 Hz and interpolates between ticks).
- Leaving and returning to the app, and the image's orientation (the surface patch).
- Whether the picked disc is read through `/proc/self/fd` or copied (logcat says which).
