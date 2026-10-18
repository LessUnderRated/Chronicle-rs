# Rust port and Android APK plan

The goal: a Rust build of Chronicle that ships as an Android APK, built the way
[doukutsu-rs](https://github.com/doukutsu-rs/doukutsu-rs) builds Cave Story's
(read at `d05995b`, 2026-10-13). The first device is a Galaxy S26.

## 1. How doukutsu-rs makes its APK

Everything lives in `drsandroid/`:

```
drsandroid/
  Cargo.toml                  crate-type = ["staticlib"], depends on the game crate
  src/lib.rs                  #[no_mangle] SDL_main → stdout/stderr to logcat → ndk_context → game::init
  build.gradle.kts            Gradle root
  app/build.gradle.kts        compileSdk/targetSdk 35, minSdk 24, NDK 28.0.13004108, c++_shared,
                              externalNativeBuild → src/main/cpp/CMakeLists.txt,
                              java.srcDirs += SDL's android-project Java (found with `cargo metadata`)
  app/src/main/cpp/CMakeLists.txt
                              FetchContent Corrosion v0.5.2
                              add_subdirectory(SDL sources that cargo checked out for sdl2-sys)
                              corrosion_import_crate(drsandroid) → libdrsandroid.so =
                                whole-archive(drsandroid-static) + android EGL log SDL2
  app/src/main/java/...       MainActivity (checks for data/, offers download)
                              GameActivity extends org.libsdl.app.SDLActivity
                                (getLibraries = SDL2, drsandroid; reports cutout insets)
                              DoukutsuDocumentsProvider (exposes getFilesDir() in the system Files
                                app, so players copy their data in)
  app/packages.txt            sdkmanager list CI installs (ndk, build-tools, platform)
```

CI (`.github/workflows/ci.yml`, `build_android`) installs the four Rust
Android targets, installs `packages.txt` with `sdkmanager`, runs
`./gradlew assembleRelease`, signs the APK with `apksigner` using keystore
secrets, and uploads it.

The engine itself has an SDL2 backend on Android, a GLES 3.0 renderer
(`#version 300 es`, sprite batches), and touch controls in game logic
(`input/touch_controls.rs` and `touch_player_controller.rs`, fed by SDL
`FingerDown`/`FingerMotion`/`FingerUp`).

**What matters here is that Gradle builds through CMake, CMake builds SDL from
source and links the Rust crate with Corrosion, and SDL's own Java activity
hosts it.** Chronicle already builds with CMake on SDL3, so it fits the same
pipeline without restructuring.

## 2. Extending this to 3D

doukutsu-rs's renderer is 2D only, so none of its graphics code carries over.
Chronicle doesn't need it:

| Piece | doukutsu-rs | Chronicle on Android |
|---|---|---|
| Window/input/audio | SDL2 + `SDLActivity` | **SDL3** + SDL3's `SDLActivity` (`android-project/` in SDL3's sources) |
| Renderer | GLES 3.0, 2D | **Kazarov's existing Vulkan renderer** (`tools/kazarov/port/gfx`), surface from `SDL_Vulkan_CreateSurface` |
| Shaders | GLSL compiled at run time | GLSL → SPIR-V at build time with the host's `glslangValidator`, as now |
| Native lib | Rust `staticlib` | C++ game (`ps2/src` + `port/src` + `dc_host`) **plus** a Rust `staticlib` (`chronicle-android`) owning `SDL_main` |
| Touch | in game logic | **a virtual gamepad below the game**: an SDL3 virtual joystick (`SDL_AttachVirtualJoystick`) fed by finger events. The game's pad path doesn't change |
| Data | Files-app provider + download | Files-app provider **and** `SDL_ShowOpenFileDialog` (SAF on Android) for the ISO, which the first-run screen already calls |

### The S26's GPU

The Galaxy S26 ships with a Snapdragon 8 Elite Gen 5 (Adreno 840) or an Exynos
2600 (Xclipse 960, AMD RDNA), depending on region. Both drivers are Vulkan 1.3+
and are expected to have everything Kazarov's renderer requires: `dualSrcBlend`,
`shaderClipDistance`, descriptor indexing with 8192 update-after-bind images,
`dynamicRendering`, and `synchronization2`.

**Check this first, on the phone:** `RendererInit` prints each requirement a
device lacks (`tools/kazarov/port/gfx/requirements.cpp`), and the first logcat
run shows it. If something is missing, see the fallbacks in section 6. **No
wgpu or GLES rewrite is needed for the S26.**

## 3. Android problems in the current C++

What I found reading the sources, and the fixes:

| Where | Problem | Fix |
|---|---|---|
| `tools/kazarov/port/platform/paths.cpp` | Android defines `__linux__`, so it takes the XDG branch | Add an `__ANDROID__` branch first: `SDL_GetAndroidInternalStoragePath()` for `data/`, `save/`, `soundtrack/` |
| `port/src/main.cpp:475` | `PORT_OVERRIDE int main(...)`, but SDL on Android calls `SDL_main` | Rust's `SDL_main` calls it under another name (`-Dmain=dc_main` on that unit, or a one-line `dc_main` wrapper) |
| `KazarovPort.cmake` | `find_package(Vulkan 1.4)`, but the NDK's headers may be older and there's no loader to `find` | On Android: Vulkan-Headers via FetchContent, link the NDK's `libvulkan.so` |
| `KazarovPort.cmake` | `find_package(SDL3)` | Already skipped `if(TARGET SDL3::SDL3)`, so `add_subdirectory(SDL3)` first is enough |
| `dcdata extract` / first run | Reads the ISO with `std::ifstream`, but SAF returns a `content://` URI | Open it with `SDL_IOFromFile` (SDL3 handles `content://` on Android), or copy it into `getCacheDir()` first |
| C++26 | The port needs clang ≥ 20 | Use an NDK whose clang is ≥ 20 (r29+). Check with `__clang_major__` and fix the libc++ gaps in `port/src` |
| `memory.cpp` | `MAP_NORESERVE` arenas | Works on bionic. Check the total reservation against the S26's RAM in logcat |
| Discord, Flatpak, GTest, `dualsense` HID | Desktop-only | `if(NOT ANDROID)` around Discord, Flatpak and tests. DualSense still works through SDL's HIDAPI over Bluetooth |

## 4. Layout

Copy `drsandroid/` and change three things: SDL3 instead of SDL2, the C++
game added to the CMake build, and a Vulkan surface instead of EGL.

```
android/
  Cargo.toml                       chronicle-android: crate-type = ["staticlib"]
  src/lib.rs                       SDL_main: logcat pipe (as drsandroid), ndk_context,
                                   chdir to internal storage, touch overlay, → dc_main(argc, argv)
  src/touch.rs                     finger events → SDL virtual gamepad; layout and drawing data
  build.gradle.kts  settings.gradle  gradle.properties  gradlew
  app/build.gradle.kts             as drsandroid's: compileSdk/targetSdk 35+, minSdk 29 (Vulkan 1.1 floor
                                   on the loader side), ndkVersion r29+, c++_shared,
                                   abiFilters arm64-v8a (+ x86_64 for the emulator),
                                   java.srcDirs += SDL3/android-project/app/src/main/java
  app/packages.txt
  app/src/main/cpp/CMakeLists.txt
      FetchContent Corrosion; FetchContent or add_subdirectory SDL3 (pinned 3.4.x)
      FetchContent Vulkan-Headers
      set(PLATFORM PC) + add_subdirectory(${CHRONICLE_ROOT} chronicle)   # builds dc_host + game as a static lib
      corrosion_import_crate(android/Cargo.toml)
      add_library(main SHARED dummy.cpp)
      target_link_libraries(main  -Wl,--whole-archive chronicle-android darkcloud_game -Wl,--no-whole-archive
                            SDL3::SDL3 vulkan android log)
  app/src/main/java/.../
      ChronicleActivity extends org.libsdl.app.SDLActivity   getLibraries = SDL3, main; insets
      MainActivity                                           data check → "Pick your Dark Cloud ISO"
      ChronicleDocumentsProvider                             copied from DoukutsuDocumentsProvider
  app/src/main/AndroidManifest.xml  sensorLandscape, isGame, uses-feature vulkan.version 0x403000
```

On the Chronicle side, `port/CMakeLists.txt` needs one new option: build the
game as a static library (`darkcloud_game`) instead of the `darkcloud`
executable when `ANDROID` is set.

## 5. Touch controls for a 3D game

On a phone, the controls doukutsu-rs builds into its game logic become an SDL
virtual gamepad. Everything above it (`libpad.cpp`, button glyphs, menus)
sees a normal controller:

- **Left half:** floating analog stick (left stick).
- **Right half, drag:** camera (right stick, which the game's camera already
  reads).
- **Right cluster:** ✕ ○ △ □. Top edges: L1/R1 (L1 is lock-on/guard), L2/R2.
  Top centre: Start/Select.
- **Menus and naming screen:** tap acts as the mouse pointer the port already
  supports (`menu_mouse`, `name_mouse`).
- **Georama:** one finger moves the cursor, two fingers rotate and zoom.
- The overlay hides when a real controller connects. That's
  `SDL_EVENT_GAMEPAD_ADDED`, and the glyph sets in `port/glyphs` already
  follow the device.
- Drawing: one translucent `gfx::Draw2D` pass after the game's frame, in the
  window's logical mapping, as `platform/overlay.cpp` draws the FPS text.
  The layout is in safe-area insets from `ChronicleActivity`, as
  doukutsu-rs's `updateCutouts`.

## 6. If a GPU requirement is missing

Not expected on the S26, but needed for older phones:

- `dualSrcBlend` missing: a second pipeline variant that writes the blend
  factor to a second attachment and blends in a resolve pass.
- Descriptor limits below 8192: page the texture array (the game's VRAM
  rectangles map onto pages).
- Vulkan < 1.3: out of scope. Those phones would need the wgpu path from the
  earlier version of this plan.

## 7. Phases

| # | Phase | Exit |
|---|---|---|
| 0 | Permission: Chronicle has **no LICENSE file** (Kazarov is MIT). Ask TheMoonPeople before publishing an APK. Never ship game data | Written OK |
| 1 | `android/` scaffold: SDL3 + Corrosion + Rust `SDL_main` that opens a window and clears it through Kazarov's renderer | APK installs on the S26 and logcat shows `RendererInit` choosing the GPU |
| 2 | Game linked in: the section 3 fixes, `darkcloud_game` static library | Boots to the language select from data copied in through the Files app |
| 3 | Data onboarding: ISO picked through SAF, extracted on the phone, with progress | Fresh install → pick ISO → title screen |
| 4 | Touch virtual gamepad + overlay; lifecycle (surface recreate on resume, autosave on `onStop`, back → pause) | Norune walked, dungeon floor 1 fought, saved and reloaded, all with touch only; Bluetooth pad also works |
| 5 | CI: an `android` job like doukutsu-rs's (`sdkmanager --package_file`, `gradlew assembleRelease`, `apksigner` with secrets) | Signed APK artifact on every push |
| 6 | Rust takes over, unit by unit, as the C++ port did with `PORT_OVERRIDE`. Start with formats/`dcdata`, then audio, then game units from leaves to roots, each checked against the C++ build with `--input` replays and per-tick state hashes | Share of the code in Rust goes up; the APK keeps working the whole time |

Phases 1–5 give a playable APK with the game logic still in C++. Phase 6 is
the long Rust port, and it never stops the APK from building.

## 8. First week

1. Install the Android SDK, NDK r29+ and `rustup target add aarch64-linux-android`.
2. Copy `drsandroid/` to `android/`, then swap in SDL3, the
   `org.libsdl.app.SDLActivity` from SDL3's `android-project`, and a Rust
   `SDL_main` that calls `SDL_Vulkan_CreateSurface`.
3. Run it on the S26 and read the renderer's device report in
   `adb logcat -s RustStdoutStderr`.
4. Add `add_subdirectory(${CHRONICLE_ROOT})` and work through section 3 until it
   links.
