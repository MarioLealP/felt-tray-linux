// felt-tray's KWin script (loaded by felt-tray when screen reactions are on): tells felt-tray which windows
// are visible and which is active, on every change that matters for the capture policy - including title
// changes, so a tab switching to a bank page is seen before the next capture. felt-tray only accepts these
// calls from KWin itself.

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
        w: Math.round(w.frameGeometry.width),
        h: Math.round(w.frameGeometry.height),
        fullscreen: w.fullScreen,
    };
}

function send() {
    const a = workspace.activeWindow;
    const data = {
        active: a ? a.internalId.toString() : "",
        windows: workspace.windowList().filter(visible).map(info),
    };
    callDBus("org.clockmost.Felt", "/org/clockmost/Felt", "org.clockmost.Felt", "Windows", JSON.stringify(data));
}

function watch(w) {
    w.captionChanged.connect(send);
    w.minimizedChanged.connect(send);
    w.fullScreenChanged.connect(send);
    w.desktopsChanged.connect(send);
}

workspace.windowList().forEach(watch);
workspace.windowAdded.connect(w => { watch(w); send(); });
workspace.windowRemoved.connect(send);
workspace.windowActivated.connect(send);
workspace.currentDesktopChanged.connect(send);
send();
