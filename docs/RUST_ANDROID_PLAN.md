# Rust port and Android APK plan

The goal: a Rust build of Chronicle that ships as an Android APK, the way
[doukutsu-rs](https://github.com/doukutsu-rs/doukutsu-rs) did for Cave Story.
That means a Rust engine that reads the player's own game data, has touch
controls, and builds with `cargo` and Gradle.

## 1. Starting point

| Layer | Where | Size | Notes |
|---|---|---|---|
| Decompiled game | `ps2/src` | ~167k lines C++ | Matching decomp. The PS2 build only, never edited for the port |
| Port overrides | `port/src` | ~68k lines C++ | `PORT_OVERRIDE` replacements for hardware-touching functions |
| Host layer `dc_host` | `tools/kazarov/port/{gfx,platform,audio,sce}` | ~19k lines C++ | Vulkan 1.3 renderer, SDL3 window/input, SPU2-style synth, SDK stubs. **Includes no game headers** |
| Data tool | `tools/dcdata` | | ISO → `data/`, normalized packs, `languages.json` |

Facts that shape the plan:

- **The 64-bit work is done.** `darkcloud` is PIE, its arenas sit above 4 GiB,
  and it already runs on arm64 macOS (`docs/DECOUPLING_PLAN.md`,
  `docs/MACOS.md`). Android arm64 is PIE-only, so that part is already handled.
- **`dc_host` is a clean seam.** `gfx/gfx.hpp` is the renderer's whole public
  API (74 entry points), and platform and audio are just as narrow. Anything
  that provides those APIs can host the game.
- **Determinism tooling exists.** `--input FILE` replays scripted pad input,
  `--headless`/`--offscreen` render without a window, and `ReadbackFrame`
  returns pixels. Together they make an oracle to diff a Rust translation
  against.
- **Mobile GPU problems.** The renderer needs Vulkan 1.3, `dualSrcBlend`
  (GS blend emulation, `draw.frag` `index = 1`), and 8192 update-after-bind
  sampled images per stage. Many Android devices have Vulkan 1.1, and some
  Mali and PowerVR drivers lack `dualSrcBlend` or have lower descriptor limits.
  **We can't just reuse the renderer as it is.**

## 2. What doukutsu-rs did, and what carries over

doukutsu-rs is a Rust reimplementation, not an emulator. It reads the original
data files, has its own renderer (GL/GLES) and a backend trait over the
platform (SDL2 on desktop, `ndk`/`jni` glue on Android), and keeps an
`app/` Gradle project that packages a Rust `cdylib` into an APK with touch
controls and a file picker for data.

What we copy: the backend trait, the Gradle-wraps-`cdylib` layout, the
"bring your own data" rule, and the touch overlay.

What we do differently: Cave Story is ~40k lines of original logic, while Dark
Cloud is ~235k lines of C++. A big-bang rewrite would take a long time to
reach anything playable. **We port incrementally (strangler fig) instead.**
Rust takes over from the outside in, the C++ keeps running until each piece is
replaced, and every replacement is checked against the C++ build.

## 3. Strategy

```
Phase 1          Phase 2                  Phase 3              Phase 4                 Phase 5
Rust formats  →  Rust host replaces    →  Android APK       →  Game units move     →  Pure Rust
+ dcdata-rs      dc_host (wgpu, audio,    (cargo-ndk, Gradle,  C++ → Rust, diffed      (C++ and clang
                 input) via C ABI;        touch, SAF)          against the oracle      dropped)
                 C++ game as staticlib
```

The APK comes in Phase 3, still with C++ game logic inside. From then on
every phase ships a playable APK.

## 4. Repository layout

A new repo (or a `rs/` tree here), as a Cargo workspace:

```
chronicle-rs/
  Cargo.toml                 workspace
  crates/
    dc-formats/              ISO9660, DATA.HD2/DAT, packs, TIM2, IMG, MDS/MDT, HD/BD/SQ, VAG, STB
    dc-data/                 dcdata in Rust: extract, normalize, fingerprint releases, languages.json
    dc-audio/                port of kazarov audio: VAG decode, synth, sequencer, reverb, mixer
    dc-gfx/                  wgpu renderer implementing the gfx.hpp contract
    dc-platform/             Backend trait: window, input, clock, paths, settings
      backend-sdl3/          desktop (sdl3 crate)
      backend-android/       android-activity (GameActivity) + ndk + jni
    dc-host-ffi/             extern "C" surface the C++ game links against (Phases 2–4)
    dc-game/                 Rust game units as they are translated (Phase 4+)
    chronicle/               desktop binary
    chronicle-android/       cdylib for the APK
  cpp/                       CMake build of ps2/src + port/src as libdcgame.a, no dc_host
  shim/                      thin C++ that keeps gfx.hpp/platform/audio signatures, forwards to Rust
  app/                       Android Gradle project (like doukutsu-rs/app)
  tools/oracle/              runs C++ and Rust builds on the same --input script and diffs them
```

## 5. Phases

### Phase 0: Ground rules (days)

- **License.** Chronicle has no `LICENSE` file at its root, so by default you
  can't redistribute it or derivatives. Kazarov is MIT. Before publishing an
  APK that contains any `ps2/src` or `port/src` translation, ask
  TheMoonPeople for a license or permission (their Discord is in the
  README). A Rust rewrite derived from their decomp is still a derivative.
- **No game data in the APK, ever.** The player supplies their own ISO, as
  with doukutsu-rs and the existing port's first-run flow.
- Pin the toolchain: stable Rust, NDK r27+, `cargo-ndk`, minSdk 26 (AAudio,
  Vulkan 1.0+), targetSdk current.

### Phase 1: Formats and data in Rust (2–4 weeks)

- `dc-formats`: zero-copy readers (`&[u8]`, little-endian, `zerocopy` or
  `binrw`), with no `unsafe` beyond the parsing crates.
- `dc-data`: a CLI matching `dcdata extract`, including NTSC filename mapping,
  palette fix-up for PAL weapon icon sheets, `KnownReleases` fingerprints, and
  `languages.json`.
- **Exit:** for PAL and NTSC ISOs, `dc-data extract` makes a tree that is
  byte-for-byte the same as C++ `dcdata`'s (CI diff). TIM2/MDT decoders match
  `tex_decode_test` fixtures.

This phase runs anywhere and needs no game binary. It's the safest place to
start.

### Phase 2: Rust host under the C++ game (6–10 weeks)

1. `cpp/CMakeLists.txt` builds `ps2/src` + `port/src` (with
   `port_override.py`, as now) into `libdcgame.a` without `dc_host`, and links
   against `shim/`.
2. `shim/` implements every `gfx::`, platform and audio function as a
   one-line forward to `extern "C"` functions exported by `dc-host-ffi`. Use
   `cbindgen` for headers; `cxx` only if a signature needs C++ types.
3. **`dc-gfx` on wgpu** (Vulkan, GLES 3.1, Metal, DX12). That covers Android
   devices that the Vulkan 1.3 renderer leaves out. Required redesigns:
   - **Dual-source blend:** use `wgpu::Features::DUAL_SOURCE_BLENDING` where
     the device has it. Otherwise fall back to two passes per affected draw,
     or to a shader that reads the previous colour (a copy of the target) for
     the few GS blend modes that need the second factor.
   - **Bindless textures:** use `TEXTURE_BINDING_ARRAY` where supported.
     Otherwise use a texture atlas/page cache with per-draw bind groups,
     batched by page. The game's VRAM-rectangle model maps well to pages.
   - Display lists, readback, snapshot, and present modes keep the semantics
     in `gfx/README.md`. That README is the spec.
4. `dc-audio`: translate kazarov `audio/` (~3k lines) and output through
   `cpal` (AAudio/Oboe on Android).
5. `dc-platform`: Backend trait; `backend-sdl3` first.
6. **Exit:** `chronicle --headless --input boot.script` gives frames that
   pass a perceptual diff (SSIM ≥ 0.99, exact on 2D-only screens) against
   `darkcloud` on lavapipe, for boot → title → Norune walk → dungeon floor 1.
   That runs in CI.

### Phase 3: Android APK (3–5 weeks, overlaps Phase 2's tail)

- `app/`: Gradle, `GameActivity` (AndroidX games), and a `cargo-ndk` task
  building `chronicle-android` for `arm64-v8a` (and `x86_64` for the
  emulator). `libdcgame.a` is cross-compiled with the NDK's clang (C++26
  needs NDK clang ≥ 19; check `std` features, since libc++ lags).
- **Data onboarding:** first run opens a Storage Access Framework picker for
  the ISO and runs `dc-data extract` from the SAF stream into
  `getFilesDir()/data`, with a progress UI. Saves go in `getFilesDir()/save`,
  and saves can be exported or imported through SAF.
- **Touch controls** (overlay drawn by `dc-gfx` in the 640×480 logical space,
  with a layout editor as in doukutsu-rs):
  - left: floating analog stick; right: ✕ ○ △ □ cluster; shoulders L1/R1/L2/R2
    on the top edges; Start/Select at the top centre.
  - camera: drag on the right half of the screen (feeds the existing
    mouse-look path).
  - menus: the port already accepts a mouse pointer on the Options and naming
    screens (`menu_mouse`, `name_mouse`), so tap goes through that path.
  - Georama editor: one-finger drag moves the cursor, two-finger rotates and
    zooms the camera.
  - The overlay hides automatically when a controller connects (SDL/Android
    gamepad mapping; the glyph sets in `port/glyphs` already cover Xbox, PS
    and Switch).
- **Lifecycle:** handle surface loss on pause (recreate the wgpu surface),
  autosave on `onStop` through the existing save-state path, request audio
  focus, keep the screen on while playing, and handle back-button → pause
  menu.
- **Performance budget:** 60 ticks/s on a Snapdragon 7-series or Mali-G610
  class device, under 1.5 GB RSS. Profile the arena sizes in
  `port/src/dataset.cpp`.
- **Exit:** a signed release APK installs on 3 reference devices (one each of
  Adreno, Mali, and PowerVR or Xclipse). It extracts a PAL ISO, reaches Norune,
  saves, survives being backgrounded, and plays with touch and with a
  Bluetooth pad. CI uploads the APK as an artifact.

### Phase 4: Moving game units to Rust (ongoing, the long part)

Per unit, the same loop as the C++ port's `PORT_OVERRIDE` (this is how
Chronicle itself got here):

1. Pick a unit. Draft with `c2rust` from the preprocessed `ps2/src` copy, then
   rewrite it into safe, idiomatic Rust in `dc-game`, keeping retail names.
2. Export the unit's public functions with `#[no_mangle] extern "C"` under
   the mangled names (or through a shim) and drop the C++ definitions with
   `port_override.py`. Game state stays in the C++ globals until the unit's
   owner moves, with access through `dc-host-ffi` accessors.
3. **Oracle gate:** `tools/oracle` runs both builds on the input-script corpus
   and compares a per-tick hash of game state (player, RNG seed, script VM
   PC/stack, entity tables) plus framebuffers. Any divergence fails CI.

Order, from leaves to roots: `mathutil`/`vu0_ops` → collision (`collisionmdt`,
`bound`) → data loaders (`dataread`, `chararead`, `itemdata`) → script VM
(`runscript`, `scriptinterpreter`, `editmapscript`) → effects → menus →
battle (`ebattle`, `btsysscript`, `monstorunit`) → Georama (`edit*`) → town
and dungeon loops (`gameloop`, `dun/*`) → title overlay → `main`.

Track progress the way decomp.dev tracks matching: the share of functions in
Rust, shown in the README.

### Phase 5: Pure Rust (when Phase 4 hits 100%)

Delete `cpp/` and `shim/`, so neither Android nor desktop needs clang. Turn
`dc-host-ffi` into ordinary Rust modules, and move globals into owned
`Game` state. Optional extras once the code is Rust, as in doukutsu-rs:
widescreen camera, higher internal resolution, and mod/translation loading.

## 6. Risks

| Risk | Mitigation |
|---|---|
| Mobile GPUs lack dual-source blend or bindless | wgpu feature detection with the fallbacks in Phase 2.3. Test on Mali early |
| C++26 not fully supported by NDK libc++ | Build `libdcgame.a` with the NDK's clang and its own libc++ (`c++_static`). Patch the few gaps in `port/src` |
| The game assumes desktop RAM | Measure arenas. Use mmap-backed reads for packs instead of loading them whole |
| Rust divergence hard to find | Per-tick state hashes, plus bisection by tick in `tools/oracle` |
| Upstream Chronicle keeps changing | Follow upstream as a submodule. Phase 4 translates only units whose upstream code has settled |
| Licensing | Phase 0: get permission before any public release |

## 7. First week

1. Create the workspace, CI (fmt, clippy, test), and `dc-formats` with the
   ISO9660 and `DATA.HD2`/`DATA.DAT` readers.
2. Port `dcdata`'s file-index and pack reader. Diff its output against
   `port/build/pc/dcdata extract` on your ISO.
3. Read `tools/kazarov/port/gfx/README.md` end to end and turn it into the
   `dc-gfx` trait and test list.
4. Run the C++ port on one Android device by hand (SDL3 + its Vulkan
   renderer) to collect real device-capability data. It costs a day and
   confirms whether the Phase 2.3 fallbacks are needed.
