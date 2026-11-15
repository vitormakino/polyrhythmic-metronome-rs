//! OS-specific output identity and latency. The watcher and the stream only
//! see `output_identity` and `query_output_latency`.

#[cfg(target_os = "ios")]
mod ios;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "ios")]
pub(crate) use ios::output_identity;
#[cfg(target_os = "ios")]
pub use ios::query_output_latency;
#[cfg(target_os = "macos")]
pub(crate) use macos::output_identity;
#[cfg(target_os = "macos")]
pub use macos::query_output_latency;

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use crate::watch::{Observation, RebuildReason, WatchState, rebuild_reason};

    /// On iOS a headphone plug changes the route's first output, which the
    /// watcher sees as a changed default uid, like a Mac default-device change.
    #[test]
    fn route_change_is_a_new_device() {
        let start = Instant::now();
        let mut state = WatchState::new(start);
        let mut tick = |n: u32, default: &str| {
            rebuild_reason(
                &mut state,
                Observation {
                    now: start + Duration::from_millis(250) * n,
                    wall_jumped: false,
                    callbacks: u64::from(n) * 20,
                    error: None,
                    uid: "Speaker",
                    default_uid: Some(default.to_owned()),
                    suspended: false,
                },
            )
        };
        assert_eq!(tick(1, "Speaker"), None);
        assert_eq!(tick(2, "Headphones"), None);
        assert_eq!(
            tick(3, "Headphones"),
            Some(RebuildReason::DeviceChanged {
                from: Some("Speaker".to_owned()),
                to: Some("Headphones".to_owned()),
            })
        );
    }
}
