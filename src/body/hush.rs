//! The Felt app's quiet switch, as on Windows (WP-42.5): she's cut off, says "okay okay, I'll zip it" (one of
//! her recorded presence lines, when `[body] sounds` points at them), and goes off screen until it's switched
//! off; nothing she'd say on her own plays meanwhile. Being hushed annoys her a little (mood).

use std::rc::Rc;

use serde_json::json;

use super::ctx::Ctx;

pub async fn set(ctx: &Rc<Ctx>, quiet: bool) {
    if quiet == ctx.quiet.replace(quiet) {
        return;
    }
    if !quiet {
        tracing::info!("hush: lifted");
        return;
    }
    tracing::info!("hush: on");
    ctx.overlay.lipsync(&[]).await; // her mouth eases closed with the cut-off sentence
    ctx.mood.borrow_mut().on_hush();
    ctx.events.report("hush", json!({}));
    if let Some(wav) = presence_line(ctx, "zip_it") {
        ctx.player.play(wav);
    }
}

/// A random take of one of her recorded presence lines (`<sounds>/presence/<line>_N.wav`).
fn presence_line(ctx: &Ctx, line: &str) -> Option<Vec<u8>> {
    let dir = ctx.sounds.as_deref()?;
    let path = felt_sounds::pick_presence_path(line, dir, None)?;
    std::fs::read(&path)
        .inspect_err(|e| tracing::warn!("can't read {}: {e}", path.display()))
        .ok()
}
