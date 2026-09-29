// Gradually moves the active power parameters to a new target over the ramp
// time (rampTms) requested by a control. See docs/bridging_decisions.md.

use async_broadcast::Sender as BroadcastSender;
use std::time::Duration;
use tokio::{
    sync::mpsc::Receiver as MpscReceiver,
    time::{self, Instant},
};

use crate::{ScaledValue, ScaledValueInner, modbus_connection::Parameters};

#[derive(Clone, Debug)]
pub enum Command {
    /// New parameters to move to, over `ramp_time` if given.
    UpdateTarget {
        parameters: Parameters,
        ramp_time: Option<Duration>,
    },
}

#[derive(Clone, Debug)]
pub enum Event {
    ParametersChanged(Parameters),
}

// How often the parameters are stepped towards the target during a ramp.
const RAMP_STEP_PERIOD: Duration = Duration::from_secs(1);

struct Ramp {
    start: Parameters,
    started: Instant,
    duration: Duration,
}

pub async fn task(
    output_ch: BroadcastSender<Event>,
    mut input_ch: MpscReceiver<Command>,
) -> crate::Result<()> {
    // The parameters last emitted.
    let mut current: Option<Parameters> = None;
    let mut target: Option<Parameters> = None;
    let mut ramp: Option<Ramp> = None;

    loop {
        match time::timeout(RAMP_STEP_PERIOD, input_ch.recv()).await {
            Err(_) => {
                // Timeout: wake up to step any ramp in progress.
            }
            Ok(None) => break,
            Ok(Some(Command::UpdateTarget {
                parameters,
                ramp_time,
            })) => {
                log::trace!("Received new target parameters with ramp time {ramp_time:?}");
                // Starting from the current parameters means a new target
                // arriving mid-ramp continues on from the present values.
                ramp = match (&current, ramp_time) {
                    (Some(start), Some(duration)) if !duration.is_zero() => Some(Ramp {
                        start: start.clone(),
                        started: Instant::now(),
                        duration,
                    }),
                    _ => None,
                };
                target = Some(parameters);
            }
        }

        let Some(target) = &target else {
            continue;
        };

        let next = match &ramp {
            Some(Ramp {
                start,
                started,
                duration,
            }) if started.elapsed() < *duration => interpolate(
                start,
                target,
                started.elapsed().as_secs_f64() / duration.as_secs_f64(),
            ),
            _ => {
                ramp = None;
                target.clone()
            }
        };

        if current.as_ref() != Some(&next) {
            output_ch
                .broadcast(Event::ParametersChanged(next.clone()))
                .await
                .map_err(|_| crate::Error::ChannelClosed)?;
            current = Some(next);
        }
    }

    log::info!("Input channel closed, stopping ramp loop");

    Ok(())
}

/// Returns the target parameters, but with the active power values moved only
/// `fraction` of the way from `start`.
fn interpolate(start: &Parameters, target: &Parameters, fraction: f64) -> Parameters {
    // No maximum limit is the same as a limit of 100%.
    let start_w_max_lim_pct = start.w_max_lim_pct.or(Some(ScaledValue::new(100, 0)));

    Parameters {
        w_max_lim_pct: lerp(start_w_max_lim_pct, target.w_max_lim_pct, fraction),
        w_set_pct: lerp(start.w_set_pct, target.w_set_pct, fraction),
        w_set: lerp(start.w_set, target.w_set, fraction),
        ..target.clone()
    }
}

/// Linear interpolation at the target's scale factor. If either value is
/// missing, there is nothing to interpolate and the target applies.
fn lerp<T: ScaledValueInner>(
    start: Option<ScaledValue<T>>,
    target: Option<ScaledValue<T>>,
    fraction: f64,
) -> Option<ScaledValue<T>> {
    let (Some(start), Some(target)) = (start, target) else {
        return target;
    };
    let start: i64 = start.rescale(target.sf).value.into();
    let end: i64 = target.value.into();
    let value = (start as f64 + (end - start) as f64 * fraction).round() as i64;
    // The value lies between start and end, so always fits.
    Some(ScaledValue::new(
        T::try_from(value).unwrap_or(target.value),
        target.sf,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test(start_paused = true)]
    async fn ramps_to_target_over_ramp_time() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let (output_tx, mut output_rx) = async_broadcast::broadcast(10);
        tokio::spawn(task(output_tx, input_rx));
        let w_set = |value| Parameters {
            w_set: Some(ScaledValue::new(value, 0)),
            ..Default::default()
        };

        input_tx
            .send(Command::UpdateTarget {
                parameters: w_set(0),
                ramp_time: None,
            })
            .await
            .expect("Send error");
        input_tx
            .send(Command::UpdateTarget {
                parameters: w_set(4000),
                ramp_time: Some(Duration::from_secs(4)),
            })
            .await
            .expect("Send error");

        let mut values = Vec::new();
        while values.last() != Some(&4000) {
            let Event::ParametersChanged(parameters) = output_rx.recv().await.expect("Recv error");
            values.push(parameters.w_set.expect("Missing w_set").value);
        }
        assert_eq!(values, vec![0, 1000, 2000, 3000, 4000]);
    }

    #[test]
    fn interpolates_midpoint() {
        let start = Parameters {
            w_set: Some(ScaledValue::new(1000, 0)),
            ..Default::default()
        };
        let target = Parameters {
            w_set: Some(ScaledValue::new(3000, 0)),
            ..Default::default()
        };

        let result = interpolate(&start, &target, 0.5);

        assert_eq!(result.w_set, Some(ScaledValue::new(2000, 0)));
    }

    #[test]
    fn interpolates_at_target_scale_factor() {
        let start = Parameters {
            w_set_pct: Some(ScaledValue::new(10, 0)),
            ..Default::default()
        };
        let target = Parameters {
            w_set_pct: Some(ScaledValue::new(-2000, -2)),
            ..Default::default()
        };

        let result = interpolate(&start, &target, 0.25);

        assert_eq!(result.w_set_pct, Some(ScaledValue::new(250, -2)));
    }

    #[test]
    fn missing_max_limit_starts_at_100_percent() {
        let target = Parameters {
            w_max_lim_pct: Some(ScaledValue::new(4000, -2)),
            ..Default::default()
        };

        let result = interpolate(&Parameters::default(), &target, 0.5);

        assert_eq!(result.w_max_lim_pct, Some(ScaledValue::new(7000, -2)));
    }
}
