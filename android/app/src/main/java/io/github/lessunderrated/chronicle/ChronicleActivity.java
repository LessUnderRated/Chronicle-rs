package io.github.lessunderrated.chronicle;

import android.app.AlertDialog;
import android.content.Intent;
import android.content.pm.ActivityInfo;
import android.graphics.Color;
import android.net.Uri;
import android.os.Bundle;
import android.os.ParcelFileDescriptor;
import android.view.Gravity;
import android.view.KeyEvent;
import android.view.ViewGroup;
import android.widget.Button;
import android.widget.FrameLayout;
import android.widget.LinearLayout;
import android.widget.SeekBar;
import android.widget.TextView;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;

import org.libsdl.app.SDLActivity;

/**
 * SDL's activity, loading libSDL3.so and libmain.so, whose SDL_main (android/src/lib.rs) runs the game,
 * with the touch controls over it and the app's own menu: speed, graphics and video settings, the
 * controls' editor, and saves brought in from a PS2 memory card. The game holds still while the menu
 * is open.
 */
public class ChronicleActivity extends SDLActivity implements TouchOverlay.Listener {
    private static final int PICK_CARD = 0x4C43;

    private TouchOverlay overlay;
    private LinearLayout editPanel;
    private TextView editLabel;
    private SeekBar editSize;
    private SeekBar editOpacity;
    private boolean menuOpen;

    @Override
    protected String[] getLibraries() {
        return new String[] {"SDL3", "main"};
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        // Opened by the setup screen already, unless the game was started some other way.
        CrashReport.open(this);
        widenTheTitleOnce();
        overlay = new TouchOverlay(this, this);
        addContentView(overlay, new ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.MATCH_PARENT));
        buildEditPanel();
        if (savedInstanceState == null) {
            overlay.post(this::offerLastState);
        }
    }

    private int dp(int value) {
        return Math.round(value * getResources().getDisplayMetrics().density);
    }

    /**
     * Before 0.5.5 the title and the opening kept to 4:3 by default, which config.json then saved; an
     * update brings them to the wide screen once, and the menu's Title & opening still goes back.
     */
    private void widenTheTitleOnce() {
        android.content.SharedPreferences prefs = getSharedPreferences("defaults", MODE_PRIVATE);
        if (prefs.getInt("title-aspect", 0) < 1) {
            Native.setSetting("video.title_aspect", "\"wide\"");
            prefs.edit().putInt("title-aspect", 1).apply();
        }
    }

    /**
     * Landscape only, either way up. SDL asks for an orientation when it makes the window, and for a
     * resizable window without a hint that is any the player allows, portrait too; the manifest's
     * sensorLandscape stands instead.
     */
    @Override
    public void setOrientationBis(int w, int h, boolean resizable, String hint) {
        setRequestedOrientation(ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE);
    }

    /** Back opens the menu rather than leaving the game. */
    @Override
    public boolean dispatchKeyEvent(KeyEvent event) {
        if (event.getKeyCode() == KeyEvent.KEYCODE_BACK) {
            if (event.getAction() == KeyEvent.ACTION_UP) {
                if (editPanel.getVisibility() == android.view.View.VISIBLE) {
                    finishEditing();
                } else if (!menuOpen) {
                    onMenu();
                }
            }
            return true;
        }
        return super.dispatchKeyEvent(event);
    }

    private AlertDialog.Builder dialog() {
        return new AlertDialog.Builder(this, android.R.style.Theme_DeviceDefault_Dialog_Alert);
    }

    // ---- The menu -----------------------------------------------------------------------------

    @Override
    public void onMenu() {
        menuOpen = true;
        Native.setPaused(true);
        String speed = Native.speed() == 2 ? getString(R.string.menu_speed_fast) : getString(R.string.menu_speed_normal);
        String controls = overlay.hidden() ? getString(R.string.menu_show_controls) : getString(R.string.menu_hide_controls);
        String[] items = {
            getString(R.string.menu_resume),
            speed,
            getString(R.string.menu_settings),
            getString(R.string.menu_edit_controls),
            controls,
            getString(R.string.menu_save_state),
            getString(R.string.menu_load_state),
            getString(R.string.menu_import_card),
            getString(R.string.menu_share_log),
            getString(R.string.menu_quit),
        };
        dialog()
            .setTitle(R.string.app_name)
            .setItems(items, (d, which) -> {
                switch (which) {
                    case 1:
                        Native.setSpeed(Native.speed() == 2 ? 1 : 2);
                        overlay.invalidate();
                        onMenu();
                        return;
                    case 2:
                        showSettings();
                        return;
                    case 3:
                        startEditing();
                        return;
                    case 4:
                        overlay.setHidden(!overlay.hidden());
                        resume();
                        return;
                    case 5:
                        chooseStateSlot(true);
                        return;
                    case 6:
                        chooseStateSlot(false);
                        return;
                    case 7:
                        pickCard();
                        return;
                    case 8:
                        CrashReport.share(this, CrashReport.currentRun(this));
                        resume();
                        return;
                    case 9:
                        finishAffinity();
                        System.exit(0);
                        return;
                    default:
                        resume();
                }
            })
            .setOnCancelListener(d -> resume())
            .show();
    }

    /** The quick save and load: three slots in the save folder, one file each. */
    private static final int STATE_SLOTS = 3;

    private File stateFile(int slot) {
        return new File(new File(getFilesDir(), "save/states"), "slot-" + slot + ".dcst");
    }

    /** The slot whose file was written last, or -1 when none exist. */
    private int lastSavedSlot() {
        long newest = -1;
        int slot = -1;
        for (int i = 0; i < STATE_SLOTS; i++) {
            File file = stateFile(i);
            if (file.isFile() && file.lastModified() > newest) {
                newest = file.lastModified();
                slot = i;
            }
        }
        return slot;
    }

    /** On a fresh start, offer the last saved slot before the game runs under the player. */
    private void offerLastState() {
        int slot = lastSavedSlot();
        if (slot < 0 || isFinishing()) {
            return;
        }
        Native.setPaused(true);
        menuOpen = true;
        dialog()
            .setTitle(R.string.state_offer_title)
            .setMessage(getString(R.string.state_offer_message, slot + 1))
            .setPositiveButton(R.string.state_offer_load, (d, w) -> {
                Native.setSetting("state.request", "\"load:" + slot + "\"");
                resume();
            })
            .setNegativeButton(R.string.state_offer_skip, (d, w) -> resume())
            .setOnCancelListener(d -> resume())
            .show();
    }

    private void chooseStateSlot(boolean save) {
        String[] rows = new String[STATE_SLOTS];
        for (int slot = 0; slot < STATE_SLOTS; slot++) {
            File file = stateFile(slot);
            rows[slot] = !save && !file.exists()
                             ? getString(R.string.state_slot_empty, slot + 1)
                             : getString(R.string.state_slot, slot + 1);
        }
        dialog()
            .setTitle(save ? R.string.menu_save_state : R.string.menu_load_state)
            .setItems(rows, (d, which) -> {
                if (!save && !stateFile(which).exists()) {
                    resume();
                    return;
                }
                Native.setSetting("state.request", "\"" + (save ? "save" : "load") + ":" + which + "\"");
                resume();
            })
            .setNegativeButton(R.string.menu_back, (d, w) -> onMenu())
            .setOnCancelListener(d -> onMenu())
            .show();
    }

    private void resume() {
        menuOpen = false;
        Native.setPaused(false);
    }

    @Override
    public void onSpeedChanged(int speed) {
        overlay.invalidate();
    }

    // ---- Settings -----------------------------------------------------------------------------

    /** A setting the menu offers: its config.json name, and its choices as labels and JSON values. */
    private static final class Setting {
        final int label;
        final String key;
        final String[] names;
        final String[] values;
        final String fallback;

        Setting(int label, String key, String fallback, String[] names, String[] values) {
            this.label = label;
            this.key = key;
            this.fallback = fallback;
            this.names = names;
            this.values = values;
        }
    }

    private static String[] percent(int from, int to, int step) {
        int n = (to - from) / step + 1;
        String[] out = new String[n];
        for (int i = 0; i < n; i++) {
            out[i] = (from + i * step) + "%";
        }
        return out;
    }

    private static String[] scales(int from, int to, int step) {
        int n = (to - from) / step + 1;
        String[] out = new String[n];
        for (int i = 0; i < n; i++) {
            out[i] = String.valueOf((from + i * step) / 100.0);
        }
        return out;
    }

    private static final String[] ON_OFF = {"On", "Off"};
    private static final String[] TRUE_FALSE = {"true", "false"};

    private static final Setting[] SETTINGS = {
        new Setting(R.string.setting_aspect, "video.aspect", "\"auto\"", new String[] {"Wide (fills the screen)", "4:3"},
            new String[] {"\"auto\"", "\"4:3\""}),
        new Setting(R.string.setting_title_aspect, "video.title_aspect", "\"wide\"", new String[] {"Wide", "4:3"},
            new String[] {"\"wide\"", "\"4:3\""}),
        new Setting(R.string.setting_cover, "video.cover", "\"fit\"", new String[] {"4:3 with bars", "Stretched"},
            new String[] {"\"fit\"", "\"stretch\""}),
        new Setting(R.string.setting_ui_scale, "video.ui_scale", "1.0", percent(50, 200, 10), scales(50, 200, 10)),
        new Setting(R.string.setting_max_fps, "video.max_fps", "0", new String[] {"As the screen allows", "30", "60", "90", "120"},
            new String[] {"0", "30", "60", "90", "120"}),
        new Setting(R.string.setting_present, "video.present_mode", "\"fifo\"", new String[] {"On", "Fast", "Off"},
            new String[] {"\"fifo\"", "\"mailbox\"", "\"immediate\""}),
        new Setting(R.string.setting_interpolation, "video.interpolation", "true", ON_OFF, TRUE_FALSE),
        new Setting(R.string.setting_water_flat, "video.water_flat", "false", ON_OFF, TRUE_FALSE),
        new Setting(R.string.setting_anisotropy, "video.anisotropy", "0", new String[] {"Off", "2x", "4x", "8x", "16x"},
            new String[] {"0", "2", "4", "8", "16"}),
        new Setting(R.string.setting_soft_focus, "video.soft_focus", "true", ON_OFF, TRUE_FALSE),
        new Setting(R.string.setting_text_font, "video.text_font", "\"sharp\"", new String[] {"Sharp", "Original"},
            new String[] {"\"sharp\"", "\"original\""}),
        new Setting(R.string.setting_show_fps, "video.show_fps", "false", ON_OFF, TRUE_FALSE),
        new Setting(R.string.setting_volume, "audio.master_volume", "1.0", percent(0, 100, 10), scales(0, 100, 10)),
    };

    /** The game's config.json, which every change rewrites. */
    private JSONObject config() {
        try {
            File file = new File(getFilesDir(), "save/config.json");
            return new JSONObject(new String(Files.readAllBytes(file.toPath()), StandardCharsets.UTF_8));
        } catch (IOException | JSONException e) {
            return new JSONObject();
        }
    }

    private static String current(JSONObject config, Setting setting) {
        String[] parts = setting.key.split("\\.");
        JSONObject section = config.optJSONObject(parts[0]);
        Object value = section == null ? null : section.opt(parts[1]);
        if (value == null) {
            return setting.fallback;
        }
        return value instanceof String ? JSONObject.quote((String) value) : String.valueOf(value);
    }

    private static int chosen(Setting setting, String value) {
        for (int i = 0; i < setting.values.length; i++) {
            if (setting.values[i].equals(value)) {
                return i;
            }
            try {
                if (Math.abs(Double.parseDouble(setting.values[i]) - Double.parseDouble(value)) < 1e-6) {
                    return i;
                }
            } catch (NumberFormatException ignored) {
            }
        }
        return -1;
    }

    /** What the settings show for a value, which may be one written since the last read of config.json. */
    private final java.util.Map<String, String> pending = new java.util.HashMap<>();

    private void showSettings() {
        JSONObject config = config();
        String[] rows = new String[SETTINGS.length + 1];
        for (int i = 0; i < SETTINGS.length; i++) {
            Setting s = SETTINGS[i];
            String value = pending.containsKey(s.key) ? pending.get(s.key) : current(config, s);
            int at = chosen(s, value);
            rows[i] = getString(s.label) + ": " + (at >= 0 ? s.names[at] : value);
        }
        rows[SETTINGS.length] = getString(R.string.setting_controls_opacity) + ": " +
            Math.round(overlay.globalOpacity() * 100) + "%";
        dialog()
            .setTitle(R.string.menu_settings)
            .setItems(rows, (d, which) -> {
                if (which == SETTINGS.length) {
                    chooseOpacity();
                } else {
                    choose(SETTINGS[which], config);
                }
            })
            .setNegativeButton(R.string.menu_back, (d, w) -> onMenu())
            .setOnCancelListener(d -> onMenu())
            .show();
    }

    private void choose(Setting setting, JSONObject config) {
        String value = pending.containsKey(setting.key) ? pending.get(setting.key) : current(config, setting);
        dialog()
            .setTitle(setting.label)
            .setSingleChoiceItems(setting.names, chosen(setting, value), (d, which) -> {
                Native.setSetting(setting.key, setting.values[which]);
                pending.put(setting.key, setting.values[which]);
                d.dismiss();
                showSettings();
            })
            .setOnCancelListener(d -> showSettings())
            .show();
    }

    private void chooseOpacity() {
        String[] names = percent(10, 100, 10);
        int at = Math.round(overlay.globalOpacity() * 10) - 1;
        dialog()
            .setTitle(R.string.setting_controls_opacity)
            .setSingleChoiceItems(names, Math.max(0, Math.min(9, at)), (d, which) -> {
                overlay.setGlobalOpacity((which + 1) / 10f);
                d.dismiss();
                showSettings();
            })
            .setOnCancelListener(d -> showSettings())
            .show();
    }

    // ---- The controls' editor -----------------------------------------------------------------

    /**
     * A slim strip along the top edge, one row: what is picked, its size, the opacity of all the
     * controls, Reset all and Done. It gets out of the way of the controls being placed: hidden while
     * one is dragged, and at the bottom edge instead while the one placed is in the top part.
     */
    private void buildEditPanel() {
        editPanel = new LinearLayout(this);
        editPanel.setOrientation(LinearLayout.HORIZONTAL);
        editPanel.setGravity(Gravity.CENTER_VERTICAL);
        editPanel.setBackgroundColor(Color.argb(0xC0, 0x10, 0x12, 0x1A));
        editPanel.setPadding(dp(12), dp(2), dp(8), dp(2));
        editLabel = new TextView(this);
        editLabel.setTextColor(Color.WHITE);
        editLabel.setTextSize(12);
        editLabel.setMaxLines(2);
        editPanel.addView(editLabel, new LinearLayout.LayoutParams(dp(120), ViewGroup.LayoutParams.WRAP_CONTENT));

        editSize = slider(R.string.edit_size, 25, 250, (value) -> overlay.setSelectedScale(value / 100f));
        // One opacity for every control, the same as the settings' Touch controls opacity.
        editOpacity = slider(R.string.edit_opacity, 5, 100, (value) -> overlay.setGlobalOpacity(value / 100f));

        Button reset = new Button(this, null, android.R.attr.borderlessButtonStyle);
        reset.setText(R.string.edit_reset);
        reset.setTextColor(Color.WHITE);
        reset.setOnClickListener(v -> overlay.resetLayout());
        editPanel.addView(reset);
        Button done = new Button(this, null, android.R.attr.borderlessButtonStyle);
        done.setText(R.string.edit_done);
        done.setTextColor(Color.WHITE);
        done.setOnClickListener(v -> finishEditing());
        editPanel.addView(done);

        editPanel.setVisibility(android.view.View.GONE);
        addContentView(editPanel, editPanelAt(Gravity.TOP));
    }

    private FrameLayout.LayoutParams editPanelAt(int edge) {
        FrameLayout.LayoutParams params = new FrameLayout.LayoutParams(Math.min(dp(640),
            getResources().getDisplayMetrics().widthPixels - dp(32)), ViewGroup.LayoutParams.WRAP_CONTENT,
            edge | Gravity.CENTER_HORIZONTAL);
        return params;
    }

    @Override
    public void onEditDrag(boolean dragging, float y) {
        if (dragging) {
            editPanel.setVisibility(android.view.View.INVISIBLE);
            return;
        }
        // Clear of the control just placed: at the bottom when it sits in the top part of the screen.
        int edge = y < overlay.getHeight() * 0.4f ? Gravity.BOTTOM : Gravity.TOP;
        editPanel.setLayoutParams(editPanelAt(edge));
        editPanel.setVisibility(android.view.View.VISIBLE);
    }

    private interface IntSink {
        void accept(int value);
    }

    /** A label over a slider, side by side with the panel's other parts. */
    private SeekBar slider(int label, int min, int max, IntSink sink) {
        LinearLayout column = new LinearLayout(this);
        column.setOrientation(LinearLayout.VERTICAL);
        TextView name = new TextView(this);
        name.setText(label);
        name.setTextColor(Color.LTGRAY);
        name.setTextSize(11);
        column.addView(name);
        SeekBar bar = new SeekBar(this);
        bar.setMin(min);
        bar.setMax(max);
        bar.setOnSeekBarChangeListener(new SeekBar.OnSeekBarChangeListener() {
            @Override
            public void onProgressChanged(SeekBar seekBar, int progress, boolean fromUser) {
                if (fromUser) {
                    sink.accept(progress);
                }
            }

            @Override
            public void onStartTrackingTouch(SeekBar seekBar) {}

            @Override
            public void onStopTrackingTouch(SeekBar seekBar) {}
        });
        column.addView(bar);
        editPanel.addView(column, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));
        return bar;
    }

    private void startEditing() {
        menuOpen = true;
        Native.setPaused(true);
        overlay.setEditing(true);
        onSelectionChanged(null);
        editPanel.setLayoutParams(editPanelAt(Gravity.TOP));
        editPanel.setVisibility(android.view.View.VISIBLE);
    }

    private void finishEditing() {
        overlay.setEditing(false);
        editPanel.setVisibility(android.view.View.GONE);
        resume();
    }

    @Override
    public void onSelectionChanged(String id) {
        boolean any = id != null;
        editLabel.setText(any ? getString(R.string.edit_selected, id.toUpperCase()) : getString(R.string.edit_hint));
        editSize.setEnabled(any);
        if (any) {
            editSize.setProgress(Math.round(overlay.selectedScale() * 100));
        }
        editOpacity.setProgress(Math.round(overlay.globalOpacity() * 100));
    }

    // ---- PS2 memory cards ---------------------------------------------------------------------

    private void pickCard() {
        Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        intent.setType("*/*");
        startActivityForResult(intent, PICK_CARD);
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        if (requestCode != PICK_CARD) {
            super.onActivityResult(requestCode, resultCode, data);
            return;
        }
        if (resultCode != RESULT_OK || data == null || data.getData() == null) {
            onMenu();
            return;
        }
        Uri uri = data.getData();
        new Thread(() -> {
            String json;
            try (ParcelFileDescriptor pfd = getContentResolver().openFileDescriptor(uri, "r")) {
                json = pfd == null ? "{\"error\":\"the file could not be opened\"}"
                                   : Native.importCard(pfd.detachFd(), new File(getFilesDir(), "save/saves").getPath());
            } catch (IOException | SecurityException e) {
                json = JSONObject.quote(String.valueOf(e.getMessage()));
                json = "{\"error\":" + json + "}";
            }
            String result = json;
            runOnUiThread(() -> showImport(result));
        }, "card-import").start();
    }

    private void showImport(String json) {
        StringBuilder message = new StringBuilder();
        try {
            JSONObject result = new JSONObject(json);
            if (result.has("error")) {
                message.append(result.getString("error"));
            } else {
                JSONArray found = result.getJSONArray("found");
                for (int i = 0; i < found.length(); i++) {
                    JSONObject f = found.getJSONObject(i);
                    message.append(f.getString("source")).append(": ");
                    if (f.has("folder")) {
                        message.append(getString(R.string.import_slot, f.getInt("folder")));
                        String from = f.optString("converted_from", "");
                        if (!from.isEmpty() && !from.equals("null")) {
                            message.append(' ').append(getString(R.string.import_converted, from));
                        }
                    } else {
                        message.append(f.optString("skipped"));
                    }
                    message.append("\n\n");
                }
                message.append(getString(R.string.import_where));
            }
        } catch (JSONException e) {
            message.append(json);
        }
        dialog()
            .setTitle(R.string.menu_import_card)
            .setMessage(message.toString().trim())
            .setPositiveButton(android.R.string.ok, (d, w) -> onMenu())
            .setOnCancelListener(d -> onMenu())
            .show();
    }
}
