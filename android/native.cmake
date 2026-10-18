# The Android build's dependencies, included by port/CMakeLists.txt when the NDK toolchain is in use
# (docs/ANDROID.md). Gradle's externalNativeBuild runs the repository's own CMakeLists.txt, so
# CMAKE_SOURCE_DIR stays the checkout's root as on every other platform.
#
# SDL3 is built from source, as doukutsu-rs builds SDL2: its Java glue (org.libsdl.app) is compiled
# into the APK from the same checkout, so the two always match. The Rust crate in android/ is built
# by Corrosion and linked into libmain.so, the library SDLActivity loads; it defines SDL_main.

include_guard(GLOBAL)
include(FetchContent)

set(SDL_SHARED ON CACHE BOOL "" FORCE)
set(SDL_STATIC OFF CACHE BOOL "" FORCE)
set(SDL_TEST_LIBRARY OFF CACHE BOOL "" FORCE)
set(SDL_EXAMPLES OFF CACHE BOOL "" FORCE)
# app/build.gradle.kts clones the same tag and passes it as FETCHCONTENT_SOURCE_DIR_SDL3.
file(STRINGS ${CMAKE_SOURCE_DIR}/android/sdl3-tag.txt sdl3_tag LIMIT_COUNT 1)
FetchContent_Declare(SDL3
    GIT_REPOSITORY https://github.com/libsdl-org/SDL.git
    GIT_TAG ${sdl3_tag}
    GIT_SHALLOW TRUE)
FetchContent_MakeAvailable(SDL3)

# The NDK sysroot carries the Vulkan 1.4 headers and libvulkan.so, the system loader; FindVulkan
# looks for a desktop SDK instead.
if(NOT TARGET Vulkan::Vulkan)
    add_library(Vulkan::Vulkan INTERFACE IMPORTED)
    set_target_properties(Vulkan::Vulkan PROPERTIES INTERFACE_LINK_LIBRARIES vulkan)
endif()

FetchContent_Declare(Corrosion
    GIT_REPOSITORY https://github.com/corrosion-rs/corrosion.git
    GIT_TAG v0.6.1
    GIT_SHALLOW TRUE)
FetchContent_MakeAvailable(Corrosion)
corrosion_import_crate(MANIFEST_PATH ${CMAKE_SOURCE_DIR}/android/Cargo.toml
    CRATES chronicle-android
    CRATE_TYPES staticlib
    PROFILE release)
