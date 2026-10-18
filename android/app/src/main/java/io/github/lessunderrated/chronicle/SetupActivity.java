package io.github.lessunderrated.chronicle;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.Intent;
import android.database.Cursor;
import android.graphics.Color;
import android.net.Uri;
import android.os.Bundle;
import android.os.ParcelFileDescriptor;
import android.os.SystemClock;
import android.provider.OpenableColumns;
import android.view.Gravity;
import android.view.View;
import android.view.WindowManager;
import android.widget.ImageView;
import android.widget.LinearLayout;
import android.widget.ProgressBar;
import android.widget.TextView;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.io.File;
import java.io.IOException;
import java.util.Locale;

/**
 * The launcher: the game's data comes from the player's own Dark Cloud disc, so before the game
 * starts the first time this asks for a disc image made from it, shows which release it is, and
 * extracts its data into the app's storage. The work is native (android/src/setup.rs, which calls
 * the extractor the PC port uses); this is the dialogs around it. Afterwards it opens the game at once.
 */
public class SetupActivity extends Activity {
    private static final int PICK_DISC = 1;

    /** What the native side reports while it copies or extracts; false stops it. */
    public interface Progress {
        boolean update(long done, long total, long bytes, long totalBytes);
    }

    private static boolean sLoaded;

    private static native String nativeOpenDisc(int fd, String copyDir, Progress progress);
    private static native String nativeIdentify(String path);
    private static native String nativeExtract(String path, String dataDir, Progress progress);

    private static synchronized void loadNative() {
        if (!sLoaded) {
            System.loadLibrary("SDL3");
            System.loadLibrary("main");
            sLoaded = true;
        }
    }

    private File dataDir() {
        return new File(getFilesDir(), "data");
    }

    private File copyDir() {
        return new File(getFilesDir(), "disc");
    }

    /** The extractor writes languages.json last of all, so its presence means the data is whole. */
    private boolean dataReady() {
        return new File(dataDir(), "languages.json").isFile();
    }

    private volatile boolean cancelled;
    private String discPath;
    private int discFd = -1;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        if (dataReady()) {
            launchGame();
            return;
        }
        setContentView(backdrop());
        showWelcome();
    }

    private View backdrop() {
        LinearLayout layout = new LinearLayout(this);
        layout.setOrientation(LinearLayout.VERTICAL);
        layout.setGravity(Gravity.CENTER);
        layout.setBackgroundColor(Color.rgb(0x0E, 0x10, 0x18));
        ImageView icon = new ImageView(this);
        icon.setImageResource(R.mipmap.ic_launcher);
        layout.addView(icon, new LinearLayout.LayoutParams(dp(96), dp(96)));
        TextView title = new TextView(this);
        title.setText(R.string.app_name);
        title.setTextColor(Color.WHITE);
        title.setTextSize(28);
        title.setGravity(Gravity.CENTER);
        layout.addView(title);
        return layout;
    }

    private int dp(int value) {
        return Math.round(value * getResources().getDisplayMetrics().density);
    }

    // ---- Dialogs ------------------------------------------------------------------------------

    private AlertDialog.Builder dialog() {
        return new AlertDialog.Builder(this, android.R.style.Theme_DeviceDefault_Dialog_Alert).setCancelable(false);
    }

    private void showWelcome() {
        dialog()
            .setTitle(R.string.setup_title)
            .setMessage(R.string.setup_message)
            .setPositiveButton(R.string.setup_choose, (d, w) -> chooseDisc())
            .setNegativeButton(R.string.setup_quit, (d, w) -> finish())
            .show();
    }

    private void showError(String title, String message) {
        dialog()
            .setTitle(title)
            .setMessage(message)
            .setPositiveButton(R.string.setup_choose_another, (d, w) -> chooseDisc())
            .setNegativeButton(R.string.setup_quit, (d, w) -> finish())
            .show();
    }

    private void chooseDisc() {
        forgetDisc();
        Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        // Most providers call a disc image application/octet-stream, so nothing is filtered out.
        intent.setType("*/*");
        startActivityForResult(intent, PICK_DISC);
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode != PICK_DISC) {
            return;
        }
        if (resultCode != RESULT_OK || data == null || data.getData() == null) {
            showWelcome();
            return;
        }
        openDisc(data.getData());
    }

    // ---- Reading the disc ---------------------------------------------------------------------

    private static final class Busy {
        final AlertDialog dialog;
        final TextView status;
        final ProgressBar bar;

        Busy(AlertDialog dialog, TextView status, ProgressBar bar) {
            this.dialog = dialog;
            this.status = status;
            this.bar = bar;
        }
    }

    private Busy showBusy(int title, String status, boolean cancellable) {
        LinearLayout layout = new LinearLayout(this);
        layout.setOrientation(LinearLayout.VERTICAL);
        layout.setPadding(dp(24), dp(16), dp(24), dp(8));
        TextView text = new TextView(this);
        text.setText(status);
        layout.addView(text);
        ProgressBar bar = new ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal);
        bar.setMax(1000);
        bar.setIndeterminate(true);
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT,
            LinearLayout.LayoutParams.WRAP_CONTENT);
        params.topMargin = dp(12);
        layout.addView(bar, params);
        AlertDialog.Builder builder = dialog().setTitle(title).setView(layout);
        cancelled = false;
        if (cancellable) {
            builder.setNegativeButton(android.R.string.cancel, (d, w) -> cancelled = true);
        }
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        return new Busy(builder.show(), text, bar);
    }

    private void endBusy(Busy busy) {
        getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        busy.dialog.dismiss();
    }

    /** A Progress that updates busy at most ten times a second. */
    private Progress progressFor(Busy busy, boolean copying) {
        long[] last = {0};
        return (done, total, bytes, totalBytes) -> {
            long now = SystemClock.uptimeMillis();
            if (now - last[0] >= 100 || (total > 0 && done == total)) {
                last[0] = now;
                runOnUiThread(() -> {
                    if (totalBytes > 0) {
                        busy.bar.setIndeterminate(false);
                        busy.bar.setProgress((int) (bytes * 1000 / totalBytes));
                    }
                    busy.status.setText(copying
                        ? getString(R.string.setup_copying, megabytes(bytes), megabytes(totalBytes))
                        : getString(R.string.setup_extracting, done, total, megabytes(bytes), megabytes(totalBytes)));
                });
            }
            return !cancelled;
        };
    }

    private void openDisc(Uri uri) {
        Busy busy = showBusy(R.string.setup_reading, displayName(uri), true);
        new Thread(() -> {
            String path = null;
            String failure = null;
            int fd = -1;
            try {
                loadNative();
                ParcelFileDescriptor pfd = getContentResolver().openFileDescriptor(uri, "r");
                if (pfd == null) {
                    failure = getString(R.string.setup_unreadable);
                } else {
                    fd = pfd.detachFd();
                    path = nativeOpenDisc(fd, copyDir().getPath(), progressFor(busy, true));
                    if (path == null) {
                        failure = getString(cancelled ? R.string.setup_cancelled : R.string.setup_unreadable);
                    }
                }
            } catch (IOException | SecurityException | UnsatisfiedLinkError e) {
                failure = e.getMessage();
            }
            String json = path != null ? nativeIdentify(path) : null;
            final String chosen = path;
            final int chosenFd = fd;
            final String why = failure;
            runOnUiThread(() -> {
                endBusy(busy);
                discPath = chosen;
                // The native side keeps the descriptor only for a /proc/self/fd path; otherwise it read
                // the disc into a copy and closed it.
                discFd = chosen != null && chosen.startsWith("/proc/self/fd/") ? chosenFd : -1;
                if (chosen == null) {
                    if (cancelled) {
                        showWelcome();
                    } else {
                        showError(getString(R.string.setup_unreadable), why != null ? why : "");
                    }
                    return;
                }
                showDisc(json);
            });
        }, "disc-open").start();
    }

    private static final String[] LANGUAGES = {
        "Japanese", "English (US)", "English (UK)", "French", "German", "Italian", "Spanish",
    };

    private void showDisc(String json) {
        JSONObject info;
        try {
            info = new JSONObject(json);
        } catch (JSONException e) {
            showError(getString(R.string.setup_not_dark_cloud), json);
            return;
        }
        if (info.has("error")) {
            showError(getString(R.string.setup_not_dark_cloud), info.optString("error"));
            return;
        }
        String region = info.optString("region");
        String serial = info.optString("serial");
        long bytes = info.optLong("bytes");
        StringBuilder message = new StringBuilder();
        if (!serial.isEmpty()) {
            message.append(getString(R.string.setup_serial, serial)).append('\n');
        }
        message.append(getString(R.string.setup_release, info.optString("release"))).append('\n');
        JSONArray languages = info.optJSONArray("languages");
        StringBuilder names = new StringBuilder();
        for (int i = 0; languages != null && i < languages.length(); i++) {
            int code = languages.optInt(i, -1);
            if (code >= 0 && code < LANGUAGES.length) {
                names.append(names.length() > 0 ? ", " : "").append(LANGUAGES[code]);
            }
        }
        message.append(getString(R.string.setup_languages, names.toString())).append('\n');
        message.append(getString(R.string.setup_size, info.optLong("files"), gigabytes(bytes)));
        if (region.equals("Japan")) {
            message.append("\n\n").append(getString(R.string.setup_japanese_note));
        }
        long free = getFilesDir().getUsableSpace();
        long needed = bytes + (64L << 20);
        AlertDialog.Builder builder = dialog().setTitle(getString(R.string.setup_disc_title, region));
        if (free < needed) {
            message.append("\n\n").append(getString(R.string.setup_no_space, gigabytes(needed), gigabytes(free)));
            builder.setPositiveButton(R.string.setup_choose_another, (d, w) -> chooseDisc());
        } else {
            message.append("\n\n").append(getString(R.string.setup_extract_question));
            builder.setPositiveButton(R.string.setup_extract, (d, w) -> extract())
                .setNeutralButton(R.string.setup_choose_another, (d, w) -> chooseDisc());
        }
        builder.setMessage(message.toString()).setNegativeButton(R.string.setup_quit, (d, w) -> finish()).show();
    }

    // ---- Extracting ---------------------------------------------------------------------------

    private void extract() {
        Busy busy = showBusy(R.string.setup_extract_title, getString(R.string.setup_starting), true);
        String path = discPath;
        new Thread(() -> {
            String error = nativeExtract(path, dataDir().getPath(), progressFor(busy, false));
            runOnUiThread(() -> {
                endBusy(busy);
                if (error == null && dataReady()) {
                    forgetDisc();
                    launchGame();
                } else if (cancelled) {
                    showDisc(nativeIdentify(path));
                } else {
                    showError(getString(R.string.setup_failed), error != null ? error : "");
                }
            });
        }, "disc-extract").start();
    }

    /** Lets go of the picked file, and of the copy made of it. */
    private void forgetDisc() {
        if (discFd >= 0) {
            closeFd(discFd);
            discFd = -1;
        }
        File copy = new File(copyDir(), "disc.iso");
        if (copy.exists()) {
            copy.delete();
        }
        discPath = null;
    }

    private static void closeFd(int fd) {
        try {
            ParcelFileDescriptor.adoptFd(fd).close();
        } catch (IOException ignored) {
        }
    }

    private void launchGame() {
        startActivity(new Intent(this, ChronicleActivity.class));
        finish();
        overridePendingTransition(0, 0);
    }

    // ---- Text ---------------------------------------------------------------------------------

    private String displayName(Uri uri) {
        try (Cursor cursor = getContentResolver().query(uri, new String[] {OpenableColumns.DISPLAY_NAME}, null, null, null)) {
            if (cursor != null && cursor.moveToFirst()) {
                return cursor.getString(0);
            }
        } catch (RuntimeException ignored) {
        }
        return uri.getLastPathSegment();
    }

    private static String megabytes(long bytes) {
        return String.format(Locale.getDefault(), "%,d", bytes >> 20);
    }

    private static String gigabytes(long bytes) {
        return String.format(Locale.getDefault(), "%.1f", bytes / 1e9);
    }
}
