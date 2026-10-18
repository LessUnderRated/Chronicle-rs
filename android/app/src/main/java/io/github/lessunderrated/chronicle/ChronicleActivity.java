package io.github.lessunderrated.chronicle;

import org.libsdl.app.SDLActivity;

/**
 * SDL's activity, loading libSDL3.so and libmain.so, whose SDL_main (android/src/lib.rs) runs the game.
 */
public class ChronicleActivity extends SDLActivity {
    @Override
    protected String[] getLibraries() {
        return new String[] {"SDL3", "main"};
    }
}
