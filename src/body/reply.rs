//! Her reaction to a reply: which gesture plays and which face she makes. The gesture is felt-core's tag when
//! this desktop has it (or its nearest ancestor), else - for a line you asked for - felt-llm's pick from her
//! tags, else her talking gesture. Speech never waits long for the pick: once her mouth cues are ready it gets
//! a short grace, then the pick is cancelled (felt-llm answers one request at a time; an abandoned pick would
//! hold up your next chat line).

use std::rc::Rc;
use std::time::Duration;

use super::ctx::Ctx;

const TAG_GRACE: Duration = Duration::from_secs(1);

/// A reaction being worked out while the lip sync runs.
pub struct Pending {
    tag: tokio::task::JoinHandle<String>,
    emotion: Option<String>,
    prompted: bool,
}

/// `prompted`: you asked for this reply (chat). Her own lines and screen reactions never ask felt-llm.
pub fn begin(
    ctx: &Rc<Ctx>,
    text: &str,
    tag: Option<String>,
    emotion: Option<String>,
    prompted: bool,
) -> Pending {
    let (c, text) = (ctx.clone(), text.to_owned());
    let pick = async move { pick_tag(&c, &text, tag.as_deref(), prompted).await };
    Pending {
        tag: tokio::task::spawn_local(pick),
        emotion,
        prompted,
    }
}

impl Pending {
    /// Right before her first sentence starts: settle the gesture (grace over -> fallback), finish thinking,
    /// put the face on for `speaking_s` (+1.5 s). Returns the clips to play once she starts talking.
    pub async fn settle(self, ctx: &Ctx, speaking_s: f64) -> Vec<String> {
        let mut pick = self.tag;
        let raw = match tokio::time::timeout(TAG_GRACE, &mut pick).await {
            Ok(Ok(t)) => t,
            Ok(Err(_)) => ctx.assets.fallback_tag().to_owned(),
            Err(_) => {
                pick.abort();
                tracing::info!(
                    "reaction pick not ready {TAG_GRACE:?} after lip sync - fallback gesture"
                );
                ctx.assets.fallback_tag().to_owned()
            }
        };
        ctx.behavior.end_thinking_and_wait().await;
        ctx.thinking.set(false);
        let tags = &ctx.assets.tags;
        let (tag, stand_up) = felt_library::resolve_posture_tag(
            &raw,
            &ctx.behavior.posture,
            tags,
            ctx.behavior.is_upper(&raw),
        );
        let mut clips = Vec::new();
        if stand_up && tags.contains("transition/sit_to_stand") {
            clips.push("transition/sit_to_stand".to_owned());
        }
        let pose = felt_faces::pose_for_reaction(self.emotion.as_deref(), &tag);
        ctx.pose.replace(pose.to_owned());
        ctx.apply_pose(pose, speaking_s + 1.5).await;
        tracing::info!("reaction: {tag} with a {pose} face");
        {
            let mut mood = ctx.mood.borrow_mut();
            mood.tick();
            mood.on_reaction_spoken();
            if self.prompted {
                mood.on_talked_to();
            }
        }
        clips.push(tag);
        clips
    }
}

async fn pick_tag(ctx: &Ctx, text: &str, from_core: Option<&str>, prompted: bool) -> String {
    let tags = &ctx.assets.tags;
    if let Some(t) = felt_library::resolve_known_tag(from_core, tags) {
        if Some(t) != from_core {
            tracing::info!("felt-core's tag {from_core:?} broadened to '{t}'");
        }
        return t.to_owned();
    }
    if !prompted {
        return ctx.assets.fallback_tag().to_owned();
    }
    match ctx.core.reaction_tag(text, &ctx.assets.tag_list).await {
        Ok(Some(t)) => t,
        Ok(None) => ctx.assets.fallback_tag().to_owned(),
        Err(e) => {
            tracing::info!("reaction pick failed ({e:#}) - fallback gesture");
            ctx.assets.fallback_tag().to_owned()
        }
    }
}
