//! Her recorded lines (`[body] sounds`): the presence takes ("zip it", "back!", "unhush") and the "hmm" filler a
//! slow reply gets.

use std::rc::Rc;
use std::time::Duration;

use super::ctx::Ctx;

/// A slow reply gets a recorded "hmm" after 1.5 s (cancelled the moment the reply arrives).
pub fn arm_filler(ctx: &Rc<Ctx>) {
    let Some(dir) = ctx.sounds.clone() else {
        return;
    };
    let c = ctx.clone();
    let task = tokio::task::spawn_local(async move {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        if let Some(path) = felt_sounds::pick_filler_path(&dir, None)
            && let Ok(wav) = std::fs::read(&path)
        {
            c.player.play(wav);
        }
    });
    if let Some(old) = ctx.filler.replace(Some(task)) {
        old.abort();
    }
}

/// A random take of one of her recorded presence lines (`<sounds>/presence/<line>_N.wav`).
pub fn presence_line(ctx: &Ctx, line: &str) -> Option<Vec<u8>> {
    let dir = ctx.sounds.as_deref()?;
    let path = felt_sounds::pick_presence_path(line, dir, None)?;
    std::fs::read(&path)
        .inspect_err(|e| tracing::warn!("can't read {}: {e}", path.display()))
        .ok()
}
