package io.github.lessunderrated.chronicle;

/**
 * The native side the overlay and the menu call (android/src/jni.rs, registered when libmain.so
 * loads). Sizes and places are in the window's pixels.
 */
final class Native {
    private Native() {}

    /** The default touch layout for a window and its safe-area insets, as JSON. */
    static native String defaultLayout(int width, int height, float left, float top, float right, float bottom);

    /** Installs the default layout with the player's arrangement over it (null: none) and returns it. */
    static native String setLayout(int width, int height, float left, float top, float right, float bottom, String saved);

    /** A touch on the controls: action 0 down, 1 move, 2 up or cancelled. */
    static native void touch(int action, long pointer, float x, float y);

    /** What is pressed, as JSON: buttons (bits), the sticks held, and whether the touch pad is in use. */
    static native String padState();

    static native void setPaused(boolean paused);

    static native void setSpeed(int speed);

    static native int speed();

    /** A setting by its config.json name, with its value as JSON text; the game applies it between ticks. */
    static native void setSetting(String name, String jsonValue);

    /** Imports the Dark Cloud saves on the memory card behind fd (closed by it) into saves; JSON. */
    static native String importCard(int fd, String saves);
}
