// felt-tray (her body): where the cursor is, for her gaze - on Wayland an app never sees the cursor outside
// its own windows, and hers are click-through. Sent at most every 66 ms while it moves, plus where it stops.
const EVERY_MS = 66;
let last = 0;
const trailing = new QTimer();
trailing.singleShot = true;
trailing.interval = EVERY_MS;

function send() {
    last = Date.now();
    const p = workspace.cursorPos;
    // one JSON string: KWin's JS numbers arrive as int or double depending on the value, a string never varies
    callDBus("org.clockmost.Felt", "/org/clockmost/Felt", "org.clockmost.Felt", "Cursor", JSON.stringify([p.x, p.y]));
}

trailing.timeout.connect(send);
workspace.cursorPosChanged.connect(() => {
    if (Date.now() - last >= EVERY_MS) {
        send();
    } else if (!trailing.active) {
        trailing.start();
    }
});
send();
