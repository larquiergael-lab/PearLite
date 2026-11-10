//! Discord Rich Presence ("Listening to ...") on a background thread, so a slow
//! or absent Discord client never blocks the UI.

use crate::media::Track;
use discord_rich_presence::activity::{Activity, ActivityType, Assets, Timestamps};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

enum Msg {
    Playing { track: Track, position: f64, duration: f64 },
    Clear,
}

pub struct Discord {
    tx: Sender<Msg>,
}

impl Discord {
    pub fn spawn(client_id: String) -> Self {
        let (tx, rx) = mpsc::channel::<Msg>();
        std::thread::Builder::new()
            .name("discord-rpc".into())
            .spawn(move || {
                let mut client = DiscordIpcClient::new(&client_id);
                let mut connected = false;
                let mut pending: Option<Msg> = None;
                loop {
                    match rx.recv_timeout(Duration::from_secs(15)) {
                        Ok(m) => pending = Some(m),
                        // Nothing new: retry a pending update (Discord may have been started since).
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                    // Only the newest state matters.
                    while let Ok(m) = rx.try_recv() {
                        pending = Some(m);
                    }
                    let Some(msg) = &pending else { continue };
                    if !connected {
                        connected = client.connect().is_ok();
                        if !connected {
                            continue;
                        }
                    }
                    let sent = match msg {
                        Msg::Playing { track, position, duration } => client.set_activity(activity(track, *position, *duration)),
                        Msg::Clear => client.clear_activity(),
                    };
                    match sent {
                        Ok(()) => pending = None,
                        Err(_) => {
                            let _ = client.close();
                            connected = false;
                        }
                    }
                }
                if connected {
                    let _ = client.clear_activity();
                    let _ = client.close();
                }
            })
            .expect("spawn discord thread");
        Self { tx }
    }

    pub fn playing(&self, track: &Track, position: f64, duration: f64) {
        let _ = self.tx.send(Msg::Playing { track: track.clone(), position, duration });
    }

    pub fn clear(&self) {
        let _ = self.tx.send(Msg::Clear);
    }
}

/// Discord rejects fields shorter than 2 or longer than 128 characters.
fn field(s: &str) -> String {
    let mut s: String = s.chars().take(128).collect();
    while s.chars().count() < 2 {
        s.push('\u{3164}');
    }
    s
}

fn activity(track: &Track, position: f64, duration: f64) -> Activity<'static> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64;
    let start = now - (position * 1000.0) as i64;
    let mut timestamps = Timestamps::new().start(start);
    if duration > 0.0 {
        timestamps = timestamps.end(start + (duration * 1000.0) as i64);
    }
    let mut assets = Assets::new().large_text(field(if track.album.is_empty() { &track.title } else { &track.album }));
    if track.art.starts_with("https://") {
        assets = assets.large_image(track.art.clone());
    }
    Activity::new()
        .activity_type(ActivityType::Listening)
        .details(field(&track.title))
        .state(field(&track.artist))
        .assets(assets)
        .timestamps(timestamps)
}
