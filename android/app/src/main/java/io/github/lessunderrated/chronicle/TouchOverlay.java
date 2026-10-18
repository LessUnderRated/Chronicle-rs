package io.github.lessunderrated.chronicle;

import android.annotation.SuppressLint;
import android.content.Context;
import android.content.SharedPreferences;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Insets;
import android.graphics.Paint;
import android.graphics.Path;
import android.graphics.RectF;
import android.view.MotionEvent;
import android.view.View;
import android.view.WindowInsets;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.HashMap;
import java.util.Map;

/**
 * The touch controls, drawn over the game. What they press is worked out natively (android/src/touch.rs,
 * the virtual gamepad, which also gives the default layout clear of the HUD); this draws them, hands
 * every touch on them over, and runs the two buttons of its own: the speed toggle (top left) and the
 * menu (top right). In edit mode a touch picks a control up and moves it instead, and the activity's
 * panel sets the size and opacity of the one picked.
 */
@SuppressLint("ViewConstructor")
public class TouchOverlay extends View {
    interface Listener {
        void onMenu();

        void onSpeedChanged(int speed);

        void onSelectionChanged(String id);
    }

    private static final String PREFS = "touch";
    private static final String KEY_LAYOUT = "layout";
    private static final String KEY_HOST = "host";
    private static final String KEY_OPACITY = "opacity";
    private static final String KEY_HIDDEN = "hidden";

    private static final int BIT_L2 = 1 << 30;
    private static final int BIT_R2 = 1 << 31;
    private static final Map<String, Integer> BITS = new HashMap<>();
    private static final Map<String, Integer> COLORS = new HashMap<>();
    private static final Map<String, String> LABELS = new HashMap<>();

    static {
        BITS.put("cross", 1);
        BITS.put("circle", 1 << 1);
        BITS.put("square", 1 << 2);
        BITS.put("triangle", 1 << 3);
        BITS.put("select", 1 << 4);
        BITS.put("start", 1 << 6);
        BITS.put("l1", 1 << 9);
        BITS.put("r1", 1 << 10);
        BITS.put("l2", BIT_L2);
        BITS.put("r2", BIT_R2);
        COLORS.put("cross", Color.rgb(0x7C, 0xA8, 0xFF));
        COLORS.put("circle", Color.rgb(0xFF, 0x6E, 0x6E));
        COLORS.put("square", Color.rgb(0xF4, 0x8C, 0xD8));
        COLORS.put("triangle", Color.rgb(0x5C, 0xE6, 0xB8));
        LABELS.put("l1", "L1");
        LABELS.put("l2", "L2");
        LABELS.put("r1", "R1");
        LABELS.put("r2", "R2");
        LABELS.put("select", "SELECT");
        LABELS.put("start", "START");
        LABELS.put("speed", "2×");
        LABELS.put("menu", "☰");
    }

    private final Listener listener;
    private final SharedPreferences prefs;
    private final Paint fill = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint edge = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint mark = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint text = new Paint(Paint.ANTI_ALIAS_FLAG);

    /** The pad's controls as the native side laid them out, and the default for this window. */
    private JSONObject layout;
    private JSONObject defaults;
    /** The speed and menu buttons: {"speed": {x, y, r, opacity}, "menu": {...}}. */
    private JSONObject host;
    private JSONObject padState = new JSONObject();
    private float globalOpacity;
    private boolean hidden;

    private boolean editing;
    private String selected;
    private final Map<Integer, String> held = new HashMap<>();
    private float dragX;
    private float dragY;

    private float insetLeft;
    private float insetTop;
    private float insetRight;
    private float insetBottom;

    TouchOverlay(Context context, Listener listener) {
        super(context);
        this.listener = listener;
        prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
        globalOpacity = prefs.getFloat(KEY_OPACITY, 0.7f);
        hidden = prefs.getBoolean(KEY_HIDDEN, false);
        edge.setStyle(Paint.Style.STROKE);
        mark.setStyle(Paint.Style.STROKE);
        mark.setStrokeCap(Paint.Cap.ROUND);
        text.setTextAlign(Paint.Align.CENTER);
        text.setFakeBoldText(true);
        setWillNotDraw(false);
    }

    // A controller connecting takes the touch pad away without a touch; look every half second.
    private final Runnable poll = new Runnable() {
        @Override
        public void run() {
            if (!editing) {
                refreshState();
            }
            postDelayed(this, 500);
        }
    };

    @Override
    protected void onAttachedToWindow() {
        super.onAttachedToWindow();
        post(poll);
    }

    @Override
    protected void onDetachedFromWindow() {
        removeCallbacks(poll);
        super.onDetachedFromWindow();
    }

    // ---- Layout -------------------------------------------------------------------------------

    @Override
    public WindowInsets onApplyWindowInsets(WindowInsets insets) {
        // The same edges SDL counts out of the game's safe area, where the HUD stops.
        Insets safe = insets.getInsets(WindowInsets.Type.systemBars() | WindowInsets.Type.systemGestures() |
            WindowInsets.Type.mandatorySystemGestures() | WindowInsets.Type.tappableElement() |
            WindowInsets.Type.displayCutout());
        insetLeft = safe.left;
        insetTop = safe.top;
        insetRight = safe.right;
        insetBottom = safe.bottom;
        relayout();
        return super.onApplyWindowInsets(insets);
    }

    @Override
    protected void onSizeChanged(int w, int h, int oldw, int oldh) {
        super.onSizeChanged(w, h, oldw, oldh);
        relayout();
    }

    private void relayout() {
        int w = getWidth();
        int h = getHeight();
        if (w <= 0 || h <= 0) {
            return;
        }
        try {
            defaults = new JSONObject(Native.defaultLayout(w, h, insetLeft, insetTop, insetRight, insetBottom));
            layout = new JSONObject(Native.setLayout(w, h, insetLeft, insetTop, insetRight, insetBottom,
                prefs.getString(KEY_LAYOUT, null)));
            host = loadHost(w, h);
        } catch (JSONException | UnsatisfiedLinkError e) {
            layout = null;
        }
        float u = Math.min(w, h);
        edge.setStrokeWidth(u * 0.006f);
        mark.setStrokeWidth(u * 0.011f);
        invalidate();
    }

    /** The speed and menu buttons, saved as fractions of the window so they keep their corner. */
    private JSONObject loadHost(int w, int h) throws JSONException {
        float u = Math.min(w, h);
        float r = u * 0.04f;
        float margin = u * 0.02f;
        JSONObject made = new JSONObject();
        made.put("speed", button(insetLeft + margin + r, insetTop + margin + r, r));
        made.put("menu", button(w - insetRight - margin - r, insetTop + margin + r, r));
        String saved = prefs.getString(KEY_HOST, null);
        if (saved != null) {
            JSONObject mine = new JSONObject(saved);
            for (String id : new String[] {"speed", "menu"}) {
                JSONObject b = mine.optJSONObject(id);
                if (b != null) {
                    made.put(id, button(b.getDouble("fx") * w, b.getDouble("fy") * h, b.getDouble("fr") * u)
                        .put("opacity", b.optDouble("opacity", 1.0)));
                }
            }
        }
        return made;
    }

    private static JSONObject button(double x, double y, double r) throws JSONException {
        return new JSONObject().put("x", x).put("y", y).put("r", r).put("opacity", 1.0);
    }

    private void save() {
        if (layout == null) {
            return;
        }
        try {
            int w = getWidth();
            int h = getHeight();
            float u = Math.min(w, h);
            JSONObject mine = new JSONObject();
            for (String id : new String[] {"speed", "menu"}) {
                JSONObject b = host.getJSONObject(id);
                mine.put(id, new JSONObject().put("fx", b.getDouble("x") / w).put("fy", b.getDouble("y") / h)
                    .put("fr", b.getDouble("r") / u).put("opacity", b.getDouble("opacity")));
            }
            prefs.edit().putString(KEY_LAYOUT, layout.toString()).putString(KEY_HOST, mine.toString()).apply();
            layout = new JSONObject(Native.setLayout(w, h, insetLeft, insetTop, insetRight, insetBottom, layout.toString()));
        } catch (JSONException ignored) {
        }
    }

    void resetLayout() {
        prefs.edit().remove(KEY_LAYOUT).remove(KEY_HOST).apply();
        selected = null;
        listener.onSelectionChanged(null);
        relayout();
    }

    float globalOpacity() {
        return globalOpacity;
    }

    void setGlobalOpacity(float opacity) {
        globalOpacity = Math.max(0.05f, Math.min(1f, opacity));
        prefs.edit().putFloat(KEY_OPACITY, globalOpacity).apply();
        invalidate();
    }

    boolean hidden() {
        return hidden;
    }

    void setHidden(boolean hide) {
        hidden = hide;
        prefs.edit().putBoolean(KEY_HIDDEN, hide).apply();
        invalidate();
    }

    void setEditing(boolean edit) {
        editing = edit;
        if (!edit) {
            selected = null;
            save();
        }
        held.clear();
        invalidate();
    }

    // ---- The picked control's size and opacity (edit mode) -------------------------------------

    private JSONObject element(String id) {
        if (id == null) {
            return null;
        }
        if (host != null && host.has(id)) {
            return host.optJSONObject(id);
        }
        return find(layout, id);
    }

    private static JSONObject find(JSONObject from, String id) {
        if (from == null) {
            return null;
        }
        JSONArray elements = from.optJSONArray("elements");
        for (int i = 0; elements != null && i < elements.length(); i++) {
            JSONObject e = elements.optJSONObject(i);
            if (e != null && id.equals(e.optString("id"))) {
                return e;
            }
        }
        return null;
    }

    /** The picked control's size against the default's: 1 is the default. */
    float selectedScale() {
        JSONObject e = element(selected);
        JSONObject d = defaultOf(selected);
        if (e == null || d == null) {
            return 1f;
        }
        return "rect".equals(e.optString("shape")) ? (float) (e.optDouble("w") / d.optDouble("w"))
                                                   : (float) (e.optDouble("r") / d.optDouble("r"));
    }

    private JSONObject defaultOf(String id) {
        if (id == null) {
            return null;
        }
        if (id.equals("speed") || id.equals("menu")) {
            float u = Math.min(getWidth(), getHeight());
            try {
                return button(0, 0, u * 0.04f);
            } catch (JSONException e) {
                return null;
            }
        }
        return find(defaults, id);
    }

    void setSelectedScale(float scale) {
        JSONObject e = element(selected);
        JSONObject d = defaultOf(selected);
        if (e == null || d == null) {
            return;
        }
        try {
            if ("rect".equals(e.optString("shape"))) {
                double cx = e.getDouble("x") + e.getDouble("w") / 2;
                double cy = e.getDouble("y") + e.getDouble("h") / 2;
                double w = d.getDouble("w") * scale;
                double h = d.getDouble("h") * scale;
                e.put("w", w).put("h", h).put("x", cx - w / 2).put("y", cy - h / 2);
            } else {
                e.put("r", d.getDouble("r") * scale);
            }
        } catch (JSONException ignored) {
        }
        invalidate();
    }

    float selectedOpacity() {
        JSONObject e = element(selected);
        return e == null ? 1f : (float) e.optDouble("opacity", 1.0);
    }

    void setSelectedOpacity(float opacity) {
        JSONObject e = element(selected);
        if (e != null) {
            try {
                e.put("opacity", Math.max(0.05, Math.min(1.0, opacity)));
            } catch (JSONException ignored) {
            }
            invalidate();
        }
    }

    String selected() {
        return selected;
    }

    // ---- Touch --------------------------------------------------------------------------------

    private static boolean inside(JSONObject e, float x, float y, float slack) {
        if ("rect".equals(e.optString("shape"))) {
            double w = e.optDouble("w");
            double h = e.optDouble("h");
            double sx = w * (slack - 1) / 2;
            double sy = h * (slack - 1) / 2;
            return x >= e.optDouble("x") - sx && x <= e.optDouble("x") + w + sx && y >= e.optDouble("y") - sy &&
                y <= e.optDouble("y") + h + sy;
        }
        return Math.hypot(x - e.optDouble("x"), y - e.optDouble("y")) <= e.optDouble("r") * slack;
    }

    /** The control under a point: the speed and menu buttons, then the pad's, as the native side hits them. */
    private String hit(float x, float y, boolean pad) {
        for (String id : new String[] {"speed", "menu"}) {
            JSONObject b = host == null ? null : host.optJSONObject(id);
            if (b != null && inside(b, x, y, 1.3f)) {
                return id;
            }
        }
        if (!pad || layout == null) {
            return null;
        }
        JSONArray elements = layout.optJSONArray("elements");
        for (float slack : new float[] {1f, 1.3f}) {
            for (int i = 0; elements != null && i < elements.length(); i++) {
                JSONObject e = elements.optJSONObject(i);
                if (e != null && inside(e, x, y, slack)) {
                    return e.optString("id");
                }
            }
        }
        return null;
    }

    private boolean padActive() {
        return !hidden && padState.optBoolean("active", true);
    }

    @SuppressLint("ClickableViewAccessibility")
    @Override
    public boolean onTouchEvent(MotionEvent event) {
        if (layout == null) {
            return false;
        }
        int action = event.getActionMasked();
        int index = event.getActionIndex();
        int pointer = event.getPointerId(index);
        float x = event.getX(index);
        float y = event.getY(index);
        if (editing) {
            return editTouch(event, action, pointer, x, y);
        }
        switch (action) {
            case MotionEvent.ACTION_DOWN:
            case MotionEvent.ACTION_POINTER_DOWN: {
                String id = hit(x, y, false);
                if (id != null) {
                    held.put(pointer, id);
                } else if (padActive()) {
                    Native.touch(0, pointer, x, y);
                } else if (action == MotionEvent.ACTION_DOWN) {
                    return false;
                }
                break;
            }
            case MotionEvent.ACTION_MOVE:
                for (int i = 0; i < event.getPointerCount(); i++) {
                    int id = event.getPointerId(i);
                    if (!held.containsKey(id) && padActive()) {
                        Native.touch(1, id, event.getX(i), event.getY(i));
                    }
                }
                break;
            case MotionEvent.ACTION_UP:
            case MotionEvent.ACTION_POINTER_UP:
            case MotionEvent.ACTION_CANCEL: {
                String id = held.remove(pointer);
                if (id != null) {
                    JSONObject b = host.optJSONObject(id);
                    if (action != MotionEvent.ACTION_CANCEL && b != null && inside(b, x, y, 1.6f)) {
                        if (id.equals("menu")) {
                            listener.onMenu();
                        } else {
                            int speed = Native.speed() == 2 ? 1 : 2;
                            Native.setSpeed(speed);
                            listener.onSpeedChanged(speed);
                        }
                    }
                } else {
                    Native.touch(2, pointer, x, y);
                }
                if (action == MotionEvent.ACTION_CANCEL) {
                    for (int i = 0; i < event.getPointerCount(); i++) {
                        Native.touch(2, event.getPointerId(i), 0, 0);
                    }
                    held.clear();
                }
                break;
            }
            default:
                break;
        }
        refreshState();
        return true;
    }

    private boolean editTouch(MotionEvent event, int action, int pointer, float x, float y) {
        switch (action) {
            case MotionEvent.ACTION_DOWN: {
                selected = hit(x, y, true);
                dragX = x;
                dragY = y;
                listener.onSelectionChanged(selected);
                break;
            }
            case MotionEvent.ACTION_MOVE: {
                JSONObject e = element(selected);
                if (e != null && event.getPointerId(0) == pointer) {
                    float nx = event.getX(0);
                    float ny = event.getY(0);
                    try {
                        e.put("x", e.getDouble("x") + nx - dragX).put("y", e.getDouble("y") + ny - dragY);
                    } catch (JSONException ignored) {
                    }
                    dragX = nx;
                    dragY = ny;
                }
                break;
            }
            default:
                break;
        }
        invalidate();
        return true;
    }

    void refreshState() {
        try {
            padState = new JSONObject(Native.padState());
        } catch (JSONException | UnsatisfiedLinkError e) {
            padState = new JSONObject();
        }
        invalidate();
    }

    // ---- Drawing ------------------------------------------------------------------------------

    private int alpha(double opacity, float part) {
        return Math.round(255 * part * (float) opacity * globalOpacity);
    }

    @Override
    protected void onDraw(Canvas canvas) {
        if (layout == null) {
            return;
        }
        boolean showPad = editing || padActive();
        if (showPad) {
            drawPad(canvas);
        }
        drawHost(canvas);
    }

    private void drawPad(Canvas canvas) {
        int buttons = padState.optInt("buttons", 0);
        JSONArray elements = layout.optJSONArray("elements");
        for (int i = 0; elements != null && i < elements.length(); i++) {
            JSONObject e = elements.optJSONObject(i);
            if (e == null) {
                continue;
            }
            String id = e.optString("id");
            double opacity = e.optDouble("opacity", 1.0);
            Integer bit = BITS.get(id);
            boolean down = bit != null && (buttons & bit) != 0;
            if ("rect".equals(e.optString("shape"))) {
                RectF r = new RectF((float) e.optDouble("x"), (float) e.optDouble("y"),
                    (float) (e.optDouble("x") + e.optDouble("w")), (float) (e.optDouble("y") + e.optDouble("h")));
                float corner = r.height() * 0.3f;
                fill.setColor(Color.WHITE);
                fill.setAlpha(alpha(opacity, down ? 0.45f : 0.16f));
                canvas.drawRoundRect(r, corner, corner, fill);
                edge.setColor(Color.WHITE);
                edge.setAlpha(alpha(opacity, 0.5f));
                canvas.drawRoundRect(r, corner, corner, edge);
                text.setColor(Color.WHITE);
                text.setAlpha(alpha(opacity, 0.85f));
                text.setTextSize(r.height() * 0.42f);
                canvas.drawText(LABELS.getOrDefault(id, id), r.centerX(), r.centerY() + r.height() * 0.15f, text);
            } else {
                float x = (float) e.optDouble("x");
                float y = (float) e.optDouble("y");
                float radius = (float) e.optDouble("r");
                fill.setColor(Color.WHITE);
                fill.setAlpha(alpha(opacity, down ? 0.45f : 0.14f));
                canvas.drawCircle(x, y, radius, fill);
                edge.setColor(Color.WHITE);
                edge.setAlpha(alpha(opacity, 0.5f));
                canvas.drawCircle(x, y, radius, edge);
                if (id.equals("dpad")) {
                    drawDpad(canvas, x, y, radius, buttons, opacity);
                } else {
                    drawSymbol(canvas, id, x, y, radius, opacity);
                }
            }
            if (editing && id.equals(selected)) {
                drawSelection(canvas, e);
            }
        }
        float stickR = (float) layout.optDouble("stick_radius");
        JSONArray sticks = padState.optJSONArray("sticks");
        boolean leftHeld = false;
        for (int i = 0; sticks != null && i < sticks.length(); i++) {
            JSONObject s = sticks.optJSONObject(i);
            leftHeld |= s.optBoolean("left");
            drawStick(canvas, (float) s.optDouble("x"), (float) s.optDouble("y"), (float) s.optDouble("knob_x"),
                (float) s.optDouble("knob_y"), stickR, 1f);
        }
        JSONArray rest = layout.optJSONArray("left_rest");
        if (!leftHeld && rest != null) {
            float x = (float) rest.optDouble(0);
            float y = (float) rest.optDouble(1);
            drawStick(canvas, x, y, x, y, stickR, 0.5f);
        }
    }

    private void drawStick(Canvas canvas, float x, float y, float kx, float ky, float r, float strength) {
        edge.setColor(Color.WHITE);
        edge.setAlpha(alpha(1, 0.45f * strength));
        canvas.drawCircle(x, y, r, edge);
        fill.setColor(Color.WHITE);
        fill.setAlpha(alpha(1, 0.12f * strength));
        canvas.drawCircle(x, y, r, fill);
        fill.setAlpha(alpha(1, 0.35f * strength));
        canvas.drawCircle(kx, ky, r * 0.45f, fill);
    }

    private void drawSymbol(Canvas canvas, String id, float x, float y, float r, double opacity) {
        Integer color = COLORS.get(id);
        if (color == null) {
            return;
        }
        mark.setColor(color);
        mark.setAlpha(alpha(opacity, 0.95f));
        float s = r * 0.42f;
        switch (id) {
            case "cross":
                canvas.drawLine(x - s, y - s, x + s, y + s, mark);
                canvas.drawLine(x - s, y + s, x + s, y - s, mark);
                break;
            case "circle":
                canvas.drawCircle(x, y, s * 1.1f, mark);
                break;
            case "square":
                canvas.drawRect(x - s, y - s, x + s, y + s, mark);
                break;
            case "triangle": {
                Path path = new Path();
                path.moveTo(x, y - s * 1.15f);
                path.lineTo(x + s * 1.1f, y + s * 0.75f);
                path.lineTo(x - s * 1.1f, y + s * 0.75f);
                path.close();
                canvas.drawPath(path, mark);
                break;
            }
            default:
                break;
        }
    }

    private void drawDpad(Canvas canvas, float x, float y, float r, int buttons, double opacity) {
        float arm = r * 0.62f;
        float half = r * 0.2f;
        int[][] arms = {{11, 0, -1}, {12, 0, 1}, {13, -1, 0}, {14, 1, 0}};
        for (int[] a : arms) {
            boolean down = (buttons & (1 << a[0])) != 0;
            fill.setColor(Color.WHITE);
            fill.setAlpha(alpha(opacity, down ? 0.8f : 0.4f));
            float dx = a[1];
            float dy = a[2];
            float left = x + Math.min(dx * half, dx * arm) - (dx == 0 ? half : 0);
            float right = x + Math.max(dx * half, dx * arm) + (dx == 0 ? half : 0);
            float top = y + Math.min(dy * half, dy * arm) - (dy == 0 ? half : 0);
            float bottom = y + Math.max(dy * half, dy * arm) + (dy == 0 ? half : 0);
            canvas.drawRoundRect(new RectF(left, top, right, bottom), half * 0.4f, half * 0.4f, fill);
        }
    }

    private void drawHost(Canvas canvas) {
        if (host == null) {
            return;
        }
        boolean fast = Native.speed() == 2;
        for (String id : new String[] {"speed", "menu"}) {
            JSONObject b = host.optJSONObject(id);
            if (b == null) {
                continue;
            }
            float x = (float) b.optDouble("x");
            float y = (float) b.optDouble("y");
            float r = (float) b.optDouble("r");
            double opacity = b.optDouble("opacity", 1.0);
            boolean on = id.equals("speed") && fast;
            fill.setColor(on ? Color.rgb(0xFF, 0xC8, 0x40) : Color.BLACK);
            fill.setAlpha(alpha(opacity, on ? 0.7f : 0.35f));
            canvas.drawCircle(x, y, r, fill);
            edge.setColor(Color.WHITE);
            edge.setAlpha(alpha(opacity, 0.6f));
            canvas.drawCircle(x, y, r, edge);
            text.setColor(Color.WHITE);
            text.setAlpha(alpha(opacity, 0.95f));
            text.setTextSize(r * (id.equals("menu") ? 1.1f : 0.85f));
            canvas.drawText(LABELS.get(id), x, y + r * 0.32f, text);
            if (editing && id.equals(selected)) {
                drawSelection(canvas, b);
            }
        }
    }

    private void drawSelection(Canvas canvas, JSONObject e) {
        Paint p = new Paint(Paint.ANTI_ALIAS_FLAG);
        p.setStyle(Paint.Style.STROKE);
        p.setStrokeWidth(edge.getStrokeWidth() * 1.5f);
        p.setColor(Color.rgb(0xFF, 0xC8, 0x40));
        if ("rect".equals(e.optString("shape"))) {
            canvas.drawRect((float) e.optDouble("x"), (float) e.optDouble("y"),
                (float) (e.optDouble("x") + e.optDouble("w")), (float) (e.optDouble("y") + e.optDouble("h")), p);
        } else {
            canvas.drawCircle((float) e.optDouble("x"), (float) e.optDouble("y"), (float) e.optDouble("r") * 1.08f, p);
        }
    }
}
