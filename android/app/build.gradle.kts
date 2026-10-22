// The APK: SDL's Java activity, libmain.so (the game, built by the repository's own CMakeLists.txt with
// the Rust crate in android/ linked in) and libSDL3.so. Laid out as doukutsu-rs's drsandroid/app
// (reference/doukutsu-rs). docs/ANDROID.md has the whole build.

import java.security.MessageDigest

plugins {
    id("com.android.application")
}

val repoRoot: File = rootDir.parentFile

// One SDL checkout serves both halves: its Java glue is compiled here and CMake builds the library from
// it (FETCHCONTENT_SOURCE_DIR_SDL3), so the two cannot disagree. android/native.cmake pins the same tag.
val sdlTag = File(rootDir, "sdl3-tag.txt").readText().trim()
val sdlDir = File(rootDir, "build/sdl3/$sdlTag")
if (!File(sdlDir, "CMakeLists.txt").exists()) {
    sdlDir.parentFile.mkdirs()
    val clone = ProcessBuilder("git", "clone", "--depth", "1", "--branch", sdlTag,
        "https://github.com/libsdl-org/SDL.git", sdlDir.path).inheritIO().start()
    check(clone.waitFor() == 0) { "cloning SDL $sdlTag failed" }
}

// The port's own files, which the game reads from save/lang and save/glyphs; android/src/storage.rs
// installs them from the APK's assets by the list this writes.
abstract class ChronicleAssets : DefaultTask() {
    @get:InputDirectory abstract val lang: DirectoryProperty
    @get:InputDirectory abstract val glyphs: DirectoryProperty
    @get:InputFile abstract val font: RegularFileProperty
    @get:InputFile abstract val fontThin: RegularFileProperty
    @get:OutputDirectory abstract val outputDir: DirectoryProperty

    @TaskAction
    fun run() {
        val root = outputDir.get().asFile
        root.deleteRecursively()
        lang.get().asFile.copyRecursively(File(root, "lang"))
        glyphs.get().asFile.copyRecursively(File(root, "glyphs"))
        font.get().asFile.copyTo(File(root, "lang/font.ttf"))
        fontThin.get().asFile.copyTo(File(root, "lang/font_thin.ttf"))
        val files = root.walkTopDown().filter { it.isFile }.map { it.relativeTo(root).invariantSeparatorsPath }.sorted().toList()
        val digest = MessageDigest.getInstance("SHA-256")
        files.forEach { digest.update(File(root, it).readBytes()) }
        val hash = digest.digest().joinToString("") { "%02x".format(it) }
        File(root, "chronicle-assets.txt").writeText("# $hash\n" + files.joinToString("\n") + "\n")
    }
}

val chronicleAssets = tasks.register<ChronicleAssets>("chronicleAssets") {
    lang.set(File(repoRoot, "port/lang"))
    glyphs.set(File(repoRoot, "port/glyphs"))
    font.set(File(repoRoot, "tools/font/DarkCloudCompendium.ttf"))
    fontThin.set(File(repoRoot, "tools/font/DarkCloudCompendiumThin.ttf"))
}

androidComponents {
    onVariants { variant ->
        variant.sources.assets?.addGeneratedSourceDirectory(chronicleAssets, ChronicleAssets::outputDir)
    }
}

android {
    namespace = "io.github.lessunderrated.chronicle"
    compileSdk = 36
    buildToolsVersion = "36.0.0"
    ndkVersion = "30.0.16248370"

    defaultConfig {
        applicationId = "io.github.lessunderrated.chronicle"
        // Vulkan 1.3's entry points are in the NDK's libvulkan.so from API 33; the renderer needs 1.3.
        minSdk = 33
        targetSdk = 36
        versionCode = 25
        versionName = "0.7.8"

        ndk {
            abiFilters += listOf("arm64-v8a")
            (project.findProperty("chronicle.abis") as String?)?.let { abiFilters.clear(); abiFilters += it.split(",") }
        }
        externalNativeBuild {
            cmake {
                arguments += listOf(
                    "-DPLATFORM=PC",
                    "-DDC_BUILD_TESTS=OFF",
                    "-DANDROID_STL=c++_shared",
                    "-DFETCHCONTENT_SOURCE_DIR_SDL3=${sdlDir.path}",
                    // C++26 has CMake scan for modules with any clang-scan-deps it finds, which may be
                    // the host's, not the NDK's, and fails on SDL's precompiled header; nothing here is a
                    // module.
                    "-DCMAKE_CXX_SCAN_FOR_MODULES=OFF",
                )
                targets += listOf("darkcloud", "SDL3-shared")
            }
        }

        val documentsAuthority = "$applicationId.documents"
        manifestPlaceholders["documentsAuthority"] = documentsAuthority
        buildConfigField("String", "DOCUMENTS_AUTHORITY", "\"$documentsAuthority\"")
    }

    signingConfigs {
        // A release is signed by the key CI is given. Without one, by chronicle-dev.jks, which is in
        // the repository: public, so it proves nothing about who built an APK, but every build signed
        // with it installs over the last one, keeping the extracted disc and the saves.
        create("release") {
            val keystore = System.getenv("CHRONICLE_KEYSTORE")
            if (keystore != null) {
                storeFile = file(keystore)
                storePassword = System.getenv("CHRONICLE_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("CHRONICLE_KEY_ALIAS")
                keyPassword = System.getenv("CHRONICLE_KEY_PASSWORD")
            } else {
                storeFile = file("chronicle-dev.jks")
                storePassword = "chronicle"
                keyAlias = "chronicle"
                keyPassword = "chronicle"
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.getByName("release")
        }
        debug {
            applicationIdSuffix = ".debug"
            isJniDebuggable = true
            // The game at -O0 cannot keep 60 ticks a second.
            externalNativeBuild { cmake { arguments += "-DCMAKE_BUILD_TYPE=RelWithDebInfo" } }
            val documentsAuthority = "io.github.lessunderrated.chronicle.debug.documents"
            manifestPlaceholders["documentsAuthority"] = documentsAuthority
            buildConfigField("String", "DOCUMENTS_AUTHORITY", "\"$documentsAuthority\"")
        }
    }

    buildFeatures {
        buildConfig = true
    }

    sourceSets {
        getByName("main") {
            java.srcDirs(File(sdlDir, "android-project/app/src/main/java"))
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    externalNativeBuild {
        cmake {
            path = File(repoRoot, "CMakeLists.txt")
            version = "3.31.6"
        }
    }
}
