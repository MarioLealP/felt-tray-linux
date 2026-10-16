// felt-tray's KWin script (loaded whenever felt-tray runs on KWin): keeps her overlay windows above, and tells
// felt-tray which windows are visible (top-most first, with where they are) and which is active, plus each
// monitor's work area - on every change that matters for the capture policy (including title changes, so a
// tab switching to a bank page is seen before the next capture) and for her body (windows moving, restacking:
// their tops are where she can stand). At most every 100 ms. felt-tray only accepts these calls from KWin.

function onCurrentDesktop(w) {
    return w.onAllDesktops || w.desktops.some(d => d.id === workspace.currentDesktop.id);
}

function visible(w) {
    return w.normalWindow && !w.skipTaskbar && !w.minimized && onCurrentDesktop(w);
}

function info(w) {
    return {
        id: w.internalId.toString(),
        app: w.resourceClass || "",
        desktop_file: w.desktopFileName || "",
        title: w.caption || "",
        pid: w.pid || 0,
        x: Math.round(w.frameGeometry.x),
        y: Math.round(w.frameGeometry.y),
        w: Math.round(w.frameGeometry.width),
        h: Math.round(w.frameGeometry.height),
        fullscreen: w.fullScreen,
    };
}

function screen(s) {
    const g = s.geometry;
    const work = workspace.clientArea(KWin.MaximizeArea, s, workspace.currentDesktop);
    return {
        x: Math.round(g.x), y: Math.round(g.y), w: Math.round(g.width), h: Math.round(g.height),
        work_bottom: Math.round(work.y + work.height),
    };
}

function sendNow() {
    const a = workspace.activeWindow;
    const data = {
        active: a ? a.internalId.toString() : "",
        // stackingOrder is bottom-first (a list, not a JS array); her world wants the top-most first
        windows: Array.from(workspace.stackingOrder).filter(visible).reverse().map(info),
        screens: Array.from(workspace.screens).map(screen),
    };
    callDBus("org.clockmost.Felt", "/org/clockmost/Felt", "org.clockmost.Felt", "Windows", JSON.stringify(data));
}

// Coalesce bursts (a window being dragged changes geometry every frame) into one send per 100 ms.
const pending = new QTimer();
pending.singleShot = true;
pending.interval = 100;
pending.timeout.connect(sendNow);

function send() {
    if (!pending.active) {
        pending.start();
    }
}

function watch(w) {
    w.captionChanged.connect(() => { overlay(w); send(); }); // Godot titles its main window after mapping
    w.minimizedChanged.connect(send);
    w.fullScreenChanged.connect(send);
    w.desktopsChanged.connect(send);
    w.frameGeometryChanged.connect(send);
}

// Her Godot overlay windows: Wayland ignores a client's own always-on-top, so KWin keeps them above, out of
// the taskbar / Alt+Tab / pager and on every desktop (click-through itself is the felt-wayland extension).
// Wayland apps can't place their own windows either: each overlay window is sized to one monitor (1 px shorter -
// a Windows-era guard), so it goes onto the monitor of that size, bottom-aligned.
function place(w) {
    const g = w.frameGeometry;
    for (const s of workspace.screens) {
        const o = s.geometry;
        if (Math.abs(o.width - g.width) <= 2 && Math.abs(o.height - g.height) <= 2) {
            const x = o.x, y = o.y + (o.height - g.height);
            if (g.x !== x || g.y !== y) {
                w.frameGeometry = { x: x, y: y, width: g.width, height: g.height };
            }
            return;
        }
    }
}

const overlays = new Set(); // internalIds already set up

function overlay(w) {
    const id = w.internalId.toString();
    if (!overlays.has(id) && w.caption && w.caption.indexOf("Felt Overlay") === 0) {
        overlays.add(id);
        w.keepAbove = true;
        w.skipTaskbar = true;
        w.skipSwitcher = true;
        w.skipPager = true;
        w.onAllDesktops = true;
        w.noBorder = true; // Wayland ignores Godot's borderless request; KWin would add a title bar
        place(w);
        w.frameGeometryChanged.connect(() => place(w)); // Godot sizes it after mapping; settles in one step
    }
}

workspace.windowList().forEach(w => { overlay(w); watch(w); });
workspace.windowAdded.connect(w => { overlay(w); watch(w); send(); });
workspace.windowRemoved.connect(w => { overlays.delete(w.internalId.toString()); send(); });
workspace.windowActivated.connect(send); // activating is what raises a window (KWin 6 has no restack signal)
workspace.currentDesktopChanged.connect(send);
workspace.screensChanged.connect(send);
send();
