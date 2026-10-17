//! The weather outside (WP-21): felt-core's cached current condition (`/v1/weather/now`), every 10 minutes. Rain
//! brings the umbrella out right away, and the ambient loop repeats it now and then while it rains.

use std::rc::Rc;
use std::time::Duration;

use super::ctx::Ctx;
use super::props;

const POLL: Duration = Duration::from_secs(600);

pub async fn run(ctx: Rc<Ctx>) {
    loop {
        match ctx.core.weather_now().await {
            Ok(Some(condition)) => {
                let changed = ctx.weather.borrow().as_deref() != Some(condition.as_str());
                if changed {
                    tracing::info!("weather: {:?} -> {condition}", ctx.weather.borrow());
                    if condition == "rain" {
                        props::start_event(&ctx, "weather:rain");
                    }
                    ctx.weather.replace(Some(condition));
                }
            }
            Ok(None) => {
                tracing::info!(
                    "weather: felt-core has no current condition endpoint - weather props off"
                );
                return;
            }
            Err(e) => tracing::debug!("weather: {e:#}"),
        }
        tokio::time::sleep(POLL).await;
    }
}
